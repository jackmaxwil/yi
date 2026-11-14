/// Invariant: the list offers what [`crate::input::handle_slash`] routes, plus `learn`, a prompt.
pub(crate) const SLASH_COMMANDS: [(&str, &str); 23] = [
    ("new", "start a session"),
    ("undo", "restore the last turn's files"),
    ("quit", "leave the session"),
    ("tree", "open the entry tree"),
    ("editor", "edit the draft in an editor"),
    ("advisor", "ask the advisor"),
    ("plan", "open a plan"),
    ("plantree", "show the plan tree"),
    ("goal", "set the session goal"),
    ("agents", "show the agents"),
    ("mcp", "manage MCP servers"),
    ("model", "choose a model"),
    ("permissions", "show the permission mode"),
    ("compact", "compact the session"),
    ("sessions", "list sessions"),
    ("lanes", "show the lanes"),
    ("land", "land the lane"),
    ("discard", "discard the lane"),
    ("heartbeat", "show the heartbeat"),
    ("todo", "show the todo list"),
    ("theme", "choose a theme"),
    ("learn", "read a skill"),
    ("queue", "run a message after this turn"),
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
                .all(|v| SLASH_COMMANDS.iter().any(|(name, _)| name == v))
        );
    }

    #[test]
    fn mcp_slash_command_is_offered_with_its_slash() {
        assert!(
            SLASH_COMMANDS
                .iter()
                .any(|(name, _)| format!("/{name}") == "/mcp")
        );
    }
}
