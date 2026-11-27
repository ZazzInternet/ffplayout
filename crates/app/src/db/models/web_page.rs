use serde::{Deserialize, Serialize};
use sqlx::{FromRow, types::Json};
use url::Url;

/// Playlist scheme that references a stored web page by id: `web://<id>`.
pub const WEB_SOURCE_SCHEME: &str = "web://";

const MAX_STEPS: usize = 64;
const MAX_TEXT_LENGTH: usize = 8192;
const MAX_STEP_SECONDS: f64 = 300.0;

/// One automated interaction that runs after the page has loaded, for example
/// to log in or to navigate to the view that should go on air.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebStepAction {
    /// Pause for `seconds`.
    #[default]
    Wait,
    /// Wait until `selector` matches a visible element.
    WaitFor,
    /// Click the element matching `selector`.
    Click,
    /// Focus the element matching `selector` and enter `value`.
    Type,
    /// Press the key named in `value`, such as `Enter` or `f`.
    Press,
    /// Navigate to the URL in `value`.
    Goto,
    /// Run the JavaScript in `value` inside the page.
    Script,
    /// Request fullscreen for `selector`, or for the whole page when empty.
    Fullscreen,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WebStep {
    pub action: WebStepAction,
    pub selector: String,
    pub value: String,
    /// Pause length for `wait`, otherwise how long to wait for the element.
    pub seconds: f64,
    /// A step that is allowed to fail, such as dismissing a cookie banner.
    pub optional: bool,
    /// Marks `value` as a credential so that interfaces can mask it.
    pub secret: bool,
}

impl WebStep {
    fn validate(&self) -> Result<(), String> {
        if !self.seconds.is_finite() || !(0.0..=MAX_STEP_SECONDS).contains(&self.seconds) {
            return Err(format!(
                "step seconds must be between 0 and {MAX_STEP_SECONDS}"
            ));
        }

        if self.selector.len() > MAX_TEXT_LENGTH || self.value.len() > MAX_TEXT_LENGTH {
            return Err("step selector or value is too long".to_string());
        }

        let needs_selector = matches!(
            self.action,
            WebStepAction::WaitFor | WebStepAction::Click | WebStepAction::Type
        );

        if needs_selector && self.selector.trim().is_empty() {
            return Err("this step requires a CSS selector".to_string());
        }

        match self.action {
            WebStepAction::Press | WebStepAction::Script if self.value.is_empty() => {
                Err("this step requires a value".to_string())
            }
            WebStepAction::Goto => validate_page_url(&self.value),
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, FromRow)]
#[serde(default)]
pub struct WebPage {
    pub id: i32,
    pub channel_id: i32,
    pub name: String,
    pub url: String,
    pub steps: Json<Vec<WebStep>>,
    /// Capture the audio that the page plays.
    pub audio: bool,
    /// Device scale factor; larger values enlarge the page content.
    pub zoom: f64,
    /// Seconds before its playlist slot at which the page starts loading.
    pub preroll: u32,
}

impl Default for WebPage {
    fn default() -> Self {
        Self {
            id: 0,
            channel_id: 1,
            name: String::new(),
            url: String::new(),
            steps: Json(Vec::new()),
            audio: true,
            zoom: 1.0,
            preroll: 20,
        }
    }
}

impl WebPage {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() || self.name.len() > 255 {
            return Err("web page name must have 1 to 255 characters".to_string());
        }

        validate_page_url(&self.url)?;

        if !self.zoom.is_finite() || !(0.25..=4.0).contains(&self.zoom) {
            return Err("web page zoom must be between 0.25 and 4".to_string());
        }

        if !(5..=300).contains(&self.preroll) {
            return Err("web page preroll must be between 5 and 300 seconds".to_string());
        }

        if self.steps.len() > MAX_STEPS {
            return Err(format!("a web page supports at most {MAX_STEPS} steps"));
        }

        for (index, step) in self.steps.iter().enumerate() {
            step.validate()
                .map_err(|error| format!("step {}: {error}", index + 1))?;
        }

        Ok(())
    }

    /// The playlist `source` that schedules this page.
    pub fn source(&self) -> String {
        format!("{WEB_SOURCE_SCHEME}{}", self.id)
    }
}

/// Only regular web addresses are browsed; local files and browser-internal
/// pages stay out of reach of playlist editors.
fn validate_page_url(url: &str) -> Result<(), String> {
    if url.len() > MAX_TEXT_LENGTH {
        return Err("URL is too long".to_string());
    }

    let parsed = Url::parse(url).map_err(|_| "a valid http(s) URL is required".to_string())?;

    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err("a valid http(s) URL is required".to_string());
    }

    Ok(())
}

/// Returns the page id of a `web://<id>` playlist source.
pub fn web_page_id(source: &str) -> Option<i32> {
    let id = source.strip_prefix(WEB_SOURCE_SCHEME)?;

    id.trim_end_matches('/').parse().ok().filter(|id| *id > 0)
}

pub fn is_web_source(source: &str) -> bool {
    source
        .get(..WEB_SOURCE_SCHEME.len())
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case(WEB_SOURCE_SCHEME))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> WebPage {
        WebPage {
            name: "Dashboard".to_string(),
            url: "https://example.org/board".to_string(),
            ..WebPage::default()
        }
    }

    #[test]
    fn playlist_sources_resolve_to_page_ids() {
        assert_eq!(web_page_id("web://12"), Some(12));
        assert_eq!(web_page_id("web://12/"), Some(12));
        assert_eq!(web_page_id("web://0"), None);
        assert_eq!(web_page_id("web://board"), None);
        assert_eq!(web_page_id("https://example.org"), None);
        assert!(is_web_source("WEB://3"));
        assert!(!is_web_source("/media/web/clip.mp4"));
        assert_eq!(WebPage { id: 7, ..page() }.source(), "web://7");
    }

    #[test]
    fn pages_only_accept_web_addresses() {
        assert!(page().validate().is_ok());

        for url in ["", "file:///etc/passwd", "chrome://settings", "example.org"] {
            let invalid = WebPage {
                url: url.to_string(),
                ..page()
            };

            assert!(invalid.validate().is_err(), "{url}");
        }
    }

    #[test]
    fn steps_require_the_fields_their_action_uses() {
        let with_step = |step: WebStep| WebPage {
            steps: Json(vec![step]),
            ..page()
        };
        let click = WebStep {
            action: WebStepAction::Click,
            ..WebStep::default()
        };
        let goto_file = WebStep {
            action: WebStepAction::Goto,
            value: "file:///etc/passwd".to_string(),
            ..WebStep::default()
        };
        let typed = WebStep {
            action: WebStepAction::Type,
            selector: "#user".to_string(),
            value: "operator".to_string(),
            seconds: 10.0,
            ..WebStep::default()
        };

        assert!(with_step(click).validate().is_err());
        assert!(with_step(goto_file).validate().is_err());
        assert!(with_step(typed).validate().is_ok());
    }
}
