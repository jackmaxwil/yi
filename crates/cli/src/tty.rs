//! The terminal's permission prompt: one question on stderr, one keystroke back.

pub(crate) fn tty_ask(ask: &yi_runtime::PermissionAsk<'_>) -> yi_runtime::AskOutcome {
    use std::io::Write;
    eprintln!("\n{}\n{}", ask.title, ask.text());
    for (index, grant) in ask.grants.iter().enumerate() {
        eprintln!("  [a{}] always allow {}", index + 1, grant.label);
    }
    eprint!("Allow? [y]es once / [a]lways (a1) / [N]o: ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return yi_runtime::AskOutcome::Reject;
    }
    match line.trim().to_lowercase().as_str() {
        "y" | "yes" => yi_runtime::AskOutcome::AllowOnce,
        "a" | "always" => yi_runtime::AskOutcome::AllowAlways(0),
        other => other
            .strip_prefix('a')
            .and_then(|index| index.parse::<usize>().ok())
            .and_then(|index| index.checked_sub(1))
            .map_or(
                yi_runtime::AskOutcome::Reject,
                yi_runtime::AskOutcome::AllowAlways,
            ),
    }
}
