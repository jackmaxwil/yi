//! A question to the user with a few light answers to pick from (plan section 6.3).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::ids::TodoLabel;
use crate::url::Url;

pub const OPTIONS_MIN: usize = 3;
pub const OPTIONS_MAX: usize = 5;
pub const OPTION_ID_MAX: usize = 32;
/// A preview is light: a line of text, a small diagram's source, or an address to the rest.
pub const PREVIEW_MAX_BYTES: usize = 2048;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct OptionId(String);

impl OptionId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for OptionId {
    type Error = AskError;

    fn try_from(id: String) -> Result<Self, Self::Error> {
        let chars = id.chars().count();
        if chars == 0 || chars > OPTION_ID_MAX || id.contains(char::is_whitespace) {
            return Err(AskError::BadId { id });
        }
        Ok(Self(id))
    }
}

impl From<OptionId> for String {
    fn from(id: OptionId) -> Self {
        id.0
    }
}

impl std::fmt::Display for OptionId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// `preview` is short text, a diagram's source, or an address such as `store://` or `user://`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AskOption {
    pub id: OptionId,
    pub label: TodoLabel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The user's reply to an [`Ask`]: `option` is the pick, the exemplar, and every other option is
/// a rejected one; `None` is a reply in the user's own words that named no option.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    pub address: Url,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option: Option<OptionId>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `after` is the newest user message when it was asked, so only a later one answers it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ask {
    pub options: Vec<AskOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Url>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<Answer>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Ask {
    pub fn new(options: Vec<AskOption>) -> Result<Self, AskError> {
        if !(OPTIONS_MIN..=OPTIONS_MAX).contains(&options.len()) {
            return Err(AskError::Count { got: options.len() });
        }
        for (at, option) in options.iter().enumerate() {
            if options.iter().take(at).any(|seen| seen.id == option.id) {
                return Err(AskError::DuplicateId {
                    id: option.id.to_string(),
                });
            }
            let bytes = option.preview.as_ref().map_or(0, String::len);
            if bytes > PREVIEW_MAX_BYTES {
                return Err(AskError::PreviewTooLong {
                    id: option.id.to_string(),
                    bytes,
                });
            }
        }
        Ok(Self {
            options,
            after: None,
            answer: None,
            extra: Map::new(),
        })
    }

    pub fn picked(&self) -> Option<&AskOption> {
        let id = self.answer.as_ref()?.option.as_ref()?;
        self.options.iter().find(|option| &option.id == id)
    }
}

/// `1. Calm · 2. Bold · 3. Dense`: the numbers a reply may pick by.
impl std::fmt::Display for Ask {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (at, option) in self.options.iter().enumerate() {
            let gap = if at == 0 { "" } else { " · " };
            write!(formatter, "{gap}{}. {}", at.saturating_add(1), option.label)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AskError {
    #[error("a question to the user offers {OPTIONS_MIN} to {OPTIONS_MAX} options, not {got}")]
    Count { got: usize },
    #[error("option id {id:?} appears twice; every option's id is unique")]
    DuplicateId { id: String },
    #[error(
        "option {id:?} carries a {bytes}-byte preview; the cap is {PREVIEW_MAX_BYTES} bytes, so keep it light: a line, a small diagram, or an address to the rest"
    )]
    PreviewTooLong { id: String, bytes: usize },
    #[error("option id {id:?} is 1 to {OPTION_ID_MAX} characters without whitespace")]
    BadId { id: String },
    #[error("options are a question to the user, so a todo carrying them blocks on user")]
    NotOnUser,
}
