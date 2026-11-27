//! Web pages as playlist sources.
//!
//! A playlist entry with the source `web://<id>` shows a stored web page. The
//! page is rendered by a browser session that starts shortly before its slot,
//! runs the configured steps and then streams to the engine until the slot
//! ends. See `browser` for the processes involved.

use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex as StdMutex, PoisonError},
    time::Duration,
};

use log::*;
use serde::Serialize;
use sqlx::SqlitePool;
use tokio::{
    sync::{Mutex, watch},
    time::timeout,
};
use tokio_util::sync::{CancellationToken, DropGuard};

#[cfg(unix)]
mod browser;
#[cfg(unix)]
mod cdp;
#[cfg(unix)]
mod relay;
#[cfg(unix)]
mod session;
#[cfg(unix)]
mod steps;
// The browser, its virtual display and the capture are Unix programs.
#[cfg(not(unix))]
#[path = "unsupported.rs"]
mod session;

use crate::db::models::WebPage;

use session::BrowserSession;

/// Size and frame rate of the rendered page, matching the program output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

/// Upper bound for starting the browser and running all steps of a page.
const LOAD_TIMEOUT: Duration = Duration::from_secs(180);

/// Progress of the browser session behind a scheduled web page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebSessionState {
    Loading,
    /// The page is streaming; the engine plays this URL.
    Ready(String),
    Failed(String),
}

/// Result of loading a page once without putting it on air.
#[derive(Debug, Serialize)]
pub struct WebPageTest {
    pub success: bool,
    pub error: Option<String>,
    /// One-based index of the step that failed.
    pub failed_step: Option<usize>,
    /// Base64 JPEG of the page after the steps, when the browser started.
    pub screenshot: Option<String>,
}

struct Session {
    state: watch::Receiver<WebSessionState>,
    /// Cancels the session task, which stops its processes, when dropped.
    _stop: DropGuard,
}

#[derive(Default)]
struct Inner {
    sessions: StdMutex<HashMap<i32, Session>>,
    /// A test starts a whole browser; run one at a time per channel.
    test: Mutex<()>,
}

/// Browser sessions of the web pages a channel is about to play or playing.
#[derive(Clone, Default)]
pub struct WebSources(Arc<Inner>);

impl fmt::Debug for WebSources {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("WebSources")
    }
}

impl WebSources {
    /// Starts loading a page unless its session already exists, and returns
    /// a receiver for the session state.
    pub fn start(
        &self,
        pool: &SqlitePool,
        channel_id: i32,
        page_id: i32,
        geometry: Geometry,
    ) -> watch::Receiver<WebSessionState> {
        let mut sessions = self
            .0
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner);

        if let Some(session) = sessions.get(&page_id) {
            return session.state.clone();
        }

        let (sender, state) = watch::channel(WebSessionState::Loading);
        let token = CancellationToken::new();

        tokio::spawn(run_session(
            pool.clone(),
            channel_id,
            page_id,
            geometry,
            sender,
            token.clone(),
        ));
        sessions.insert(
            page_id,
            Session {
                state: state.clone(),
                _stop: token.drop_guard(),
            },
        );

        state
    }

    /// Stops every session whose page is not in `page_ids`.
    pub fn retain(&self, page_ids: &[i32]) {
        self.0
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|page_id, _| page_ids.contains(page_id));
    }

    pub fn stop_all(&self) {
        self.retain(&[]);
    }

    /// Loads a page once, as playback would, and returns a screenshot.
    pub async fn test(&self, page: &WebPage, geometry: Geometry) -> WebPageTest {
        let _running = self.0.test.lock().await;

        // The test waits for the lead time of the page before its screenshot.
        let limit = LOAD_TIMEOUT + Duration::from_secs(u64::from(page.preroll));

        match timeout(limit, session::test_page(page, geometry)).await {
            Ok(result) => result,
            Err(_) => WebPageTest::failed("the page did not load in time".to_string()),
        }
    }
}

impl WebPageTest {
    pub(super) fn failed(error: String) -> Self {
        Self {
            success: false,
            error: Some(error),
            failed_step: None,
            screenshot: None,
        }
    }
}

/// Owns the processes of one session until it is cancelled or its capture
/// stops. Dropping the browser and the capture ends their processes.
async fn run_session(
    pool: SqlitePool,
    channel_id: i32,
    page_id: i32,
    geometry: Geometry,
    state: watch::Sender<WebSessionState>,
    token: CancellationToken,
) {
    let opened = tokio::select! {
        () = token.cancelled() => return,
        opened = timeout(
            LOAD_TIMEOUT,
            BrowserSession::open(&pool, channel_id, page_id, geometry),
        ) => opened,
    };
    let mut session = match opened {
        Ok(Ok(session)) => session,
        Ok(Err(error)) => {
            error!(channel = channel_id;
                "Web page <span class=\"log-addr\">web://{page_id}</span> failed to load: {error}"
            );
            state.send_replace(WebSessionState::Failed(error));

            // Stay registered until stopped, so a failing page is not
            // restarted over and over within the same playlist slot.
            token.cancelled().await;

            return;
        }
        Err(_) => {
            error!(channel = channel_id;
                "Web page <span class=\"log-addr\">web://{page_id}</span> did not load in time"
            );
            state.send_replace(WebSessionState::Failed("load timeout".to_string()));
            token.cancelled().await;

            return;
        }
    };

    let name = session.name().to_string();

    info!(channel = channel_id;
        "Web page <span class=\"log-addr\">{name}</span> is ready"
    );
    state.send_replace(WebSessionState::Ready(session.url().to_string()));

    tokio::select! {
        () = token.cancelled() => {}
        reason = session.stopped() => {
            error!(channel = channel_id;
                "Web page <span class=\"log-addr\">{name}</span>: {reason}"
            );
            state.send_replace(WebSessionState::Failed(reason));
            token.cancelled().await;
        }
    }

    drop(session);
    debug!(channel = channel_id; "Web page <span class=\"log-addr\">{name}</span> closed");
}
