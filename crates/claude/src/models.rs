use crate::backend::ClaudeOptions;
use del_agent::{AgentError, EffortOption, ModelInfo, Result};
use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

pub fn parse_models(response: &Value) -> Result<Vec<ModelInfo>> {
    let rows = response["models"]
        .as_array()
        .ok_or_else(|| AgentError::Backend("Claude returned no model catalog".into()))?;
    Ok(rows
        .iter()
        .filter_map(|m| {
            Some(ModelInfo {
                id: m["value"].as_str()?.into(),
                name: m["displayName"].as_str().or(m["value"].as_str())?.into(),
                description: m["description"].as_str().unwrap_or_default().into(),
                efforts: if m["supportsEffort"] == true {
                    m["supportedEffortLevels"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(EffortOption::named)
                        .collect()
                } else {
                    vec![]
                },
                default_effort: m["defaultEffort"].as_str().map(str::to_owned),
                is_default: m["value"] == "default",
                fast_tier: None,
            })
        })
        .collect())
}

pub async fn discover(options: &ClaudeOptions, cwd: &Path) -> Result<Vec<ModelInfo>> {
    let mut cmd = Command::new(&options.binary);
    cmd.args([
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--no-session-persistence",
    ])
    .current_dir(cwd)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .kill_on_drop(true)
    .process_group(0);
    if let Some(s) = &options.setting_sources {
        cmd.args(["--setting-sources", s]);
    }
    del_agent::environment::clear_worker_session_env(cmd.as_std_mut());
    let mut child = cmd.spawn()?;
    let pid = child.id();
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let result = tokio::time::timeout(Duration::from_secs(20), async {
        let request = json!({"type":"control_request","request_id":"del-models","request":{"subtype":"initialize"}});
        stdin.write_all(format!("{request}\n").as_bytes()).await?;
        stdin.flush().await?;
        while let Some(line) = lines.next_line().await? {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue; };
            if v["type"] == "control_response" && v["response"]["request_id"] == "del-models" {
                if v["response"]["subtype"] == "error" {
                    return Err(AgentError::Backend(v["response"]["error"].as_str().unwrap_or("Claude model discovery failed").into()));
                }
                return parse_models(&v["response"]["response"]);
            }
        }
        Err(AgentError::Backend("Claude exited before returning models".into()))
    }).await.unwrap_or_else(|_| Err(AgentError::Backend("Claude model discovery timed out".into())));
    if let Some(pid) = pid {
        del_agent::procgroup::terminate_group(pid, &mut child, Duration::from_millis(200)).await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsupported_models_do_not_inherit_another_models_effort() {
        let models = parse_models(&json!({"models":[
            {"value":"default","displayName":"Recommended","supportsEffort":true,"supportedEffortLevels":["low","max"]},
            {"value":"fast","displayName":"Fast"}
        ]})).unwrap();
        assert!(models[0].is_default);
        assert!(models[0].supports_effort("max"));
        assert!(models[1].efforts.is_empty());
    }
}
