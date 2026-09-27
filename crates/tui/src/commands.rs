/// Invariant: the popup offers exactly what [`crate::input::handle_slash`] routes.
#[rustfmt::skip]
pub(crate) const SLASH_COMMANDS: [&str; 18] = [
    "new", "undo", "quit", "tree", "editor", "advisor", "plan", "plantree", "goal", "agents",
    "model", "permissions", "compact", "sessions", "lanes", "land", "discard", "heartbeat",
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
