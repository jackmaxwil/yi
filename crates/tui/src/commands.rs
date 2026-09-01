use std::sync::Arc;

use yi_runtime::session_store::{JsonlRepo, SessionRepo, age_label, now_ms};
use yi_runtime::{AgentSession, PermissionMode};

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
        "advisor" => advisor_command(session, args),
        "plan" => plan_command(session),
        "goal" => goal_command(session),
        "permissions" => permissions_command(session, args),
        "compact" => compact_command(session, args),
        "sessions" => sessions_command(app, args),
        other => format!("unknown command: /{other}"),
    };
    app.commit_cell(&Cell::Notice { text });
    app.scheduler.request();
}

/// Invariant: V11 promotion has no host request behind it, so this command is
/// the only writer.
fn advisor_command(session: &Arc<AgentSession>, args: &str) -> String {
    let Some(advisor) = session.advisor() else {
        return "/advisor: no advisor is attached to this session".to_owned();
    };
    match args.split_once(char::is_whitespace) {
        Some(("promote", id)) => match yi_runtime::advisor::promote_advice(session, id.trim()) {
            Ok(path) => format!(
                "/advisor promote {}: armed now and standing in {}",
                id.trim(),
                path.display()
            ),
            Err(error) => format!("/advisor promote: {error}"),
        },
        _ if args == "promote" => {
            "/advisor promote <advice-id> — the id is in the advisory's details".to_owned()
        }
        _ => {
            let promotable = advisor.promotable();
            let listed = if promotable.is_empty() {
                "nothing to promote yet".to_owned()
            } else {
                promotable
                    .iter()
                    .rev()
                    .take(5)
                    .map(|(id, advice)| format!("  {id}  {advice}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            format!("{}\npromotable:\n{listed}", advisor.stats())
        }
    }
}

/// Invariant: the canonical plan file has one writer; this command only renders.
fn plan_command(session: &Arc<AgentSession>) -> String {
    use yi_runtime::plan::{CanonicalPlanError, frontier_text, summary_line};
    let Some(service) = session.plan_service() else {
        return "/plan: no plan service is attached to this session".to_owned();
    };
    match service.read_plan() {
        Err(CanonicalPlanError::NoPlanOpen { .. }) => "/plan: no plan is open".to_owned(),
        Err(error) => format!("/plan: {error}"),
        Ok(plan) => match frontier_text(&plan) {
            frontier if frontier.is_empty() => summary_line(&plan),
            frontier => format!("{}\n{frontier}", summary_line(&plan)),
        },
    }
}

/// Invariant: a goal is explicit-only (G2), so a keystroke may read one and
/// never create one.
fn goal_command(session: &Arc<AgentSession>) -> String {
    let Some(service) = session.goal_service() else {
        return "/goal: no goal service is attached to this session".to_owned();
    };
    match service.get() {
        Err(error) => format!("/goal: {error}"),
        Ok(goal) => {
            let field = |key: &str| {
                goal.get(key)
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_owned()
            };
            let number = |key: &str| goal.get(key).and_then(serde_json::Value::as_u64);
            let budget = match (number("tokensUsed"), number("tokenBudget")) {
                (Some(used), Some(budget)) => format!(" · {used}/{budget} tokens"),
                (Some(used), None) => format!(" · {used} tokens"),
                _ => String::new(),
            };
            let check = goal
                .get("check")
                .and_then(serde_json::Value::as_str)
                .map(|check| format!("\ncheck: {check}"))
                .unwrap_or_default();
            let failure = goal
                .get("checkFailure")
                .and_then(serde_json::Value::as_str)
                .map(|tail| format!("\nlast check failure:\n{tail}"))
                .unwrap_or_default();
            format!(
                "goal [{}]{budget}\n{}{check}{failure}",
                field("status"),
                field("objective")
            )
        }
    }
}

fn parse_mode(text: &str) -> Result<PermissionMode, &'static str> {
    match text {
        "ask" => Ok(PermissionMode::Ask),
        "auto" => Ok(PermissionMode::Auto),
        "yolo" => Ok(PermissionMode::Yolo),
        _ => Err("ask, auto, or yolo"),
    }
}

fn permissions_command(session: &Arc<AgentSession>, args: &str) -> String {
    let Some(broker) = session.permission_broker() else {
        return "/permissions: no permission broker is attached to this session".to_owned();
    };
    if args.is_empty() {
        return format!(
            "permission mode: {}",
            yi_runtime::gate::mode_label(broker.mode())
        );
    }
    match parse_mode(args) {
        Ok(mode) => {
            broker.set_mode_and_fragment(mode, session);
            format!("permission mode: {}", yi_runtime::gate::mode_label(mode))
        }
        Err(want) => format!("/permissions [{want}]"),
    }
}

fn compact_command(session: &Arc<AgentSession>, args: &str) -> String {
    let Some(compactor) = session.compactor() else {
        return "/compact: compaction is not attached to this session".to_owned();
    };
    if args.is_empty() {
        compactor.schedule();
        "compaction scheduled".to_owned()
    } else {
        compactor.schedule_with_instructions(Some(args.to_owned()));
        format!("compaction scheduled · {args}")
    }
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
        #[rustfmt::skip]
        const NEED: [&str; 6] = ["advisor", "plan", "goal", "permissions", "compact", "sessions"];
        assert!(NEED.iter().all(|v| SLASH_COMMANDS.contains(v)));
    }
}
