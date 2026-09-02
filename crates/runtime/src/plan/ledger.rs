use std::collections::{HashMap, HashSet};

use yi_types::plan::doc::{BlockedOn, Check, Plan, PlanId, TodoLabel, TodoState, TodoStateName};
use yi_types::plan::ledger::{PLAN_OP_ENTRY_TYPE, PlanOpRecord};
use yi_types::url::Durability;

/// The op stream's writer, beside its readers: every applied op becomes one
/// durable custom entry on the owning session, which is where every duration
/// below is read back from.
pub struct SessionOpSink(pub crate::goal::StoreHandle);

impl super::ops::OpSink for SessionOpSink {
    fn record(&self, record: PlanOpRecord) {
        let Some(session) = (self.0)() else {
            return;
        };
        let Ok(payload) = serde_json::to_value(&record) else {
            return;
        };
        let _a_ledger_write_never_fails_an_op = yi_session::lock_session(&session).append_custom(
            "main",
            PLAN_OP_ENTRY_TYPE,
            Some(payload),
        );
    }
}

/// Every `custom{plan_op}` entry this session recorded, oldest first.
pub fn records(session: &yi_session::SharedSession) -> Vec<PlanOpRecord> {
    let entries = yi_session::lock_session(session)
        .find_entries(&yi_session::EntryQuery {
            order: yi_session::EntryOrder::OldestFirst,
            ..yi_session::EntryQuery::default()
        })
        .unwrap_or_default();
    entries
        .iter()
        .filter_map(|entry| {
            let yi_types::entry::Entry::Custom {
                custom_type,
                data: Some(data),
                ..
            } = entry
            else {
                return None;
            };
            if custom_type != PLAN_OP_ENTRY_TYPE {
                return None;
            }
            serde_json::from_value::<PlanOpRecord>(data.clone()).ok()
        })
        .collect()
}

/// One todo's time, from the transition timestamps and nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoOutcome {
    pub label: TodoLabel,
    pub running_ms: u64,
    pub waiting_ms: u64,
    pub blocked_ms: u64,
    pub retries: u32,
    pub ended: Option<TodoStateName>,
}

impl TodoOutcome {
    /// Blocked time is on the critical path: a successor cannot start through
    /// it, however little of it was the todo's own work.
    pub fn span_ms(&self) -> u64 {
        self.running_ms.saturating_add(self.blocked_ms)
    }
}

/// Invariant: every number here is a difference of two recorded timestamps, so
/// a plan whose op stream is missing is reported empty rather than guessed at.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub plan: Option<PlanId>,
    pub wall_ms: u64,
    pub critical_path_ms: u64,
    pub todos: Vec<TodoOutcome>,
    pub at_init: u32,
    pub widest: u32,
}

impl Report {
    /// K3's serial fraction: the critical path against the clock the run
    /// actually took. Above 1 means the path overlapped itself, which a
    /// re-cut plan can do; the caller sees the raw ratio, not a clamp.
    pub fn serial_fraction(&self) -> Option<f64> {
        (self.wall_ms > 0).then(|| {
            let path = u32::try_from(self.critical_path_ms).unwrap_or(u32::MAX);
            let wall = u32::try_from(self.wall_ms).unwrap_or(u32::MAX);
            f64::from(path) / f64::from(wall)
        })
    }

    /// Todos added mid-flight over todos at init: coding low, investigation
    /// high, a game near 1. The label nobody annotates, for free.
    pub fn discovery_ratio(&self) -> Option<f64> {
        (self.at_init > 0)
            .then(|| f64::from(self.widest.saturating_sub(self.at_init)) / f64::from(self.at_init))
    }
}

fn bucket(outcome: &mut TodoOutcome, state: &TodoStateName, span: u64) {
    match state {
        TodoStateName::Running => outcome.running_ms = outcome.running_ms.saturating_add(span),
        TodoStateName::Pending => outcome.waiting_ms = outcome.waiting_ms.saturating_add(span),
        TodoStateName::Blocked => outcome.blocked_ms = outcome.blocked_ms.saturating_add(span),
        TodoStateName::Done
        | TodoStateName::Failed
        | TodoStateName::Abandoned
        | TodoStateName::Other(_) => {}
    }
}

