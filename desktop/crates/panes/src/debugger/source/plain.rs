//! The plain words of a source area with no stop to show: not started, no adapter, exited.

use rune::{El, col, text};
use yd_core::tokens::Tokens;
use yd_ui::theme::look::{BODY, LABEL, colour};

use crate::debugger::pane::session::{Session, Status};

/// What the source area says when there is no stop to show: a heading and a line under it.
pub fn words(session: Option<&Session>) -> (String, String) {
    let Some(session) = session else {
        return nothing();
    };
    let reason = session.reason.trim();
    let because = |plain: &str| {
        if reason.is_empty() {
            plain.to_owned()
        } else {
            reason.to_owned()
        }
    };
    match &session.status {
        Status::Idle => nothing(),
        Status::NotInstalled => (
            "No debugger found".to_owned(),
            because("Install LLDB or GDB to debug a program here."),
        ),
        Status::Starting => (
            format!("Starting {}", session.program),
            "The program stops at its first breakpoint.".to_owned(),
        ),
        Status::Running => (
            format!("{} is running", session.program),
            "Its source shows when it stops at a breakpoint.".to_owned(),
        ),
        Status::Exited => (
            "The program exited".to_owned(),
            because("Restart it to debug it again."),
        ),
        Status::Stopped => (
            "The debugger stopped".to_owned(),
            "Restart it to debug the program again.".to_owned(),
        ),
        Status::Error => (
            "The debugger stopped".to_owned(),
            because("It reported no reason."),
        ),
        Status::Paused => (
            "No source for this frame".to_owned(),
            "The frame is in a library; step out to reach your code.".to_owned(),
        ),
        Status::Other(word) => (word.clone(), because("")),
    }
}

fn nothing() -> (String, String) {
    (
        "Nothing is being debugged".to_owned(),
        "Launch a program from the command box: Launch in Debugger.".to_owned(),
    )
}

/// The block the source area shows in place of code.
pub fn plain(tokens: &Tokens, key: &str, heading: &str, line: &str) -> El {
    col()
        .key(key)
        .w_full()
        .grow(1.0)
        .center()
        .gap(4.0)
        .px(16.0)
        .child(text(heading.to_owned()).font(BODY).color(colour(tokens.fg)))
        .child(
            text(line.to_owned())
                .font(LABEL)
                .color(colour(tokens.faint))
                .wrap(),
        )
}
