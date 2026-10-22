use del_agent::{AgentError, EffortOption, ModelInfo, Result};
use serde_json::Value;

pub fn parse_models(page: &Value) -> Result<Vec<ModelInfo>> {
    let rows = page["data"]
        .as_array()
        .ok_or_else(|| AgentError::Backend("model/list returned no model list".into()))?;
    Ok(rows
        .iter()
        .filter(|m| m["hidden"] != true)
        .filter_map(|m| {
            Some(ModelInfo {
                id: m["model"].as_str()?.into(),
                name: m["displayName"].as_str().or(m["model"].as_str())?.into(),
                description: m["description"].as_str().unwrap_or_default().into(),
                efforts: m["supportedReasoningEfforts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e["reasoningEffort"].as_str())
                    .map(EffortOption::named)
                    .collect(),
                default_effort: m["defaultReasoningEffort"].as_str().map(str::to_owned),
                fast_tier: m["serviceTiers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|tier| tier["id"] == "priority")
                    .and_then(|tier| tier["id"].as_str())
                    .map(str::to_owned),
                is_default: m["isDefault"] == true,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn model_catalog_preserves_ids_order_defaults_and_filters_hidden() {
        let models = parse_models(&json!({"data":[
            {"model":"model-new","displayName":"New Model","isDefault":true,"defaultReasoningEffort":"medium",
             "supportedReasoningEfforts":[{"reasoningEffort":"medium"},{"reasoningEffort":"ultra"}],
             "serviceTiers":[{"id":"priority","name":"Fast"}]},
            {"model":"hidden","hidden":true}
        ]})).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "model-new");
        assert_eq!(models[0].efforts[1].id, "ultra");
        assert_eq!(models[0].default_effort.as_deref(), Some("medium"));
        assert!(models[0].is_default);
        assert_eq!(models[0].fast_tier.as_deref(), Some("priority"));
    }
}
