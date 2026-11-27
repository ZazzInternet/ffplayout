//! Stand-in for platforms without the browser, display and capture programs.

use std::future::pending;

use sqlx::SqlitePool;

use crate::db::models::WebPage;

use super::{Geometry, WebPageTest};

const UNSUPPORTED: &str = "web pages are only supported on Linux";

pub(super) struct BrowserSession;

impl BrowserSession {
    pub(super) async fn open(
        _pool: &SqlitePool,
        _channel_id: i32,
        _page_id: i32,
        _geometry: Geometry,
    ) -> Result<Self, String> {
        Err(UNSUPPORTED.to_string())
    }

    pub(super) fn url(&self) -> &str {
        ""
    }

    pub(super) fn name(&self) -> &str {
        ""
    }

    pub(super) async fn stopped(&mut self) -> String {
        pending().await
    }
}

pub(super) async fn test_page(_page: &WebPage, _geometry: Geometry) -> WebPageTest {
    WebPageTest::failed(UNSUPPORTED.to_string())
}
