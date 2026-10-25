//! Ready-made launch config pointing an agent CLI at del's MCP server.
//!
//! Nothing here touches the user's global config: Claude Code gets a private
//! `--mcp-config` file (mode 0600, because it holds the bearer token) and Codex
//! gets `-c` overrides plus the token in an environment variable, so the token
//! never appears in argv.

use crate::server::Credentials;
use serde_json::json;
use std::io;
use std::path::{Path, PathBuf};

/// Name the server is registered under in the agent (tools appear as `del`'s).
pub const SERVER_NAME: &str = "del";
pub const ENV_URL: &str = "DEL_MCP_URL";
pub const ENV_TOKEN: &str = "DEL_MCP_TOKEN";

/// Allow the server's default 30-minute user wait to settle before the client
/// aborts. Scoped to del only: no process-wide timeout/env overrides.
pub const TOOL_TIMEOUT_SECS: u64 = 31 * 60;

/// Extra argv and environment to launch an agent CLI with.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct LaunchConfig {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// File created for this launch; delete it when the session ends.
    pub files: Vec<PathBuf>,
}

impl std::fmt::Debug for LaunchConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaunchConfig")
            .field("args", &self.args)
            .field(
                "env_keys",
                &self.env.iter().map(|(key, _)| key).collect::<Vec<_>>(),
            )
            .field("files", &self.files)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transport {
    /// Native streamable HTTP with a bearer token.
    Http,
    /// Spawn this `del-mcp-stdio` binary, which proxies to the HTTP server.
    Stdio { binary: PathBuf },
}

fn file_name(c: &Credentials) -> io::Result<String> {
    let safe: String = c
        .session_id
        .as_str()
        .chars()
        .take(64)
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let mut nonce = [0; 16];
    getrandom::fill(&mut nonce).map_err(|e| io::Error::other(e.to_string()))?;
    let nonce: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("del-mcp-{safe}-{nonce}.json"))
}

fn write_private(path: &Path, content: &str) -> io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)?.write_all(content.as_bytes())
}

/// Claude Code: `claude <args>`. Writes the JSON config into `dir`. Other MCP
/// servers the user has configured keep working (no `--strict-mcp-config`).
pub fn claude(c: &Credentials, transport: &Transport, dir: &Path) -> io::Result<LaunchConfig> {
    let server = match transport {
        Transport::Http => json!({
            "type": "http",
            "url": c.url,
            "headers": {"Authorization": format!("Bearer {}", c.token)},
            "timeout": TOOL_TIMEOUT_SECS * 1_000,
        }),
        Transport::Stdio { binary } => json!({
            "type": "stdio",
            "command": binary,
            "args": [],
            "env": {ENV_URL: c.url, ENV_TOKEN: c.token},
            "timeout": TOOL_TIMEOUT_SECS * 1_000,
        }),
    };
    let path = dir.join(file_name(c)?);
    let doc = json!({"mcpServers": {SERVER_NAME: server}});
    write_private(&path, &serde_json::to_string_pretty(&doc).expect("json"))?;
    Ok(LaunchConfig {
        args: vec![
            "--mcp-config".into(),
            path.to_string_lossy().into_owned(),
            "--append-system-prompt".into(),
            del_agent::backend::BROWSER_INSTRUCTIONS.into(),
        ],
        env: vec![],
        files: vec![path],
    })
}

/// A TOML basic string literal for `-c key=value`.
fn toml_str(s: &str) -> String {
    let mut out = String::from("\"");
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Codex: `codex <args>` with `env` added to the child's environment.
pub fn codex(c: &Credentials, transport: &Transport) -> LaunchConfig {
    let k = |field: &str| format!("mcp_servers.{SERVER_NAME}.{field}");
    let mut args = Vec::new();
    let mut set = |key: String, value: String| {
        args.push("-c".to_string());
        args.push(format!("{key}={value}"));
    };
    match transport {
        Transport::Http => {
            set(k("url"), toml_str(&c.url));
            set(k("bearer_token_env_var"), toml_str(ENV_TOKEN));
        }
        Transport::Stdio { binary } => {
            set(k("command"), toml_str(&binary.to_string_lossy()));
            set(k("args"), "[]".into());
            set(
                k("env_vars"),
                format!("[{}, {}]", toml_str(ENV_URL), toml_str(ENV_TOKEN)),
            );
        }
    }
    set(k("tool_timeout_sec"), TOOL_TIMEOUT_SECS.to_string());
    LaunchConfig {
        args,
        env: vec![
            (ENV_URL.into(), c.url.clone()),
            (ENV_TOKEN.into(), c.token.clone()),
        ],
        files: vec![],
    }
}

/// OpenCode merges launch-local JSON with the user's configuration. Secrets
/// remain in the child environment and never enter argv or saved terminal history.
pub fn opencode(c: &Credentials) -> LaunchConfig {
    LaunchConfig {
        args: vec![],
        env: vec![(
            "OPENCODE_CONFIG_CONTENT".into(),
            json!({"mcp":{"del":{
                "type":"remote", "url":c.url, "oauth":false,
                "headers":{"Authorization":format!("Bearer {}",c.token)},
                "timeout":{"execution":TOOL_TIMEOUT_SECS * 1000}
            }}})
            .to_string(),
        )],
        files: vec![],
    }
}
