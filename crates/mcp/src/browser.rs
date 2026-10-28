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
