//! The plan tool's reply: an outcome rendered as the counts, the frontier and the stepped todo.

use serde_json::Value;
use yi_types::plan::doc::{Plan, PlanTier, Todo, TodoLabel, TodoState, TodoStateName};

use super::ops::{Op, Outcome};

const WINDOW: usize = 8;

fn labels(of: &[TodoLabel]) -> String {
    of.iter()
        .map(TodoLabel::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

fn counts(todos: &[Todo]) -> String {
    let mut parts = Vec::new();
    for name in [
        TodoStateName::Pending,
        TodoStateName::Running,
        TodoStateName::Blocked,
        TodoStateName::Done,
        TodoStateName::Failed,
        TodoStateName::Abandoned,
    ] {
        let count = todos
            .iter()
            .filter(|todo| TodoStateName::of(&todo.state) == name)
            .count();
        if count > 0 {
            parts.push(format!("{count} {name}"));
        }
    }
    let unknown = todos
        .iter()
        .filter(|todo| matches!(TodoStateName::of(&todo.state), TodoStateName::Other(_)))
        .count();
    if unknown > 0 {
        parts.push(format!("{unknown} unknown"));
    }
    if parts.is_empty() {
        return "no todos".to_owned();
    }
    format!("{} todos: {}", todos.len(), parts.join(", "))
}

fn todo_line(todo: &Todo) -> String {
    let mut line = format!("- {} {}", TodoStateName::of(&todo.state), todo.label);
    if !todo.after.is_empty() {
        line.push_str(&format!(" after {}", labels(&todo.after)));
    }
    match &todo.state {
        TodoState::Running { by } => line.push_str(&format!(" by {by}")),
        TodoState::Blocked { on, note } => {
            let on = serde_json::to_string(on).unwrap_or_else(|_| "?".to_owned());
            line.push_str(&format!(" on {on}: {note}"));
        }
        TodoState::Done { output, resolution } => {
            if let Some(url) = output {
                line.push_str(&format!(" -> {url}"));
            }
            if let Some(resolution) = resolution {
                line.push_str(&format!(" ({resolution})"));
            }
        }
        TodoState::Failed { cause, last } => {
            line.push_str(&format!(" ! {cause}"));
            if let Some(url) = last {
                line.push_str(&format!(" (last {url})"));
            }
        }
        TodoState::Pending | TodoState::Abandoned | TodoState::Other(_) => {}
    }
    line.push_str(&super::ask::line(todo));
    if let Some(Value::String(url)) = todo.extra.get(super::state::SUBMITTED_KEY) {
        line.push_str(&format!(" submitted {url}"));
    }
    if !todo.retries.is_zero() {
        line.push_str(&format!(" retries {}", todo.retries.0));
    }
    if let Some(subplan) = &todo.subplan {
        line.push_str(&format!(" [subplan {subplan}]"));
    }
    line
}

fn header(plan: &Plan, out: &mut Vec<String>) {
    out.push(format!(
        "plan {} v{} touched {} {}",
        plan.id, plan.version.0, plan.touched.0, plan.state
    ));
    match &plan.tier {
        PlanTier::Root => {}
        PlanTier::Sub { parent } => out.push(format!("sub of {parent}")),
        PlanTier::Other { tier, parent } => {
            let parent = parent.as_ref().map(ToString::to_string).unwrap_or_default();
            out.push(format!("tier {tier} {parent}"));
        }
    }
    out.push(counts(&plan.todos));
    let progress = yi_types::plan::doc::progress(&plan.todos);
    let mut line = format!("progress {}/{}", progress.done, progress.total);
    if let Some(running) = progress.running {
        line.push_str(&format!(" · now: {running}"));
    }
    out.push(line);
}

fn body(outcome: &Outcome, full: bool, stepped: Option<&Todo>, out: &mut Vec<String>) {
    let plan = &outcome.plan;
    let rest = plan
        .todos
        .iter()
        .filter(|todo| stepped.is_none_or(|led| led.label != todo.label));
    let shown: Vec<&Todo> = if full {
        rest.collect()
    } else {
        rest.filter(|todo| {
            outcome.ready.contains(&todo.label)
                || matches!(
                    TodoStateName::of(&todo.state),
                    TodoStateName::Running | TodoStateName::Blocked
                )
        })
        .take(WINDOW)
        .collect()
    };
    out.extend(shown.iter().map(|todo| todo_line(todo)));
    let printed = shown.len().saturating_add(usize::from(stepped.is_some()));
    let hidden = plan.todos.len().saturating_sub(printed);
    if hidden > 0 {
        out.push(format!(
            "{hidden} more todos not shown; op view full for all"
        ));
    }
}

pub(super) fn render(outcome: &Outcome, full: bool, stepped: Option<&TodoLabel>) -> String {
    let mut out = Vec::new();
    header(&outcome.plan, &mut out);
    let stepped =
        stepped.and_then(|label| outcome.plan.todos.iter().find(|todo| todo.label == *label));
    out.extend(stepped.map(todo_line));
    if full {
        out.push(format!("goal: {}", outcome.plan.goal));
        out.push(format!(
            "spawns {} of {}",
            outcome.plan.spawns().get(),
            yi_types::plan::doc::SPAWN_CAP.get()
        ));
    }
    body(outcome, full, stepped, &mut out);
    if !outcome.ready.is_empty() {
        out.push(format!("ready: {}", labels(&outcome.ready)));
    }
    if !outcome.dispatched.is_empty() {
        out.push(format!("dispatched: {}", labels(&outcome.dispatched)));
    }
    if !outcome.held.is_empty() {
        out.push(format!(
            "held behind the dispatch width: {} ({})",
            outcome.held.len(),
            labels(&outcome.held)
        ));
    }
    for url in &outcome.spawned {
        out.push(format!("spawned {url}"));
    }
    out.extend(outcome.notices.iter().cloned());
    for url in &outcome.reaped {
        out.push(format!("reaped {url}"));
    }
    if let Some(subplan) = &outcome.subplan {
        out.push(format!("subplan {subplan}"));
    }
    out.join("\n")
}

pub(crate) fn render_outcome(op: &Op, outcome: &Outcome) -> String {
    let full = matches!(op, Op::View { full: true });
    render(outcome, full, op.label())
}

#[cfg(test)]
mod tests {
    use super::*;
    use yi_types::plan::doc::{GoalText, PlanId};

    type Fallible = Result<(), Box<dyn std::error::Error>>;

    fn todo(label: &str, state: TodoState) -> Result<Todo, yi_types::plan::doc::DocError> {
        Ok(Todo {
            state,
            ..Todo::pending(TodoLabel::new(label)?)
        })
    }

    #[test]
    fn a_windowed_view_counts_what_it_hid() -> Fallible {
        let by = yi_types::plan::doc::AgentId::new("kid")?;
        let mut plan = Plan::opening(
            PlanId::new("ship-it")?,
            GoalText::new("ship it")?,
            PlanTier::Root,
            vec![
                todo(
                    "cut",
                    TodoState::Done {
                        output: None,
                        resolution: None,
                    },
                )?,
                todo("build", TodoState::Running { by })?,
                todo("ship", TodoState::Pending)?,
            ],
        );
        plan.touched = yi_types::plan::doc::TouchCount(4);
        let outcome = Outcome {
            plan,
            ready: Vec::new(),
            dispatched: Vec::new(),
            held: Vec::new(),
            spawned: Vec::new(),
            reaped: Vec::new(),
            subplan: None,
            notices: Vec::new(),
            standing: Default::default(),
        };
        let windowed = render(&outcome, false, None);
        assert!(
            windowed.contains("3 todos: 1 pending, 1 running, 1 done"),
            "{windowed}"
        );
        assert!(windowed.contains("- running build by kid"), "{windowed}");
        assert!(windowed.contains("2 more todos not shown"), "{windowed}");
        let full = render(&outcome, true, None);
        assert!(full.contains("- pending ship"), "{full}");
        assert!(!full.contains("not shown"), "{full}");
        Ok(())
    }
}
