//! Request-shape facts derived from a model's family and transport, never read from a catalog
//! flag whose absence silently meant "off" (D306, #749).

use serde_json::Value;
use yi_types::model::Model;

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
/// deciding: a family row covers ids the bundle has not seen; the rows above it predate it.
const REASONING_CONTENT: [(&str, bool); 5] = [
    ("deepseek/deepseek-r1", false),
    ("deepseek/deepseek-chat", false),
    ("deepseek/deepseek-v3", false),
    ("deepseek/", true),
    ("moonshotai/kimi-k2.6", true),
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
        .strip_prefix("claude-")?
        .split('-')
        .filter_map(|part| part.parse::<u32>().ok())
        .filter(|number| *number < 100);
    Some((numbers.next()?, numbers.next().unwrap_or(0)))
}

/// Claude takes adaptive thinking from 4.6 on and a token budget before it; the direct
/// Messages API refuses the other shape. An id with no generation is newer than this table.
pub fn adaptive_thinking(model: &Model) -> bool {
    claude_generation(&model.id).is_none_or(|generation| generation >= (4, 6))
}
