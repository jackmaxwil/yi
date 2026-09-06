//! The terminal's permission prompt: one question on stderr, one keystroke back.

pub(crate) fn tty_ask(ask: &yi_runtime::PermissionAsk<'_>) -> yi_runtime::AskOutcome {
    use std::io::Write;
    eprintln!("\n{}\n{}", ask.title, ask.text());
    eprint!("Allow? [y]es once / [a]lways / [N]o: ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return yi_runtime::AskOutcome::Reject;
    }
    match line.trim().to_lowercase().as_str() {
        "y" | "yes" => yi_runtime::AskOutcome::AllowOnce,
        "a" | "always" => yi_runtime::AskOutcome::AllowAlways,
        _ => yi_runtime::AskOutcome::Reject,
    }
}
