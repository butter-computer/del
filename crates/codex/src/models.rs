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

pub fn apply_configured_default(models: &mut Vec<ModelInfo>, response: &Value) {
    if let Some(effort) = response["config"]["model_reasoning_effort"].as_str() {
        for model in models
            .iter_mut()
            .filter(|model| model.supports_effort(effort))
        {
            model.default_effort = Some(effort.into());
        }
    }
    let Some(id) = response["config"]["model"]
        .as_str()
        .filter(|id| !id.is_empty())
    else {
        return;
    };
    for model in models.iter_mut() {
        model.is_default = model.id == id;
    }
    if !models.iter().any(|model| model.is_default) {
        models.push(ModelInfo {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            efforts: vec![],
            default_effort: None,
            fast_tier: None,
            is_default: true,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn configured_default_overrides_recommendation_and_preserves_custom_ids() {
        let mut models = parse_models(&json!({"data":[
            {"model":"recommended","displayName":"Recommended","isDefault":true},
            {"model":"configured","displayName":"Configured Model"}
        ]}))
        .unwrap();
        apply_configured_default(&mut models, &json!({"config":{}}));
        assert!(models[0].is_default);
        apply_configured_default(&mut models, &json!({"config":{"model":"configured"}}));
        assert!(!models[0].is_default && models[1].is_default);
        apply_configured_default(&mut models, &json!({"config":{"model":"custom-id"}}));
        let defaults: Vec<_> = models.iter().filter(|m| m.is_default).collect();
        assert_eq!(defaults.len(), 1);
        assert_eq!(defaults[0].name, "custom-id");
        assert!(defaults[0].efforts.is_empty());
    }
    #[test]
    fn configured_effort_overrides_supported_defaults_only() {
        let mut models = parse_models(&json!({"data":[
            {"model":"a","defaultReasoningEffort":"medium",
             "supportedReasoningEfforts":[{"reasoningEffort":"medium"},{"reasoningEffort":"high"}]},
            {"model":"b","defaultReasoningEffort":"low",
             "supportedReasoningEfforts":[{"reasoningEffort":"low"}]}
        ]}))
        .unwrap();
        apply_configured_default(
            &mut models,
            &json!({"config":{"model_reasoning_effort":"high"}}),
        );
        assert_eq!(models[0].default_effort.as_deref(), Some("high"));
        assert_eq!(models[1].default_effort.as_deref(), Some("low"));
    }
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
