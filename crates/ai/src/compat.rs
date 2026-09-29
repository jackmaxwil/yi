//! Request-shape facts derived from a model's family and transport, never read from a catalog
//! flag whose absence silently meant "off" (D306, #749).

use serde_json::Value;
use yi_types::model::{Effort, Model};

use crate::anthropic::Thinking;

/// For the keys that still vary per entry and fail loudly with a 400 when wrong.
pub(crate) fn compat_bool(model: &Model, key: &str, default: bool) -> bool {
    model
        .compat
        .as_ref()
        .and_then(|compat| compat.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

/// OpenRouter nests effort as `reasoning: {effort}`; other completions endpoints send it flat.
pub(crate) fn nested_reasoning(model: &Model) -> bool {
    model.base_url.contains("openrouter.ai")
}

/// OpenAI's own providers take a reasoning model's system text as `developer`; every other route
/// takes `system`, so one model's stable prefix is one byte string whichever catalog named it.
pub(crate) fn developer_role(model: &Model) -> bool {
    model.reasoning && matches!(model.provider.as_str(), "openai" | "openai-codex")
}

/// Whose thinking needs `reasoning_content` on every assistant turn, by id prefix, first match
/// deciding: a family row covers ids the bundle has not seen; the rows above it keep D34's flags.
const REASONING_CONTENT: [(&str, bool); 5] = [
    ("deepseek/deepseek-r1", false),
    ("deepseek/deepseek-chat", false),
    ("deepseek/deepseek-v3", false),
    ("deepseek/", true),
    ("moonshotai/kimi-", true),
];

pub(crate) fn replays_reasoning_content(model: &Model) -> bool {
    let id = model.id.trim_start_matches('~');
    model.reasoning
        && REASONING_CONTENT
            .iter()
            .find(|(prefix, _)| id.starts_with(prefix))
            .is_some_and(|(_, replays)| *replays)
}

/// `claude-opus-4-6` is (4, 6), `claude-opus-4-20250514` (4, 0), `claude-3-7-sonnet` (3, 7).
fn claude_generation(id: &str) -> Option<(u32, u32)> {
    let mut numbers = id
        .trim_start_matches('~')
        .strip_prefix("claude-")?
        .split('-')
        .filter_map(|part| part.parse::<u32>().ok())
        .filter(|number| *number < 100);
    Some((numbers.next()?, numbers.next().unwrap_or(0)))
}

/// A direct Claude thinks adaptively from 4.6 and on a token budget before it; the Messages API
/// refuses the other shape. An id with no generation is newer than this table.
pub fn anthropic_thinking(model: &Model, effort: Effort) -> Thinking {
    match effort {
        Effort::Off => Thinking::Off,
        effort if claude_generation(&model.id).is_none_or(|generation| generation >= (4, 6)) => {
            Thinking::Adaptive {
                effort: Some(effort.to_string()),
            }
        }
        Effort::Minimal | Effort::Low => Thinking::Budget { tokens: 1024 },
        Effort::Medium => Thinking::Budget { tokens: 4096 },
        Effort::High | Effort::XHigh | Effort::Max => Thinking::Budget { tokens: 16384 },
    }
}
