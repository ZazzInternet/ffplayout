//! Loads a web page and runs its automated steps through the DevTools protocol.

use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};
use tokio::time::{Instant, sleep};

use crate::db::models::{WebPage, WebStep, WebStepAction};

use super::cdp::Cdp;

/// A document that keeps loading, such as one with a streaming request, is
/// used as it is after this time.
const LOAD_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_ELEMENT_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_WAIT: Duration = Duration::from_secs(1);
const POLL_INTERVAL: Duration = Duration::from_millis(200);
/// Lets the result of the last interaction render before the page goes on air.
const SETTLE_TIME: Duration = Duration::from_millis(500);
/// Chrome announces full screen with a "press Esc to exit" notice that it
/// removes by itself; wait until it is gone.
const FULLSCREEN_NOTICE_TIME: Duration = Duration::from_secs(5);

/// Why a page could not be prepared. `step` is the one-based failing step.
#[derive(Debug)]
pub(super) struct LoadError {
    pub(super) step: Option<usize>,
    pub(super) message: String,
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.step {
            Some(step) => write!(formatter, "step {step}: {}", self.message),
            None => formatter.write_str(&self.message),
        }
    }
}

/// Opens the page and runs every step in order. Optional steps may fail.
pub(super) async fn load_page(cdp: &mut Cdp, page: &WebPage) -> Result<(), LoadError> {
    navigate(cdp, &page.url).await.map_err(|error| LoadError {
        step: None,
        message: error.to_string(),
    })?;

    for (index, step) in page.steps.iter().enumerate() {
        if let Err(error) = run_step(cdp, step).await
            && !step.optional
        {
            return Err(LoadError {
                step: Some(index + 1),
                message: error.to_string(),
            });
        }
    }

    sleep(SETTLE_TIME).await;

    Ok(())
}

async fn run_step(cdp: &mut Cdp, step: &WebStep) -> Result<()> {
    let element_timeout = if step.seconds > 0.0 {
        Duration::from_secs_f64(step.seconds)
    } else {
        DEFAULT_ELEMENT_TIMEOUT
    };

    match step.action {
        WebStepAction::Wait => {
            let pause = if step.seconds > 0.0 {
                Duration::from_secs_f64(step.seconds)
            } else {
                DEFAULT_WAIT
            };

            sleep(pause).await;
        }
        WebStepAction::WaitFor => {
            wait_for_element(cdp, &step.selector, element_timeout).await?;
        }
        WebStepAction::Click => {
            let (x, y) = wait_for_element(cdp, &step.selector, element_timeout).await?;

            click(cdp, x, y).await?;
        }
        WebStepAction::Type => {
            wait_for_element(cdp, &step.selector, element_timeout).await?;

            let focus = format!(
                "(() => {{ const el = document.querySelector({}); el.focus(); \
                 if (typeof el.select === 'function') el.select(); }})()",
                js_string(&step.selector)
            );

            cdp.evaluate(&focus, true).await?;
            cdp.call("Input.insertText", json!({"text": step.value}))
                .await?;
        }
        WebStepAction::Press => press_key(cdp, &step.value).await?,
        WebStepAction::Goto => navigate(cdp, &step.value).await?,
        WebStepAction::Script => {
            cdp.evaluate(&step.value, true).await?;
        }
        WebStepAction::Fullscreen => {
            let target = if step.selector.trim().is_empty() {
                "document.documentElement".to_string()
            } else {
                wait_for_element(cdp, &step.selector, element_timeout).await?;

                format!("document.querySelector({})", js_string(&step.selector))
            };

            cdp.evaluate(&format!("{target}.requestFullscreen()"), true)
                .await?;
            sleep(FULLSCREEN_NOTICE_TIME).await;
        }
    }

    Ok(())
}

async fn navigate(cdp: &mut Cdp, url: &str) -> Result<()> {
    let reply = cdp.call("Page.navigate", json!({"url": url})).await?;

    if let Some(error) = reply["errorText"].as_str().filter(|text| !text.is_empty()) {
        bail!("failed to open {url}: {error}");
    }

    let deadline = Instant::now() + LOAD_TIMEOUT;

    while Instant::now() < deadline {
        // Evaluation fails while the navigation replaces the document.
        if cdp
            .evaluate("document.readyState", false)
            .await
            .is_ok_and(|state| state == "complete")
        {
            break;
        }

        sleep(POLL_INTERVAL).await;
    }

    Ok(())
}

/// Waits for a visible element, scrolls it into view and returns its centre.
async fn wait_for_element(cdp: &mut Cdp, selector: &str, wait: Duration) -> Result<(f64, f64)> {
    let locate = format!(
        "(() => {{ const el = document.querySelector({}); if (!el) return null; \
         const box = el.getBoundingClientRect(); \
         if (box.width === 0 || box.height === 0) return null; \
         el.scrollIntoView({{block: 'center', inline: 'center'}}); \
         const rect = el.getBoundingClientRect(); \
         return [rect.left + rect.width / 2, rect.top + rect.height / 2]; }})()",
        js_string(selector)
    );
    let deadline = Instant::now() + wait;
    let mut last_error = None;

    loop {
        match cdp.evaluate(&locate, false).await {
            Ok(Value::Array(point)) => {
                if let [Some(x), Some(y)] = [point[0].as_f64(), point[1].as_f64()] {
                    return Ok((x, y));
                }
            }
            Ok(_) => last_error = None,
            // A navigation in progress also ends here and resolves itself.
            Err(error) => last_error = Some(error),
        }

        if Instant::now() >= deadline {
            break;
        }

        sleep(POLL_INTERVAL).await;
    }

    Err(match last_error {
        Some(error) => anyhow!("element {selector:?}: {error}"),
        None => anyhow!("element {selector:?} did not appear"),
    })
}