fn terminal(state: &TodoStateName) -> bool {
    match state {
        TodoStateName::Done | TodoStateName::Failed | TodoStateName::Abandoned => true,
        TodoStateName::Pending
        | TodoStateName::Running
        | TodoStateName::Blocked
        | TodoStateName::Other(_) => false,
    }
}

/// Incident: a hand-edited cycle is never refused by the parser, and this walk
/// re-pushed it forever; an edge back into the open walk now counts as zero.
fn critical_path(plan: &Plan, spans: &HashMap<String, u64>) -> u64 {
    let mut best: HashMap<&str, u64> = HashMap::new();
    let mut open: HashSet<&str> = HashSet::new();
    let mut longest = 0u64;
    for todo in &plan.todos {
        let mut stack = vec![(todo.label.as_str(), false)];
        while let Some((name, expanded)) = stack.pop() {
            if best.contains_key(name) {
                continue;
            }
            let Some(node) = plan.todos.iter().find(|kin| kin.label.as_str() == name) else {
                best.insert(name, 0);
                continue;
            };
            if !expanded {
                let pending: Vec<&str> = node
                    .after
                    .iter()
                    .map(TodoLabel::as_str)
                    .filter(|edge| !best.contains_key(edge) && !open.contains(edge))
                    .collect();
                if !pending.is_empty() {
                    open.insert(name);
                    stack.push((name, true));
                    stack.extend(pending.into_iter().map(|edge| (edge, false)));
                    continue;
                }
            }
            let inbound = node
                .after
                .iter()
                .filter_map(|edge| best.get(edge.as_str()).copied())
                .max()
                .unwrap_or(0);
            let own = spans.get(name).copied().unwrap_or(0);
            let total = inbound.saturating_add(own);
            best.insert(name, total);
            longest = longest.max(total);
        }
    }
    longest
}

/// The outcome ledger, Amdahl and the discovery ratio, all one pass over the
/// op stream plus the plan's own edges.
pub fn report(plan: &Plan, records: &[PlanOpRecord]) -> Report {
    let mut ordered: Vec<&PlanOpRecord> = records
        .iter()
        .filter(|record| record.plan == plan.id)
        .collect();
    ordered.sort_by_key(|record| record.at);
    let (Some(first), Some(last)) = (ordered.first(), ordered.last()) else {
        return Report::default();
    };
    let mut outcomes: HashMap<String, TodoOutcome> = HashMap::new();
    let mut standing: HashMap<String, (TodoStateName, u64)> = HashMap::new();
    let mut at_init = 0u32;
    let mut widest = 0u32;
    for record in &ordered {
        widest = widest.max(record.todos);
        if record.op == "init" {
            at_init = record.todos;
        }
        let Some(label) = &record.todo else {
            continue;
        };
        let key = label.as_str().to_owned();
        let outcome = outcomes.entry(key.clone()).or_insert_with(|| TodoOutcome {
            label: label.clone(),
            running_ms: 0,
            waiting_ms: 0,
            blocked_ms: 0,
            retries: 0,
            ended: None,
        });
        if record.op == "retry" {
            outcome.retries = outcome.retries.saturating_add(1);
        }
        if let Some((was, since)) = standing.get(&key) {
            bucket(outcome, was, record.at.saturating_sub(*since));
        }
        if let Some(now) = &record.to {
            outcome.ended = terminal(now).then(|| now.clone());
            standing.insert(key, (now.clone(), record.at));
        }
    }
    // A todo still in flight accrues to the last recorded op, never to now:
    // the same stream read twice must report the same numbers.
    for (key, (was, since)) in &standing {
        if let Some(outcome) = outcomes.get_mut(key)
            && outcome.ended.is_none()
        {
            bucket(outcome, was, last.at.saturating_sub(*since));
        }
    }
    let spans: HashMap<String, u64> = outcomes
        .iter()
        .map(|(key, outcome)| (key.clone(), outcome.span_ms()))
        .collect();
    let mut todos: Vec<TodoOutcome> = outcomes.into_values().collect();
    todos.sort_by(|left, right| left.label.as_str().cmp(right.label.as_str()));
    Report {
        plan: Some(plan.id.clone()),
        wall_ms: last.at.saturating_sub(first.at),
        critical_path_ms: critical_path(plan, &spans),
        todos,
        at_init,
        widest,
    }
}

