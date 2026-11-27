### Web Pages

A playlist can show web pages, for example a dashboard, a scoreboard or a
weather page. ffplayout renders the page with a browser on the server, captures
picture and sound, and plays the result like a live source.

Web pages are available on Linux.

#### Requirements

Install these programs on the host that runs ffplayout:

| Program | Purpose | Debian/Ubuntu package |
| --- | --- | --- |
| Chrome or Chromium | renders the page | `google-chrome-stable` or `chromium` |
| `Xvfb` | virtual display for the browser | `xvfb` |
| `ffmpeg` | captures display and audio | `ffmpeg` (with `x11grab`) |
| `pulseaudio` | page audio (optional) | `pulseaudio` |

Without `pulseaudio` pages play silently. Fonts decide how a page looks; install
the font packages your pages need, for example `fonts-noto` and
`fonts-noto-color-emoji`.

The browser is searched on `PATH` as `google-chrome-stable`, `google-chrome`,
`chromium` or `chromium-browser`. Two environment variables adjust it:

- `FFPLAYOUT_WEB_CHROME`: full path of the browser executable.
- `FFPLAYOUT_WEB_CHROME_ARGS`: additional browser flags separated by spaces,
  for example `--no-sandbox` in a container that does not permit the Chrome
  sandbox. When ffplayout runs as `root`, `--no-sandbox` is added
  automatically. Run ffplayout as an unprivileged user where possible.

#### Create a page

Open **Configure → Web pages** (channel and global administrators) and enter:

- **Name** and **Address (URL)**: only `http` and `https` addresses are allowed.
- **Zoom**: works like the zoom of a desktop browser. The page always fills
  the picture, at the size and frame rate of the program output; `0.8` makes
  the content smaller so that more of the page fits, `1.5` enlarges it.
- **Lead time**: how many seconds before its playlist entry the page starts
  loading, 5 to 300. It must cover the browser start and all steps.
- **Play the audio of the page**.
- **Steps**: interactions that run after the address has loaded.

| Step | Element | Value | Seconds |
| --- | --- | --- | --- |
| Wait for element | CSS selector | | timeout, default 15 |
| Click | CSS selector | | timeout, default 15 |
| Enter text | CSS selector | text | timeout, default 15 |
| Press key | | `Enter`, `Tab`, `Escape`, `Space`, arrow keys, `F1`–`F12` or one character | |
| Wait | | | pause, default 1 |
| Open address | | URL | |
| Run JavaScript | | script; a returned promise is awaited | |
| Full screen | optional CSS selector | | |

A step that is marked **Continue if this step fails** is skipped when its
element does not appear, which suits cookie banners. **Hide value** masks a
password in the interface; it is stored in the ffplayout database and only
returned to administrators.

A typical login:

```json
[
    { "action": "type", "selector": "#username", "value": "operator" },
    { "action": "type", "selector": "#password", "value": "secret", "secret": true },
    { "action": "click", "selector": "button[type=submit]" },
    { "action": "click", "selector": "a[href='/board']", "seconds": 20 },
    { "action": "wait_for", "selector": "#board" },
    { "action": "fullscreen" }
]
```

The browser window already covers the whole picture without address bar or
tabs, like a desktop browser after F11. The **Full screen** step is for pages
that offer their own full screen mode, such as a video player; alternatively
press the key the page uses for it, for example `f`.

**Test page** loads the page with its steps once without putting it on air. It
reports the failing step and shows a screenshot. The screenshot is taken when
the lead time has passed, so it shows the picture the playlist entry begins
with. If content is still missing there, increase the lead time or add a step
that waits for it.

Every playback starts with a fresh browser profile. Cookies and logins are not
kept, so the steps always see the same starting point.

#### Add a page to a playlist

In the player, use **Add web page to playlist**, select the page and the
duration. The entry references the page by id:

```json
{
    "in": 0,
    "out": 120,
    "duration": 120,
    "title": "Dashboard",
    "source": "web://3"
}
```

Like other live sources, a web page has no timeline: `in` is ignored and the
page is shown for `out` seconds.

#### Behaviour on air

- The browser starts `lead time` seconds before the entry and closes when the
  entry ends. Only the entry after the current one is prepared; an infinite
  playlist continues with its first entry after the last one. When a page is
  the first entry after a start, a skip, a playlist change or the change to
  the playlist of the next day, it loads while its slot already runs.
- While a page is loading, the program shows black instead of a login form or
  a half-loaded page. A page that cannot be loaded stays black for its slot,
  and the reason is written to the channel log.
- A running page needs about one CPU core for rendering and capturing at
  1280x720 and 25 fps, more for larger outputs and for pages with video or
  animations, and 0.5 to 1 GB of memory. The browser renders in software.
- After a **Full screen** step ffplayout waits five seconds, until the browser
  has removed its "press Esc to exit full screen" notice.

#### API

See the [API documentation](api.md) for authentication.

```BASH
# list, create, update, delete
curl http://127.0.0.1:8787/api/web/1 -H 'Authorization: Bearer <TOKEN>'
curl -X POST http://127.0.0.1:8787/api/web/1 -H 'Content-Type: application/json' \
    -H 'Authorization: Bearer <TOKEN>' \
    -d '{ "name": "Dashboard", "url": "https://example.org/board", "audio": true, "zoom": 1.0, "preroll": 20, "steps": [] }'
curl -X PUT http://127.0.0.1:8787/api/web/1/3 -H 'Content-Type: application/json' \
    -H 'Authorization: Bearer <TOKEN>' -d '{ "name": "Dashboard", "url": "https://example.org/board", "steps": [] }'
curl -X DELETE http://127.0.0.1:8787/api/web/1/3 -H 'Authorization: Bearer <TOKEN>'

# load once and return { "success", "error", "failed_step", "screenshot" }
curl -X POST http://127.0.0.1:8787/api/web/1/test -H 'Content-Type: application/json' \
    -H 'Authorization: Bearer <TOKEN>' -d '{ "name": "Dashboard", "url": "https://example.org/board", "steps": [] }'
```

Creating, changing and testing pages requires an administrator role, because
the server opens the address and shows the result.
