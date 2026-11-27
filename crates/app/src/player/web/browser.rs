//! Processes that render one web page off screen and capture it as a stream.
//!
//! A page runs in a kiosk-mode Chrome on a private virtual X display. FFmpeg
//! grabs that display, and the page audio from a private PulseAudio sink, and
//! sends both as MPEG-TS to a loopback relay that the engine plays like any
//! other live source.

use std::{
    env,
    fs::{self, DirBuilder},
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use nix::{
    sys::signal::{Signal, killpg},
    unistd::{Pid, Uid},
};
use serde_json::json;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
    time::{Instant, sleep, timeout},
};
use uuid::Uuid;

use crate::db::models::WebPage;

use super::{
    Geometry,
    cdp::Cdp,
    relay::{Relay, VIDEO_PID},
};

const START_TIMEOUT: Duration = Duration::from_secs(20);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Time after which a capture process that is still running counts as started.
const CAPTURE_START_CHECK: Duration = Duration::from_millis(700);
/// Without a window manager Chrome shrinks a window that is exactly as large
/// as the screen by one pixel. A slightly larger screen keeps the window, in
/// its top left corner, at the requested size; only that area is captured.
const SCREEN_MARGIN: u32 = 16;
const AUDIO_SINK: &str = "ffplayout_web";
const AUDIO_SAMPLE_RATE: u32 = 48_000;
/// Environment overrides for hosts where the browser is not on `PATH` or
/// needs additional flags, for example `--no-sandbox` inside a container.
const CHROME_ENV: &str = "FFPLAYOUT_WEB_CHROME";
const CHROME_ARGS_ENV: &str = "FFPLAYOUT_WEB_CHROME_ARGS";
const CHROME_NAMES: [&str; 4] = [
    "google-chrome-stable",
    "google-chrome",
    "chromium",
    "chromium-browser",
];

/// External programs that web page playback depends on.
pub(super) struct Tools {
    xvfb: PathBuf,
    chrome: PathBuf,
    ffmpeg: PathBuf,
    pulseaudio: Option<PathBuf>,
}

impl Tools {
    pub(super) fn detect() -> Result<Self> {
        let chrome = match env::var_os(CHROME_ENV) {
            Some(path) => PathBuf::from(path),
            None => CHROME_NAMES
                .iter()
                .find_map(|name| find_program(name))
                .ok_or_else(|| {
                    anyhow!("no Chrome or Chromium browser found; install one or set {CHROME_ENV}")
                })?,
        };

        Ok(Self {
            xvfb: find_program("Xvfb").ok_or_else(|| anyhow!("Xvfb not found; install xvfb"))?,
            chrome,
            ffmpeg: find_program("ffmpeg")
                .ok_or_else(|| anyhow!("ffmpeg not found; install ffmpeg"))?,
            pulseaudio: find_program("pulseaudio"),
        })
    }
}

