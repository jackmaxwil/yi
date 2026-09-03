use std::sync::Arc;

use yi_runtime::AgentSession;
use yi_runtime::session_store::{JsonlRepo, SessionRepo, age_label, now_ms};

use crate::app::App;
use crate::cell::Cell;

/// Invariant: the popup offers exactly what [`process_pending_command`] and
/// [`crate::input::handle_slash`] route, so the table lives beside them.
#[rustfmt::skip]
pub(crate) const SLASH_COMMANDS: [&str; 14] = [
    "new", "undo", "quit", "tree", "editor", "advisor", "plan", "plantree", "goal", "agents",
    "model", "permissions", "compact", "sessions",
];

pub fn process_pending_command(app: &mut App, session: &Arc<AgentSession>) {
    let Some(line) = app.pending_command.take() else {
        return;
    };
    let (command, args) = line
        .split_once(char::is_whitespace)
        .map_or((line.as_str(), ""), |(head, rest)| (head, rest.trim()));
    let text = match command {
        "sessions" => sessions_command(app, args),
        other => yi_runtime::slash::run(session, other, args)
            .unwrap_or_else(|| format!("unknown command: /{other}")),
    };
    app.commit_cell(&Cell::Notice { text });
    app.scheduler.request();
}

fn sessions_command(app: &App, args: &str) -> String {
    if !args.is_empty() && args != "list" {
        return "/sessions — listing only; show and rm stay on `yi sessions`".to_owned();
    }
    let mut repo = JsonlRepo::new(
        app.options.session_dir.clone().into(),
        app.options.cwd.clone(),
    );
    match repo.list() {
        Err(error) => format!("/sessions: {error}"),
        Ok(listed) if listed.is_empty() => "no sessions for this directory".to_owned(),
        Ok(listed) => {
            let now = now_ms();
            listed
                .iter()
                .map(|m| {
                    format!(
                        "{}  {:>8}",
                        m.id,
                        age_label(now.saturating_sub(m.created_at))
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
    }
}

pub fn process_pending_selection(app: &mut App, session: &Arc<AgentSession>) {
    let Some((model, effort)) = app.selection.pending.take() else {
        return;
    };
    let label = format!("{}/{}", model.provider, model.id);
    session.set_model(model);
    let effective = session.set_effort(effort);
    app.selection.model = session.model();
    app.selection.effort = effective;
    app.commit_cell(&Cell::Notice {
        text: if effective == yi_types::model::Effort::Off {
            format!("model {label}")
        } else {
            format!("model {label} · reasoning {effective}")
        },
    });
    app.scheduler.request();
}

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
