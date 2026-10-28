//! The ACP agent registry: which agents del can launch, and which are installed.
//!
//! Agents are a plain list of `{id, name, command, args, env}`. Built-in
//! defaults cover the ACP-speaking agents known to exist; the user's own list
//! (`<data dir>/acp-agents.json`) is merged on top, replacing a default with the
//! same `id`. Detection only looks for the executable; nothing is launched, and
//! nothing is downloaded (an `npx` adapter is listed in `install_hint`, never
//! run implicitly).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use del_agent::detect::find_binary;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpAgentConfig {
    /// Stable slug; this is what `SessionConfig::model` selects (`"<id>"` or `"<id>/<model>"`).
    pub id: String,
    pub name: String,
    /// Executable name (looked up like the other CLIs) or an absolute path.
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// How to get the agent when it is not installed; shown, never executed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_hint: Option<String>,
}

impl AcpAgentConfig {
    fn builtin(id: &str, name: &str, command: &str, args: &[&str], hint: &str) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            command: command.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            env: BTreeMap::new(),
            install_hint: Some(hint.into()),
        }
    }

    /// Absolute path of the executable if it is installed.
    pub fn resolve(&self) -> Option<PathBuf> {
        let c = Path::new(&self.command);
        if c.is_absolute() {
            return is_executable(c).then(|| c.to_path_buf());
        }
        find_binary(&self.command)
    }
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// The agents del knows about out of the box.
pub fn default_agents() -> Vec<AcpAgentConfig> {
    vec![
        AcpAgentConfig::builtin(
            "claude-acp",
            "Claude Code (ACP)",
            "claude-agent-acp",
            &[],
            "npm i -g @agentclientprotocol/claude-agent-acp",
        ),
        AcpAgentConfig::builtin(
            "claude-code-acp",
            "Claude Code (ACP, legacy name)",
            "claude-code-acp",
            &[],
            "npm i -g @zed-industries/claude-code-acp",
        ),
        AcpAgentConfig::builtin(
            "codex-acp",
            "Codex (ACP)",
            "codex-acp",
            &[],
            "npm i -g @agentclientprotocol/codex-acp",
        ),
        AcpAgentConfig::builtin(
            "gemini",
            "Gemini CLI",
            "gemini",
            &["--acp"],
            "npm i -g @google/gemini-cli",
        ),
        AcpAgentConfig::builtin("pi-acp", "Pi (ACP)", "pi-acp", &[], "npm i -g pi-acp"),
        AcpAgentConfig::builtin(
            "dsh",
            "DeepSeek harness",
            "dsh",
            &["acp"],
            "build deepseek-harness and put `dsh` on PATH",
        ),
    ]
}

/// Defaults overlaid with the user's list; entries with an existing `id` replace it.
pub fn merge(defaults: Vec<AcpAgentConfig>, user: Vec<AcpAgentConfig>) -> Vec<AcpAgentConfig> {
    let mut out = defaults;
    for u in user {
        match out.iter_mut().find(|a| a.id == u.id) {
            Some(slot) => *slot = u,
            None => out.push(u),
        }
    }
    out
}

#[derive(Deserialize)]
struct ConfigFile {
    #[serde(default)]
    agents: Vec<AcpAgentConfig>,
}

pub fn config_path() -> Option<PathBuf> {
    del_agent::data_dir().map(|d| d.join("acp-agents.json"))
}

/// Parse a user config document: `{"agents": [...]}`.
pub fn parse_config(text: &str) -> Result<Vec<AcpAgentConfig>, serde_json::Error> {
    let agents = serde_json::from_str::<ConfigFile>(text)?.agents;
    if agents
        .iter()
        .any(|a| a.id.is_empty() || a.id.contains('/') || a.command.is_empty())
    {
        return Err(serde::de::Error::custom(
            "ACP agents require a nonempty id without '/' and a command",
        ));
    }
    Ok(agents)
}

