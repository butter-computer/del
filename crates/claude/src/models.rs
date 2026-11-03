use crate::backend::ClaudeOptions;
use del_agent::{AgentError, EffortOption, ModelInfo, Result};
use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

pub fn parse_commands(response: &Value) -> Vec<del_agent::ComposerCommand> {
    use del_agent::{CommandKind, ComposerCommand};
    response["commands"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let name = row["name"].as_str()?;
            // These controls require the CLI's own terminal UI.
            if row["description"]
                .as_str()
                .is_some_and(|description| description.starts_with("(removed)"))
                || name.starts_with('_')
                || matches!(name, "color" | "focus" | "reload-plugins")
            {
                return None;
            }
            Some(ComposerCommand {
                name: name.into(),
                description: row["description"].as_str().unwrap_or_default().into(),
                argument_hint: if name == "goal" {
                    "<objective> · clear"
                } else {
                    row["argumentHint"].as_str().unwrap_or_default()
                }
                .into(),
                kind: match name {
                    "goal" => CommandKind::Goal,
                    "fast" => CommandKind::Fast,
                    _ if row["builtin"] == true => CommandKind::Action,
                    _ => CommandKind::Skill,
                },
                source: if row["builtin"] == true {
                    "Claude Code"
                } else {
                    "Skill"
                }
                .into(),
                target: None,
            })
        })
        .collect()
}

pub fn parse_models(response: &Value) -> Result<Vec<ModelInfo>> {
    let rows = response["models"]
        .as_array()
        .ok_or_else(|| AgentError::Backend("Claude returned no model catalog".into()))?;
    let mut models: Vec<ModelInfo> = rows
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
                fast_tier: (m["supportsFastMode"] == true).then(|| "fast".into()),
            })
        })
        .collect();
    if let Some(index) = models.iter().position(|m| m.is_default) {
        let configured = response["delConfiguredModel"]
            .as_str()
            .filter(|id| !id.is_empty() && *id != "default");
        let native_default = rows.iter().find(|m| m["value"] == "default");
        let resolved =
            configured.or_else(|| native_default.and_then(|m| m["resolvedModel"].as_str()));
        if let Some(id) = resolved {
            let matched = rows.iter().find(|m| {
                m["value"] != "default" && (m["value"] == id || m["resolvedModel"] == id)
            });
            if let Some(model) = matched
                .and_then(|row| models.iter().find(|m| row["value"] == m.id))
                .cloned()
            {
                models[index] = ModelInfo {
                    id: "default".into(),
                    is_default: true,
                    ..model
                };
            } else {
                models[index].name = id.into();
                if configured.is_some() {
                    models[index].efforts.clear();
                    models[index].default_effort = None;
                    models[index].fast_tier = None;
                }
            }
        } else if models[index].name.to_lowercase().starts_with("default") {
            models[index].name = "Automatic".into();
        }
    }
    Ok(models)
}

pub async fn discover(options: &ClaudeOptions, cwd: &Path) -> Result<Vec<ModelInfo>> {
    parse_models(&control_query(options, cwd, None, true).await?)
}

pub async fn discover_catalog(options: &ClaudeOptions, cwd: &Path) -> Result<Value> {
    control_query(options, cwd, None, false).await
}

/// Read native plan limits without submitting a prompt or saving a session.
pub async fn usage(options: &ClaudeOptions, cwd: &Path) -> Result<Value> {
    control_query(
        options,
        cwd,
        Some(json!({"subtype":"get_usage","skip_behaviors":true})),
        false,
    )
    .await
}

