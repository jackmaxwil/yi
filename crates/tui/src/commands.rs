/// Invariant: the popup offers what [`crate::input::handle_slash`] routes, plus `learn`, a prompt.
#[rustfmt::skip]
pub(crate) const SLASH_COMMANDS: [&str; 21] = [
    "new", "undo", "quit", "tree", "editor", "advisor", "plan", "plantree", "goal", "agents",
    "model", "permissions", "compact", "sessions", "lanes", "land", "discard", "heartbeat", "todo",
    "theme", "learn",
];

#[cfg(test)]
mod tests {
    use super::SLASH_COMMANDS;

    #[test]
    fn slash_table_covers_every_runtime_verb() {
        assert!(
            yi_runtime::slash::SESSION_VERBS
                .iter()
                .chain(std::iter::once(&"sessions"))
                .all(|v| SLASH_COMMANDS.contains(v))
        );
    }
}
