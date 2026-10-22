//! Provider-advertised model choices; identifiers are passed through unchanged.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffortOption {
    pub id: String,
    pub name: String,
}

impl EffortOption {
    pub fn named(id: impl Into<String>) -> Self {
        let id = id.into();
        let name = match id.as_str() {
            "xhigh" => "Extra high".into(),
            "max" => "Maximum".into(),
            _ => {
                let mut chars = id.chars();
                chars
                    .next()
                    .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
                    .unwrap_or_default()
            }
        };
        Self { id, name }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub efforts: Vec<EffortOption>,
    pub default_effort: Option<String>,
    #[serde(default)]
    pub fast_tier: Option<String>,
    pub is_default: bool,
}

impl ModelInfo {
    pub fn supports_effort(&self, effort: &str) -> bool {
        self.efforts.iter().any(|e| e.id == effort)
    }
}