async fn control_query(
    options: &ClaudeOptions,
    cwd: &Path,
    followup: Option<Value>,
    include_settings: bool,
) -> Result<Value> {
    let mut cmd = Command::new(&options.binary);
    options.prepare(&mut cmd);
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
    del_agent::environment::prepare_provider_command(cmd.as_std_mut());
    let mut child = cmd.spawn()?;
    let pid = child.id();
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let result = tokio::time::timeout(Duration::from_secs(20), async {
        let request = json!({"type":"control_request","request_id":"del-models","request":{"subtype":"initialize"}});
        stdin.write_all(format!("{request}\n").as_bytes()).await?;
        stdin.flush().await?;
        let mut followup = followup;
        let mut catalog: Option<Value> = None;
        while let Some(line) = lines.next_line().await? {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue; };
            if v["type"] == "control_response" && v["response"]["request_id"] == "del-models" {
                if let Some(mut catalog) = catalog.take() {
                    // Only retain the model setting, never settings containing credentials.
                    let body = &v["response"]["response"];
                    let configured = std::env::var("ANTHROPIC_MODEL").ok().filter(|s| !s.is_empty())
                        .or_else(|| body["effective"]["env"]["ANTHROPIC_MODEL"].as_str().map(str::to_owned))
                        .or_else(|| body["effective"]["model"].as_str().map(str::to_owned));
                    catalog["delConfiguredModel"] = json!(configured);
                    // Resolve effort before the picker opens, including model-specific settings.
                    let env_effort = std::env::var("CLAUDE_CODE_EFFORT_LEVEL").ok().filter(|s| !s.is_empty());
                    if let Some(models) = catalog["models"].as_array_mut() {
                        for model in models {
                            let id = model["resolvedModel"].as_str().or(model["value"].as_str()).unwrap_or_default();
                            let effort = env_effort.as_deref()
                                .or_else(|| body["effective"]["modelSettings"][id]["effortLevel"].as_str())
                                .or_else(|| body["effective"]["effortLevel"].as_str());
                            if let Some(effort) = effort
                                && model["supportedEffortLevels"].as_array().is_some_and(|levels| levels.iter().any(|level| level == effort))
                            {
                                model["defaultEffort"] = json!(effort);
                            }
                        }
                    }
                    return Ok(catalog);
                }
                if v["response"]["subtype"] == "error" {
                    return Err(AgentError::Backend(v["response"]["error"].as_str().unwrap_or("Claude model discovery failed").into()));
                }
                if let Some(request) = followup.take() {
                    let request = json!({"type":"control_request","request_id":"del-models","request":request});
                    stdin.write_all(format!("{request}\n").as_bytes()).await?;
                    stdin.flush().await?;
                    continue;
                }
                if include_settings {
                    catalog = Some(v["response"]["response"].clone());
                    let request = json!({"type":"control_request","request_id":"del-models","request":{"subtype":"get_settings"}});
                    stdin.write_all(format!("{request}\n").as_bytes()).await?;
                    stdin.flush().await?;
                    continue;
                }
                return Ok(v["response"]["response"].clone());
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
    fn default_name_resolves_native_ids_and_configured_aliases_without_pinning_selection() {
        let mut catalog = json!({"models":[
            {"value":"default","displayName":"Default (recommended)","resolvedModel":"claude-a"},
            {"value":"opus","displayName":"Opus A","resolvedModel":"claude-a","supportsFastMode":true},
            {"value":"sonnet","displayName":"Sonnet B","resolvedModel":"claude-b","supportsEffort":true,"supportedEffortLevels":["high"]}
        ]});
        let default = |value: &Value| {
            parse_models(value)
                .unwrap()
                .into_iter()
                .find(|m| m.is_default)
                .unwrap()
        };
        assert_eq!(default(&catalog).name, "Opus A");
        catalog["delConfiguredModel"] = json!("sonnet");
        let model = default(&catalog);
        assert_eq!(model.id, "default");
        assert_eq!(model.name, "Sonnet B");
        assert!(model.supports_effort("high"));
        assert!(model.fast_tier.is_none());
        catalog["delConfiguredModel"] = json!("private-model");
        let model = default(&catalog);
        assert_eq!(model.name, "private-model");
        assert!(model.efforts.is_empty() && model.fast_tier.is_none());
    }

    #[test]
    fn unsupported_models_do_not_inherit_another_models_effort() {
        let models = parse_models(&json!({"models":[
            {"value":"default","displayName":"Recommended","supportsEffort":true,"supportedEffortLevels":["low","max"],"supportsFastMode":true},
            {"value":"fast","displayName":"Fast"}
        ]})).unwrap();
        assert!(models[0].is_default);
        assert!(models[0].supports_effort("max"));
        assert!(models[1].efforts.is_empty());
        assert_eq!(models[0].fast_tier.as_deref(), Some("fast"));
        assert!(models[1].fast_tier.is_none());
    }

    #[test]
    fn missing_catalogs_are_errors_and_odd_rows_are_skipped() {
        assert!(parse_models(&json!({})).is_err());
        assert!(parse_models(&json!({"models":{}})).is_err());
        assert!(
            parse_models(&json!({"models":[1, null, {"displayName":"no value"}]}))
                .unwrap()
                .is_empty()
        );
        // Effort levels are listed only when the model says it supports effort.
        let models = parse_models(&json!({"models":[
            {"value":"m","supportsEffort":false,"supportedEffortLevels":["high"],"defaultEffort":"high"}
        ]}))
        .unwrap();
        assert!(models[0].efforts.is_empty());
        assert_eq!(models[0].default_effort.as_deref(), Some("high"));
        assert!(!models[0].is_default);
        assert!(parse_commands(&json!({})).is_empty());
        assert!(parse_commands(&json!({"commands":[{"description":"no name"}]})).is_empty());
    }
}
