use del_agent::{EffortOption, ModelInfo};
use serde_json::Value;

pub fn option<'a>(response: &'a Value, category: &str) -> Option<&'a Value> {
    response["configOptions"]
        .as_array()?
        .iter()
        .find(|o| o["category"] == category && o["type"] == "select")
}

pub fn choices(option: &Value) -> Vec<&Value> {
    option["options"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|o| {
            o["options"]
                .as_array()
                .map(|group| group.iter().collect::<Vec<_>>())
                .unwrap_or_else(|| vec![o])
        })
        .collect()
}

pub fn catalog(agent: &str, name: &str, response: &Value) -> Vec<ModelInfo> {
    let effort = option(response, "thought_level");
    let efforts: Vec<_> = effort
        .into_iter()
        .flat_map(choices)
        .filter_map(|e| {
            Some(EffortOption {
                id: e["value"].as_str()?.into(),
                name: e["name"].as_str().or(e["value"].as_str())?.into(),
            })
        })
        .collect();
    let default_effort = effort
        .and_then(|e| e["currentValue"].as_str())
        .map(str::to_owned);
    let mut models = vec![ModelInfo {
        id: agent.into(),
        name: name.into(),
        description: "Agent default model".into(),
        efforts,
        default_effort,
        is_default: false,
        fast_tier: None,
    }];
    // Advertised effort options belong to the default session model. Other
    // models may expose different config options, so do not copy those levels.
    if let Some(option) = option(response, "model") {
        for m in choices(option) {
            if let Some(id) = m["value"].as_str() {
                models.push(ModelInfo {
                    id: format!("{agent}/{id}"),
                    name: m["name"].as_str().unwrap_or(id).into(),
                    description: name.into(),
                    efforts: vec![],
                    default_effort: None,
                    is_default: false,
                    fast_tier: None,
                });
            }
        }
    } else {
        for m in response["models"]["availableModels"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if let Some(id) = m["modelId"].as_str() {
                models.push(ModelInfo {
                    id: format!("{agent}/{id}"),
                    name: m["name"].as_str().unwrap_or(id).into(),
                    description: name.into(),
                    efforts: vec![],
                    default_effort: None,
                    is_default: false,
                    fast_tier: None,
                });
            }
        }
    }
    models
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn grouped_options_keep_wire_ids_and_efforts_are_not_guessed() {
        let models = catalog(
            "agent",
            "Agent",
            &json!({"configOptions":[
                {"id":"thinking","category":"thought_level","type":"select","currentValue":"deep","options":[{"value":"deep","name":"Deep"}]},
                {"id":"model","category":"model","type":"select","options":[{"group":"models","options":[{"value":"v2","name":"Version 2"}]}]}
            ]}),
        );
        assert_eq!(models[0].efforts[0].id, "deep");
        assert_eq!(models[1].id, "agent/v2");
        assert!(models[1].efforts.is_empty());
    }
}