/// The §12 lint, bounded by what a lint may be: advisory, mechanical, and run
/// at a cut boundary rather than per op. A judgment rule would need a fitted
/// threshold, so none is here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub todo: Option<TodoLabel>,
    pub rule: &'static str,
    pub detail: String,
}

pub fn lint(plan: &Plan, width: usize) -> Vec<Finding> {
    let mut findings = Vec::new();
    let ready = plan.ready();
    let delegated = ready
        .iter()
        .filter(|todo| todo.delegation.is_some())
        .count();
    if delegated > width {
        findings.push(Finding {
            todo: None,
            rule: "width",
            detail: format!(
                "{delegated} ready todos delegate at once against a measured width of {width}; \
                 {} will be held rather than run",
                delegated.saturating_sub(width)
            ),
        });
    }
    for todo in &plan.todos {
        match &todo.state {
            TodoState::Done { output: Some(url) } => {
                if url.durability() == Durability::Ephemeral {
                    findings.push(Finding {
                        todo: Some(todo.label.clone()),
                        rule: "ephemeral-terminal",
                        detail: format!("done names {url}, which will not outlive the run"),
                    });
                }
            }
            TodoState::Failed {
                cause: _,
                last: Some(url),
            } => {
                if url.durability() == Durability::Ephemeral {
                    findings.push(Finding {
                        todo: Some(todo.label.clone()),
                        rule: "ephemeral-terminal",
                        detail: format!("last product {url} will not outlive the run"),
                    });
                }
            }
            TodoState::Other(tag) => findings.push(Finding {
                todo: Some(todo.label.clone()),
                rule: "unknown-state",
                detail: format!("state {tag:?} admits no op; the step table cannot move it"),
            }),
            TodoState::Pending
            | TodoState::Running { .. }
            | TodoState::Blocked { .. }
            | TodoState::Done { output: None }
            | TodoState::Failed { last: None, .. }
            | TodoState::Abandoned => {}
        }
        if let Some(delegation) = &todo.delegation
            && matches!(delegation.accept, Check::Stated(_) | Check::Other(_))
        {
            findings.push(Finding {
                todo: Some(todo.label.clone()),
                rule: "unrunnable-acceptance",
                detail: "acceptance is prose, so nothing adjudicates done but the model".to_owned(),
            });
        }
        match blocked_on(&todo.state) {
            Some(BlockedOn::External { probe: None }) => findings.push(Finding {
                todo: Some(todo.label.clone()),
                rule: "no-probe",
                detail: "blocked on an external condition with no probe: only a person clears it"
                    .to_owned(),
            }),
            // `on: external` written flat lands in the untagged tail, which the
            // probe ladder cannot read; the nested `external: {probe: …}` can.
            Some(BlockedOn::Other(tag)) => findings.push(Finding {
                todo: Some(todo.label.clone()),
                rule: "unknown-blocker",
                detail: format!(
                    "blocked on {tag:?}, which is not child, user or external, so nothing but a \
                     person can clear it"
                ),
            }),
            Some(
                BlockedOn::Child(_) | BlockedOn::User | BlockedOn::External { probe: Some(_) },
            )
            | None => {}
        }
    }
    findings
}

fn blocked_on(state: &TodoState) -> Option<&BlockedOn> {
    match state {
        TodoState::Blocked { on, note: _ } => Some(on),
        TodoState::Pending
        | TodoState::Running { .. }
        | TodoState::Done { .. }
        | TodoState::Failed { .. }
        | TodoState::Abandoned
        | TodoState::Other(_) => None,
    }
}
