//! Commands for Del's visible browser. All page data is untrusted content.
use del_agent::SessionId;
use serde::{Deserialize, Serialize};

/// Most tabs one session's browser may hold through agent `new_tab` calls.
pub const MAX_AGENT_TABS: usize = 20;

/// Which sessions' browsers one browser request may touch. The server derives
/// this from the authenticated caller; hosts must never widen it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowserAccess {
    /// The caller's own session plus the sessions it spawned and its parent.
    Sessions(Vec<SessionId>),
    /// A credential shared by several chats of one provider process (OpenCode
    /// serves one MCP config per workspace), so it cannot name its caller:
    /// exactly those chats. Hosts further limit it to the ones running a turn.
    Shared(Vec<SessionId>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserRequest {
    pub action: String,
    pub session_id: Option<String>,
    pub tab_id: Option<String>,
    pub url: Option<String>,
    pub selector: Option<String>,
    pub value: Option<String>,
    pub script: Option<String>,
    pub key: Option<String>,
}
impl BrowserRequest {
    pub fn validate(&self) -> Result<(), String> {
        if ![
            "list_tabs",
            "new_tab",
            "select_tab",
            "close_tab",
            "navigate",
            "back",
            "forward",
            "reload",
            "snapshot",
            "screenshot",
            "click",
            "fill",
            "press_key",
            "evaluate",
        ]
        .contains(&self.action.as_str())
        {
            return Err("Unknown browser action".into());
        }
        for text in [
            &self.session_id,
            &self.tab_id,
            &self.url,
            &self.selector,
            &self.value,
            &self.script,
            &self.key,
        ]
        .into_iter()
        .flatten()
        {
            if text.len() > 50000 || text.contains('\0') {
                return Err("Browser argument too long or contains NUL".into());
            }
        }
        let required = match self.action.as_str() {
            "navigate" => self.url.as_deref(),
            "click" | "fill" => self.selector.as_deref(),
            "evaluate" => self.script.as_deref(),
            "press_key" => self.key.as_deref(),
            _ => Some("ok"),
        };
        if required.is_none_or(|text| text.trim().is_empty()) {
            return Err("Missing required browser action argument".into());
        }
        if self.action == "fill" && self.value.is_none() {
            return Err("fill requires value".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(action: &str) -> BrowserRequest {
        BrowserRequest {
            action: action.into(),
            session_id: None,
            tab_id: None,
            url: None,
            selector: None,
            value: None,
            script: None,
            key: None,
        }
    }

    #[test]
    fn every_documented_action_validates_with_its_required_argument() {
        for action in [
            "list_tabs",
            "new_tab",
            "select_tab",
            "close_tab",
            "back",
            "forward",
            "reload",
            "snapshot",
            "screenshot",
        ] {
            assert_eq!(request(action).validate(), Ok(()), "{action}");
        }
        let with = |action: &str, set: fn(&mut BrowserRequest)| {
            let mut r = request(action);
            set(&mut r);
            r
        };
        assert!(
            with("navigate", |r| r.url = Some("https://a.test".into()))
                .validate()
                .is_ok()
        );
        assert!(
            with("click", |r| r.selector = Some("#go".into()))
                .validate()
                .is_ok()
        );
        assert!(
            with("evaluate", |r| r.script = Some("return 1".into()))
                .validate()
                .is_ok()
        );
        assert!(
            with("press_key", |r| r.key = Some("Enter".into()))
                .validate()
                .is_ok()
        );
        assert!(
            with("fill", |r| {
                r.selector = Some("#q".into());
                r.value = Some(String::new());
            })
            .validate()
            .is_ok(),
            "an empty fill value clears the field"
        );
    }

    #[test]
    fn missing_blank_unknown_oversized_and_nul_arguments_are_rejected() {
        for action in ["navigate", "click", "fill", "evaluate", "press_key"] {
            assert!(request(action).validate().is_err(), "{action}");
        }
        let mut r = request("navigate");
        r.url = Some("   ".into());
        assert!(r.validate().is_err());
        let mut r = request("fill");
        r.selector = Some("#q".into());
        assert_eq!(r.validate(), Err("fill requires value".into()));
        for action in ["", "List_tabs", "open", "close"] {
            assert_eq!(
                request(action).validate(),
                Err("Unknown browser action".into())
            );
        }
        let mut r = request("snapshot");
        r.tab_id = Some("x".repeat(50_000));
        assert!(r.validate().is_ok());
        r.tab_id = Some("x".repeat(50_001));
        assert!(r.validate().is_err());
        let mut r = request("evaluate");
        r.script = Some("return 1\0".into());
        assert!(r.validate().is_err());
        let mut r = request("snapshot");
        r.session_id = Some("a\0b".into());
        assert!(r.validate().is_err());
    }

    #[test]
    fn unknown_fields_are_rejected_when_parsing() {
        let parsed: Result<BrowserRequest, _> =
            serde_json::from_value(serde_json::json!({"action":"snapshot","frame":"x"}));
        assert!(parsed.is_err());
        let parsed: BrowserRequest =
            serde_json::from_value(serde_json::json!({"action":"snapshot"})).unwrap();
        assert_eq!(parsed.validate(), Ok(()));
    }
}
