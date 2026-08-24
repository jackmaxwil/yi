use crate::account::Tokens;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub enabled: bool,
    pub reserve_tokens: Tokens,
    pub keep_recent_tokens: Tokens,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            reserve_tokens: Tokens(16_384),
            keep_recent_tokens: Tokens(20_000),
        }
    }
}

pub fn should_compact(context_tokens: Tokens, context_window: Tokens, settings: &Settings) -> bool {
    if !settings.enabled || context_window.0 == 0 {
        return false;
    }
    context_tokens > context_window.saturating_sub(settings.reserve_tokens)
}