fn find_program(name: &str) -> Option<PathBuf> {
    env::split_paths(&env::var_os("PATH")?)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

/// Private working directory of one session, removed when the session ends.
struct SessionDir(PathBuf);

impl SessionDir {
    fn create(channel_id: i32, page_id: i32) -> Result<Self> {
        let path = env::temp_dir().join(format!(
            "ffplayout-web-{channel_id}-{page_id}-{}",
            Uuid::new_v4().simple()
        ));

        // The directory holds the X authority cookie, the audio socket and
        // the browser profile with any session the page logged in to.
        DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .with_context(|| format!("failed to create {}", path.display()))?;

        Ok(Self(path))
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for SessionDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A running browser that shows one page full screen on its own display.
///
/// Fields drop in declaration order: the browser stops before the audio
/// server and the display it draws on, and the directory is removed last.
pub(super) struct Browser {
    pub(super) cdp: Cdp,
    chrome: Child,
    audio: Option<AudioServer>,
    display: Display,
    dir: SessionDir,
    geometry: Geometry,
}

impl Browser {
    pub(super) async fn launch(
        tools: &Tools,
        page: &WebPage,
        geometry: Geometry,
        with_audio: bool,
    ) -> Result<Self> {
        let started = Instant::now();
        let dir = SessionDir::create(page.channel_id, page.id)?;
        let display = Display::start(tools, &dir, geometry).await?;
        let audio = match (&tools.pulseaudio, with_audio && page.audio) {
            (Some(pulseaudio), true) => match AudioServer::start(pulseaudio, &dir).await {
                Ok(server) => Some(server),
                Err(error) => {
                    log::warn!(channel = page.channel_id;
                        "Web page <span class=\"log-addr\">{}</span> plays without audio: {error:#}",
                        page.name
                    );

                    None
                }
            },
            (None, true) => {
                log::warn!(channel = page.channel_id;
                    "Web page <span class=\"log-addr\">{}</span> plays without audio: pulseaudio is not installed",
                    page.name
                );

                None
            }
            _ => None,
        };
        let (chrome, cdp) =
            start_chrome(tools, &dir, &display, audio.as_ref(), page, geometry).await?;

        log::debug!(channel = page.channel_id;
            "Browser for web page <span class=\"log-addr\">{}</span> started in <span class=\"log-number\">{:.1}s</span>",
            page.name,
            started.elapsed().as_secs_f32()
        );

        Ok(Self {
            cdp,
            chrome,
            audio,
            display,
            dir,
            geometry,
        })
    }

    /// Starts streaming the display and the page audio and returns the
    /// capture together with the URL the engine plays.
    pub(super) async fn capture(&self, tools: &Tools) -> Result<(Capture, String)> {
        let relay = Relay::start().await?;
        let Geometry { width, height, fps } = self.geometry;
        let log_path = self.dir.join("capture.log");
        let mut command = Command::new(&tools.ffmpeg);

        // Both devices stamp their data with the wall clock. `-copyts` keeps
        // those stamps, and with them lip sync, instead of starting each
        // device at zero regardless of when it delivered its first data. The
        // output offset below brings the stream back to small timestamps.
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs().saturating_sub(10));

        command
            .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-copyts"])
            .args(["-thread_queue_size", "64", "-f", "x11grab"])
            .args(["-draw_mouse", "0", "-framerate", &fps.to_string()])
            .args(["-video_size", &format!("{width}x{height}")])
            .args(["-i", &self.display.name()]);

        // Without page audio the stream is video only; the engine then
        // plays silence.
        if let Some(audio) = &self.audio {
            command
                .args(["-thread_queue_size", "1024", "-f", "pulse"])
                .args(["-server", &audio.server()])
                .args(["-i", &format!("{AUDIO_SINK}.monitor")])
                .args(["-map", "0:v:0", "-map", "1:a:0"])
                .args(["-c:a", "aac", "-b:a", "256k", "-ac", "2"])
                .args(["-ar", &AUDIO_SAMPLE_RATE.to_string()]);
        }

        command
            .args([
                "-vf",
                "scale=out_color_matrix=bt709:out_range=tv,format=yuv420p",
            ])
            .args(["-colorspace", "bt709", "-color_primaries", "bt709"])
            .args(["-color_trc", "bt709", "-color_range", "tv"]);

        // Short GOPs let the engine join the stream at any moment. The stream
        // never leaves the host, so quality is preferred over bitrate.
        let gop = (fps / 2).max(1).to_string();

        if has_encoder(&tools.ffmpeg, "libx264").await {
            command
                .args(["-c:v", "libx264", "-preset", "ultrafast"])
                .args(["-tune", "zerolatency", "-crf", "12"])
                .args(["-maxrate", "40M", "-bufsize", "20M"]);
        } else {
            command.args(["-c:v", "mpeg2video", "-q:v", "2", "-bf", "0"]);
        }

        command
            .args(["-g", &gop])
            .args(["-output_ts_offset", &format!("-{epoch}")])
            // Program tables in front of every frame let the relay hand a
            // complete, decodable stream to the engine from any key frame.
            .args(["-f", "mpegts", "-mpegts_flags", "+pat_pmt_at_frames"])
            .args(["-mpegts_start_pid", &VIDEO_PID.to_string()])
            .args(["-flush_packets", "1"])
            .arg(relay.ingest_url())
            .env("XAUTHORITY", self.display.authority())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(fs::File::create(&log_path)?)
            .kill_on_drop(true);

        let mut ffmpeg = command.spawn().context("failed to start ffmpeg")?;

        sleep(CAPTURE_START_CHECK).await;

        if let Some(status) = ffmpeg.try_wait()? {
            bail!("capture stopped ({status}): {}", log_tail(&log_path));
        }

        let url = relay.serve_url();

        Ok((
            Capture {
                ffmpeg,
                log_path,
                _relay: relay,
            },
            url,
        ))
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        // Chrome runs in its own process group; stopping only the main
        // process could leave renderer and utility processes behind.
        if let Some(pid) = self.chrome.id().and_then(|pid| i32::try_from(pid).ok()) {
            let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
        }
    }
}

/// The FFmpeg process that streams a browser display.
pub(super) struct Capture {
    ffmpeg: Child,
    log_path: PathBuf,
    _relay: Relay,
}

impl Capture {
    /// Resolves with a description when the capture stops by itself.
    pub(super) async fn stopped(&mut self) -> String {
        match self.ffmpeg.wait().await {
            Ok(status) => format!("capture stopped ({status}): {}", log_tail(&self.log_path)),
            Err(error) => format!("capture failed: {error}"),
        }
    }
}

struct Display {
    xvfb: Child,
    number: u32,
    authority: PathBuf,
}

impl Display {
    async fn start(tools: &Tools, dir: &SessionDir, geometry: Geometry) -> Result<Self> {
        let authority = dir.join("xauthority");

        fs::write(&authority, xauthority_entry(Uuid::new_v4().as_bytes()))?;

        // `-displayfd 1` lets the server pick a free display and report it.
        let mut xvfb = Command::new(&tools.xvfb)
            .args(["-displayfd", "1", "-screen", "0"])
            .arg(format!(
                "{}x{}x24",
                geometry.width + SCREEN_MARGIN,
                geometry.height + SCREEN_MARGIN
            ))
            .args(["-nolisten", "tcp", "-nocursor", "-auth"])
            .arg(&authority)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("failed to start Xvfb")?;
        let stdout = xvfb
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Xvfb has no output"))?;
        let mut line = String::new();

        timeout(START_TIMEOUT, BufReader::new(stdout).read_line(&mut line))
            .await
            .context("Xvfb did not start in time")??;

        let number = line
            .trim()
            .parse()
            .map_err(|_| anyhow!("Xvfb reported no display"))?;

        Ok(Self {
            xvfb,
            number,
            authority,
        })
    }

    fn name(&self) -> String {
        format!(":{}", self.number)
    }

    fn authority(&self) -> &Path {
        // Keep the server process owned for as long as its name is in use.
        let _ = &self.xvfb;

        &self.authority
    }
}

/// One `.Xauthority` record that grants access to any local display with the
/// given MIT magic cookie. Other local users then cannot watch or control it.
fn xauthority_entry(cookie: &[u8; 16]) -> Vec<u8> {
    const FAMILY_WILD: u16 = 0xFFFF;

    let mut entry = FAMILY_WILD.to_be_bytes().to_vec();

    // Address and display number stay empty, which matches every display.
    for field in [&b""[..], b"", b"MIT-MAGIC-COOKIE-1", cookie] {
        let length = u16::try_from(field.len()).expect("authority fields are short");

        entry.extend(length.to_be_bytes());
        entry.extend(field);
    }

    entry
}

struct AudioServer {
    _pulseaudio: Child,
    socket: PathBuf,
}

impl AudioServer {
    async fn start(pulseaudio: &Path, dir: &SessionDir) -> Result<Self> {
        let socket = dir.join("pulse.sock");
        let home = dir.join("pulse");

        fs::create_dir(&home)?;

        // `-n` skips the system configuration: this server only has the
        // socket and a null sink whose monitor carries the page audio.
        let mut child = Command::new(pulseaudio)
            .args(["-n", "--daemonize=no", "--exit-idle-time=-1"])
            .args(["--disallow-exit", "--log-level=error"])
            .arg(format!(
                "--load=module-native-protocol-unix socket={} auth-anonymous=1",
                socket.display()
            ))
            .arg(format!(
                "--load=module-null-sink sink_name={AUDIO_SINK} rate={AUDIO_SAMPLE_RATE} channels=2"
            ))
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &home)
            .env("XDG_CONFIG_HOME", &home)
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .env_remove("DISPLAY")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("failed to start pulseaudio")?;
        let deadline = Instant::now() + START_TIMEOUT;

        while !socket.exists() {
            if let Some(status) = child.try_wait()? {
                bail!("pulseaudio stopped ({status})");
            }

            if Instant::now() >= deadline {
                bail!("pulseaudio did not start in time");
            }

            sleep(POLL_INTERVAL).await;
        }

        Ok(Self {
            _pulseaudio: child,
            socket,
        })
    }

    fn server(&self) -> String {
        format!("unix:{}", self.socket.display())
    }
}

async fn start_chrome(
    tools: &Tools,
    dir: &SessionDir,
    display: &Display,
    audio: Option<&AudioServer>,
    page: &WebPage,
    geometry: Geometry,
) -> Result<(Child, Cdp)> {
    let profile = dir.join("profile");
    let home = dir.join("home");
    let log_path = dir.join("chrome.log");

    fs::create_dir_all(profile.join("Default"))?;
    fs::create_dir(&home)?;
    // Keep browser dialogs about passwords and translation off the air.
    fs::write(
        profile.join("Default").join("Preferences"),
        json!({
            "credentials_enable_service": false,
            "profile": {"password_manager_enabled": false},
            "translate": {"enabled": false},
        })
        .to_string(),
    )?;

    let mut command = Command::new(&tools.chrome);

    command
        .arg(format!("--user-data-dir={}", profile.display()))
        // Port 0 selects a free loopback port, reported in the profile.
        .arg("--remote-debugging-port=0")
        .arg("--kiosk")
        .arg("--window-position=0,0")
        .arg(format!(
            "--window-size={},{}",
            window_points(geometry.width, page.zoom),
            window_points(geometry.height, page.zoom)
        ))
        .arg(format!("--force-device-scale-factor={}", page.zoom))
        .args([
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-search-engine-choice-screen",
            "--disable-infobars",
            "--disable-notifications",
            "--deny-permission-prompts",
            "--disable-session-crashed-bubble",
            "--noerrdialogs",
            "--hide-scrollbars",
            "--password-store=basic",
            "--autoplay-policy=no-user-gesture-required",
            "--disable-features=Translate,MediaRouter,OptimizationHints",
            // Keep update checks and telemetry from competing with the page.
            "--disable-background-networking",
            "--disable-component-update",
            "--disable-sync",
            "--disable-default-apps",
            "--disable-extensions",
            "--disable-breakpad",
            "--disable-domain-reliability",
            "--disable-client-side-phishing-detection",
            "--metrics-recording-only",
            "--no-pings",
            "--disable-background-timer-throttling",
            "--disable-backgrounding-occluded-windows",
            "--disable-renderer-backgrounding",
            "--disable-dev-shm-usage",
            // There is no GPU on a virtual display; render in software.
            "--disable-gpu",
            "--enable-unsafe-swiftshader",
        ]);

    if Uid::effective().is_root() {
        // Chrome refuses to start its sandbox as root.
        command.arg("--no-sandbox");
    }

    if let Some(extra) = env::var_os(CHROME_ARGS_ENV) {
        command.args(extra.to_string_lossy().split_whitespace());
    }

    match audio {
        Some(audio) => command.env("PULSE_SERVER", audio.server()),
        None => command.arg("--mute-audio"),
    };

    command
        .arg("about:blank")
        .env("DISPLAY", display.name())
        .env("XAUTHORITY", display.authority())
        .env("HOME", &home)
        .env("XDG_RUNTIME_DIR", &home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .env_remove("WAYLAND_DISPLAY")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(fs::File::create(&log_path)?)
        .process_group(0)
        .kill_on_drop(true);

    let mut chrome = command.spawn().context("failed to start the browser")?;
    let port_file = profile.join("DevToolsActivePort");
    let deadline = Instant::now() + START_TIMEOUT;
    let endpoint = loop {
        if let Some(endpoint) = devtools_endpoint(&port_file) {
            break endpoint;
        }

        if let Some(status) = chrome.try_wait()? {
            bail!("the browser stopped ({status}): {}", log_tail(&log_path));
        }

        if Instant::now() >= deadline {
            bail!("the browser did not start in time: {}", log_tail(&log_path));
        }

        sleep(POLL_INTERVAL).await;
    };
    let cdp = Cdp::connect(&endpoint).await?;

    Ok((chrome, cdp))
}

/// Chrome takes the window size in points, which it multiplies by the scale
/// factor. Dividing by it keeps the window at the size of the picture, so a
/// smaller zoom shows more of the page instead of a smaller window. Rounding
/// up never leaves a gap; the excess pixel lies outside the captured area.
fn window_points(pixels: u32, zoom: f64) -> u32 {
    (f64::from(pixels) / zoom).ceil() as u32
}

/// Chrome writes its debugging port and browser path, one per line.
fn devtools_endpoint(port_file: &Path) -> Option<String> {
    let content = fs::read_to_string(port_file).ok()?;
    let mut lines = content.lines();
    let port: u16 = lines.next()?.trim().parse().ok()?;
    let path = lines.next()?.trim();

    path.starts_with('/')
        .then(|| format!("ws://127.0.0.1:{port}{path}"))
}

async fn has_encoder(ffmpeg: &Path, name: &str) -> bool {
    let output = Command::new(ffmpeg)
        .args(["-hide_banner", "-encoders"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await;

    output.is_ok_and(|output| {
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line.split_whitespace().nth(1) == Some(name))
    })
}

/// The last lines of a process log, for an error message.
fn log_tail(path: &Path) -> String {
    let content = fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let tail = lines[lines.len().saturating_sub(3)..].join(" | ");

    if tail.is_empty() {
        "no error output".to_string()
    } else {
        tail
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{devtools_endpoint, window_points, xauthority_entry};

    #[test]
    fn window_keeps_the_picture_size_at_any_zoom() {
        assert_eq!(window_points(1280, 1.0), 1280);
        assert_eq!(window_points(1280, 0.8), 1600);
        assert_eq!(window_points(720, 0.8), 900);
        assert_eq!(window_points(1280, 1.5), 854);
    }

    #[test]
    fn authority_record_matches_any_display_with_the_cookie() {
        let entry = xauthority_entry(&[7; 16]);

        // family, empty address, empty number, name, cookie
        assert_eq!(&entry[..6], [0xFF, 0xFF, 0, 0, 0, 0]);
        assert_eq!(&entry[6..8], [0, 18]);
        assert_eq!(&entry[8..26], b"MIT-MAGIC-COOKIE-1");
        assert_eq!(&entry[26..28], [0, 16]);
        assert_eq!(&entry[28..], [7; 16]);
    }

    #[test]
    fn devtools_endpoint_needs_port_and_path() {
        let dir = std::env::temp_dir().join(format!("ffplayout-web-test-{}", uuid::Uuid::new_v4()));
        let file = dir.join("DevToolsActivePort");

        fs::create_dir(&dir).unwrap();
        assert_eq!(devtools_endpoint(&file), None);

        fs::write(&file, "9222\n").unwrap();
        assert_eq!(devtools_endpoint(&file), None);

        fs::write(&file, "9222\n/devtools/browser/abc\n").unwrap();
        assert_eq!(
            devtools_endpoint(&file).as_deref(),
            Some("ws://127.0.0.1:9222/devtools/browser/abc")
        );

        fs::remove_dir_all(dir).unwrap();
    }
}