async fn click(cdp: &mut Cdp, x: f64, y: f64) -> Result<()> {
    for event in ["mouseMoved", "mousePressed", "mouseReleased"] {
        cdp.call(
            "Input.dispatchMouseEvent",
            json!({"type": event, "x": x, "y": y, "button": "left", "clickCount": 1}),
        )
        .await?;
    }

    Ok(())
}

async fn press_key(cdp: &mut Cdp, name: &str) -> Result<()> {
    let key = key_event(name).ok_or_else(|| anyhow!("unknown key {name:?}"))?;

    for event in ["keyDown", "keyUp"] {
        let mut params = key.clone();

        params["type"] = json!(event);
        cdp.call("Input.dispatchKeyEvent", params).await?;
    }

    Ok(())
}

/// Describes a named key such as `Enter` or `F5`, or a single character.
fn key_event(name: &str) -> Option<Value> {
    let named = |key: &str, code: &str, virtual_key: u32, text: Option<&str>| {
        let mut event = json!({
            "key": key,
            "code": code,
            "windowsVirtualKeyCode": virtual_key,
            "nativeVirtualKeyCode": virtual_key,
        });

        if let Some(text) = text {
            event["text"] = json!(text);
        }

        Some(event)
    };
    let name = name.trim_matches(|character| character == '\r' || character == '\n');

    match name.to_ascii_lowercase().as_str() {
        "enter" | "return" => return named("Enter", "Enter", 13, Some("\r")),
        "tab" => return named("Tab", "Tab", 9, None),
        "escape" | "esc" => return named("Escape", "Escape", 27, None),
        "backspace" => return named("Backspace", "Backspace", 8, None),
        "delete" => return named("Delete", "Delete", 46, None),
        "space" | " " => return named(" ", "Space", 32, Some(" ")),
        "arrowleft" | "left" => return named("ArrowLeft", "ArrowLeft", 37, None),
        "arrowup" | "up" => return named("ArrowUp", "ArrowUp", 38, None),
        "arrowright" | "right" => return named("ArrowRight", "ArrowRight", 39, None),
        "arrowdown" | "down" => return named("ArrowDown", "ArrowDown", 40, None),
        "home" => return named("Home", "Home", 36, None),
        "end" => return named("End", "End", 35, None),
        "pageup" => return named("PageUp", "PageUp", 33, None),
        "pagedown" => return named("PageDown", "PageDown", 34, None),
        _ => {}
    }

    if let Some(number) = name
        .strip_prefix(['f', 'F'])
        .and_then(|number| number.parse::<u32>().ok())
        .filter(|number| (1..=12).contains(number))
    {
        let key = format!("F{number}");

        return named(&key, &key, 111 + number, None);
    }

    let mut characters = name.chars();
    let character = characters.next().filter(|_| characters.next().is_none())?;
    let code = match character {
        'a'..='z' | 'A'..='Z' => format!("Key{}", character.to_ascii_uppercase()),
        '0'..='9' => format!("Digit{character}"),
        _ => String::new(),
    };
    let virtual_key = if character.is_ascii_alphanumeric() {
        u32::from(character.to_ascii_uppercase())
    } else {
        0
    };

    named(
        &character.to_string(),
        &code,
        virtual_key,
        Some(&character.to_string()),
    )
}

/// A JSON string is a valid JavaScript string literal.
fn js_string(text: &str) -> String {
    Value::from(text).to_string()
}

#[cfg(test)]
mod tests {
    use super::{js_string, key_event};

    #[test]
    fn keys_resolve_by_name_or_single_character() {
        assert_eq!(key_event("Enter").unwrap()["text"], "\r");
        assert_eq!(key_event("esc").unwrap()["key"], "Escape");
        assert_eq!(key_event("F11").unwrap()["windowsVirtualKeyCode"], 122);
        assert_eq!(key_event("f").unwrap()["code"], "KeyF");
        assert_eq!(key_event("f").unwrap()["text"], "f");
        assert_eq!(key_event("7").unwrap()["windowsVirtualKeyCode"], 55);
        assert!(key_event("F13").is_none());
        assert!(key_event("NoSuchKey").is_none());
        assert!(key_event("").is_none());
    }

    #[test]
    fn selectors_are_embedded_as_string_literals() {
        assert_eq!(
            js_string(r#"a[href="x"]\'); alert(1)//"#),
            r#""a[href=\"x\"]\\'); alert(1)//""#
        );
    }
}
