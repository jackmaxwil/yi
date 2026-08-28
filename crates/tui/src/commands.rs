use std::sync::Arc;

use yi_runtime::AgentSession;

use crate::app::App;
use crate::cell::Cell;

/// A5's dispatch: every slash command that reads or changes runtime state runs
/// here, against the session the event loop owns.
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
        other => format!("unknown command: /{other}"),
    };
    app.commit_cell(&Cell::Notice { text });
    app.scheduler.request();
}

/// V11 is a user act: no host request exists, so only this command promotes.
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

/// Read-only: a plan is grown through the model's own `plan.*` calls, whose
/// transitions are host-verified (D52); a TUI shortcut past that would be a
/// second, unchecked writer.
fn plan_command(session: &Arc<AgentSession>) -> String {
    let Some(service) = session.plan_service() else {
        return "/plan: no plan service is attached to this session".to_owned();
    };
    match service.read_plan() {
        None => "/plan: no plan in this session".to_owned(),
        Some(plan) => {
            let frontier = yi_runtime::plan::frontier_text(&plan);
            let summary = yi_runtime::plan::summary_line(&plan);
            if frontier.is_empty() {
                summary
            } else {
                format!("{summary}\n{frontier}")
            }
        }
    }
}

/// Read-only for the same reason plus G2: a goal is explicit-only and creating
/// one from a keystroke is exactly the inference that rule forbids.
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
