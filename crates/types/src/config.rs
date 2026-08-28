use serde::{Deserialize, Serialize};

/// Design §12 model roles. Each role names a `provider/id`; an unset role
/// falls back to the primary model, so a config that names nothing behaves
/// exactly as one model for everything.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRoles {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summarizer: Option<String>,
    /// Naming this role is what turns the LLM reviewer on (D28/D50: the
    /// advisor observes and says nothing until a model role names it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub advisor: Option<String>,
}