/// Defaults plus `<data dir>/acp-agents.json`. A missing file is fine; an
/// unreadable one is reported by the second return value and ignored.
pub fn load() -> (Vec<AcpAgentConfig>, Option<String>) {
    let user = config_path().and_then(|p| match std::fs::read_to_string(&p) {
        Ok(t) => Some(parse_config(&t).map_err(|e| format!("{}: {e}", p.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => Some(Err(format!("{}: {e}", p.display()))),
    });
    match user {
        Some(Ok(u)) => (merge(default_agents(), u), None),
        Some(Err(e)) => (default_agents(), Some(e)),
        None => (default_agents(), None),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentStatus {
    pub agent: AcpAgentConfig,
    /// Resolved executable, `None` when not installed.
    pub path: Option<PathBuf>,
}

impl AgentStatus {
    pub fn installed(&self) -> bool {
        self.path.is_some()
    }
}

/// Status of every registry agent.
pub fn detect(agents: &[AcpAgentConfig]) -> Vec<AgentStatus> {
    agents
        .iter()
        .map(|a| AgentStatus {
            path: a.resolve(),
            agent: a.clone(),
        })
        .collect()
}

/// Ids of installed agents, registry order. Used by the model picker.
pub fn installed_ids() -> Vec<String> {
    let (agents, _) = load();
    detect(&agents)
        .into_iter()
        .filter(AgentStatus::installed)
        .map(|s| s.agent.id)
        .collect()
}

/// Split a `SessionConfig::model` into `(agent id, model)`.
pub fn split_model(model: Option<&str>) -> (Option<&str>, Option<&str>) {
    match model {
        None | Some("") => (None, None),
        Some(m) => match m.split_once('/') {
            Some((a, rest)) => (Some(a), (!rest.is_empty()).then_some(rest)),
            None => (Some(m), None),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_entries_replace_defaults_by_id_and_extend() {
        let user = parse_config(
            r#"{"agents":[
                {"id":"gemini","name":"G","command":"/x/gemini","args":["--experimental-acp"],"env":{"K":"V"}},
                {"id":"mine","name":"Mine","command":"mine"}
            ]}"#,
        )
        .unwrap();
        let all = merge(default_agents(), user);
        let g = all.iter().find(|a| a.id == "gemini").unwrap();
        assert_eq!(g.command, "/x/gemini");
        assert_eq!(g.args, ["--experimental-acp"]);
        assert_eq!(g.env["K"], "V");
        assert_eq!(all.iter().filter(|a| a.id == "gemini").count(), 1);
        assert_eq!(all.last().unwrap().id, "mine");
    }

    #[test]
    fn defaults_have_unique_ids_and_no_slash() {
        let d = default_agents();
        let mut ids: Vec<_> = d.iter().map(|a| a.id.clone()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), d.len());
        assert!(d.iter().all(|a| !a.id.contains('/')));
    }

    #[test]
    fn config_rejects_ids_that_cannot_be_selected() {
        for id in ["", "agent/model"] {
            assert!(
                parse_config(
                    &serde_json::json!({"agents":[{"id":id,"name":"Test","command":"test"}]})
                        .to_string()
                )
                .is_err()
            );
        }
    }

    #[test]
    fn model_string_splits_agent_and_model() {
        assert_eq!(split_model(None), (None, None));
        assert_eq!(split_model(Some("gemini")), (Some("gemini"), None));
        assert_eq!(
            split_model(Some("gemini/gemini-2.5-pro")),
            (Some("gemini"), Some("gemini-2.5-pro"))
        );
        assert_eq!(split_model(Some("a/b/c")), (Some("a"), Some("b/c")));
    }

    #[test]
    fn absolute_command_resolves_only_when_executable() {
        let mut a = default_agents().remove(0);
        a.command = "/definitely/not/here".into();
        assert!(a.resolve().is_none());
        a.command = "/bin/sh".into();
        assert!(a.resolve().is_some());
    }
}
