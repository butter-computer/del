//! Authentication. del never reads or stores credentials: it relies on the
//! login Codex itself keeps (`codex login`) and asks the app-server whether an
//! account is active. API-key mode hands the key to Codex via `account/login/start`;
//! Codex then persists it in its own store, exactly as `codex login --with-api-key`.

use crate::protocol::{Account, GetAccountResponse};
use crate::rpc::{ClientError, CodexClient};
use del_agent::AgentError;
use serde_json::json;

/// Environment variables consulted (in order) for API-key mode.
pub const API_KEY_ENV: [&str; 2] = ["CODEX_API_KEY", "OPENAI_API_KEY"];

#[derive(Debug, thiserror::Error)]
pub enum CodexError {
    #[error(
        "the `codex` CLI was not found. Install it (`npm i -g @openai/codex` or `brew install codex`) and run `codex login`"
    )]
    NotInstalled,
    #[error(
        "Codex is not logged in. Run `codex login` in a terminal (ChatGPT sign-in), \
         or set CODEX_API_KEY / OPENAI_API_KEY and restart del"
    )]
    NotLoggedIn,
    #[error("Codex rejected the API key from the environment: {0}")]
    ApiKeyRejected(String),
    #[error(transparent)]
    Client(#[from] ClientError),
}

impl From<CodexError> for AgentError {
    fn from(e: CodexError) -> Self {
        match e {
            CodexError::Client(ClientError::Spawn { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                AgentError::Backend(CodexError::NotInstalled.to_string())
            }
            other => AgentError::Backend(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthStatus {
    ChatGpt {
        email: Option<String>,
        plan: Option<String>,
    },
    ApiKey,
    /// Some other Codex-managed credential (e.g. Bedrock).
    Other,
}

/// API key from the environment, if the user configured one.
pub fn api_key_from_env() -> Option<String> {
    API_KEY_ENV
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()))
}

/// Verify (or establish, in API-key mode) authentication on a connected,
/// initialized client.
pub async fn ensure_authenticated(
    client: &CodexClient,
    api_key: Option<&str>,
) -> Result<AuthStatus, CodexError> {
    let mut resp: GetAccountResponse = client
        .call("account/read", json!({ "refreshToken": false }))
        .await?;
    if resp.account.is_none() {
        let Some(key) = api_key else {
            return Err(if resp.requires_openai_auth {
                CodexError::NotLoggedIn
            } else {
                // Provider needs no OpenAI auth (custom model provider).
                return Ok(AuthStatus::Other);
            });
        };
        client
            .request(
                "account/login/start",
                json!({ "type": "apiKey", "apiKey": key }),
            )
            .await
            .map_err(|e| match e {
                ClientError::Rpc(r) => CodexError::ApiKeyRejected(r.message),
                other => other.into(),
            })?;
        resp = client
            .call("account/read", json!({ "refreshToken": false }))
            .await?;
    }
    match resp.account {
        Some(Account::Chatgpt { email, plan_type }) => Ok(AuthStatus::ChatGpt {
            email,
            plan: plan_type,
        }),
        Some(Account::ApiKey) => Ok(AuthStatus::ApiKey),
        Some(Account::Other) => Ok(AuthStatus::Other),
        None => Err(CodexError::NotLoggedIn),
    }
}
