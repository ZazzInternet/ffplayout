//! Minimal Chrome DevTools Protocol client for driving one page.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::{net::TcpStream, time::timeout};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};

/// A command that takes longer than this indicates a hung browser.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) struct Cdp {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    next_id: u64,
    session_id: Option<String>,
}

impl Cdp {
    /// Connects to the browser endpoint and attaches to its first page.
    pub(super) async fn connect(browser_url: &str) -> Result<Self> {
        let (socket, _) = timeout(CALL_TIMEOUT, connect_async(browser_url))
            .await
            .context("timed out connecting to the browser")?
            .context("failed to connect to the browser")?;
        let mut cdp = Self {
            socket,
            next_id: 0,
            session_id: None,
        };
        let targets = cdp.call("Target.getTargets", json!({})).await?;
        let target_id = targets["targetInfos"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|target| target["type"] == "page")
            .and_then(|target| target["targetId"].as_str())
            .ok_or_else(|| anyhow!("the browser opened no page"))?
            .to_string();
        let attached = cdp
            .call(
                "Target.attachToTarget",
                json!({"targetId": target_id, "flatten": true}),
            )
            .await?;

        cdp.session_id = Some(
            attached["sessionId"]
                .as_str()
                .ok_or_else(|| anyhow!("the browser returned no page session"))?
                .to_string(),
        );

        Ok(cdp)
    }

    /// Sends one command and waits for its reply; events are discarded.
    pub(super) async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        timeout(CALL_TIMEOUT, self.call_unbounded(method, params))
            .await
            .with_context(|| format!("browser command {method} timed out"))?
    }

    async fn call_unbounded(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next_id += 1;

        let id = self.next_id;
        let mut request = json!({"id": id, "method": method, "params": params});

        if let Some(session_id) = &self.session_id {
            request["sessionId"] = json!(session_id);
        }

        self.socket
            .send(Message::Text(request.to_string().into()))
            .await
            .context("the browser connection closed")?;

        loop {
            let message = self
                .socket
                .next()
                .await
                .ok_or_else(|| anyhow!("the browser connection closed"))?
                .context("the browser connection failed")?;
            let Message::Text(text) = message else {
                continue;
            };
            let reply: Value = serde_json::from_str(text.as_str())?;

            if reply["id"].as_u64() != Some(id) {
                continue;
            }

            if let Some(error) = reply.get("error") {
                bail!(
                    "{method}: {}",
                    error["message"].as_str().unwrap_or("unknown browser error")
                );
            }

            return Ok(reply["result"].clone());
        }
    }

    /// Evaluates JavaScript in the page and returns its JSON value. A returned
    /// promise is awaited. `user_gesture` permits gated APIs such as fullscreen.
    pub(super) async fn evaluate(&mut self, expression: &str, user_gesture: bool) -> Result<Value> {
        let reply = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "awaitPromise": true,
                    "returnByValue": true,
                    "userGesture": user_gesture,
                }),
            )
            .await?;

        if let Some(details) = reply.get("exceptionDetails") {
            let description = details["exception"]["description"]
                .as_str()
                .or_else(|| details["text"].as_str())
                .unwrap_or("script failed");

            bail!("{}", description.lines().next().unwrap_or(description));
        }

        Ok(reply["result"]["value"].clone())
    }

    /// Returns a JPEG of the current viewport, base64 encoded.
    pub(super) async fn screenshot(&mut self) -> Result<String> {
        let reply = self
            .call(
                "Page.captureScreenshot",
                json!({"format": "jpeg", "quality": 80}),
            )
            .await?;

        reply["data"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| anyhow!("the browser returned no screenshot"))
    }
}
