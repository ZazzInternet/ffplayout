//! One browser session: a loaded page and the stream captured from it.

use std::time::Duration;

use log::*;
use sqlx::SqlitePool;
use tokio::time::{Instant, sleep_until};

use crate::db::{handles, models::WebPage};

use super::{
    Geometry, WebPageTest,
    browser::{Browser, Capture, Tools},
    steps,
};

/// A page that is loaded and streaming. Dropping it stops the capture, the
/// browser and their helper processes, in that order.
pub(super) struct BrowserSession {
    capture: Capture,
    _browser: Browser,
    url: String,
    name: String,
}

impl BrowserSession {
    /// Starts a browser for the stored page, runs its steps and begins to
    /// stream. Dropping the future stops whatever was started so far.
    pub(super) async fn open(
        pool: &SqlitePool,
        channel_id: i32,
        page_id: i32,
        geometry: Geometry,
    ) -> Result<Self, String> {
        let page = handles::select_web_page(pool, channel_id, page_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "the page does not exist on this channel".to_string())?;

        debug!(channel = channel_id;
            "Load web page <span class=\"log-addr\">{}</span>: <span class=\"log-addr\">{}</span>",
            page.name,
            page.url
        );

        let tools = Tools::detect().map_err(|error| error.to_string())?;
        let mut browser = Browser::launch(&tools, &page, geometry, true)
            .await
            .map_err(|error| format!("{error:#}"))?;

        steps::load_page(&mut browser.cdp, &page)
            .await
            .map_err(|error| error.to_string())?;

        let (capture, url) = browser
            .capture(&tools)
            .await
            .map_err(|error| format!("{error:#}"))?;

        Ok(Self {
            capture,
            _browser: browser,
            url,
            name: page.name,
        })
    }

    /// The live source the engine plays.
    pub(super) fn url(&self) -> &str {
        &self.url
    }

    pub(super) fn name(&self) -> &str {
        &self.name
    }

    /// Resolves with a description when the stream stops by itself.
    pub(super) async fn stopped(&mut self) -> String {
        self.capture.stopped().await
    }
}

/// Loads a page as playback would, without capturing it.
///
/// On air a page starts loading its lead time before the slot and keeps
/// loading until then. The screenshot is taken after the same time, so it
/// shows the picture the slot begins with, not a page that is still filling.
pub(super) async fn test_page(page: &WebPage, geometry: Geometry) -> WebPageTest {
    let on_air = Instant::now() + Duration::from_secs(u64::from(page.preroll));
    let tools = match Tools::detect() {
        Ok(tools) => tools,
        Err(error) => return WebPageTest::failed(error.to_string()),
    };
    let mut browser = match Browser::launch(&tools, page, geometry, false).await {
        Ok(browser) => browser,
        Err(error) => return WebPageTest::failed(format!("{error:#}")),
    };
    let loaded = steps::load_page(&mut browser.cdp, page).await;

    if loaded.is_ok() {
        sleep_until(on_air).await;
    }

    // A screenshot of the failing state shows why a step did not match.
    let screenshot = browser.cdp.screenshot().await.ok();

    match loaded {
        Ok(()) => WebPageTest {
            success: true,
            error: None,
            failed_step: None,
            screenshot,
        },
        Err(error) => WebPageTest {
            success: false,
            error: Some(error.message),
            failed_step: error.step,
            screenshot,
        },
    }
}
