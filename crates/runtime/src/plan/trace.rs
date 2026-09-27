//! Two-way traceability: every root todo cites a user message it serves, and every user message
//! is cited or waived. A declaring op journals what fails either way; it never refuses.

use serde_json::{Map, Value, json};
use yi_types::plan::doc::{Plan, PlanId, PlanTier, Todo, TodoLabel, TodoState};
use yi_types::plan::ledger::PlanOpRecord;
use yi_types::plan::op::{Op, TodoSpec};
use yi_types::url::{Scheme, Url};

use super::ops::PlanEngine;

pub const TRACE_KEY: &str = "trace";

/// Flags a notice names before it points at the plan for the rest.
pub const TRACE_SHOWN: usize = 8;

pub struct Trace {
    pub unasked: Vec<TodoLabel>,
    pub forgotten: Vec<Url>,
}

/// Invariant: `user://<n>` counts the user's own messages from 1, the index every `user://`
/// reader shares, so `owner` messages resolve exactly the ordinals `1..=owner`.
pub fn trace(plan: &Plan, owner: usize) -> Trace {
    let resolves = |url: &Url| ordinal(url).is_some_and(|n| n <= owner);
    let live: Vec<&Todo> = plan
        .todos
        .iter()
        .filter(|todo| !matches!(todo.state, TodoState::Abandoned))
        .collect();
    let mut covered = vec![false; owner];
    let mut walk: Vec<&Todo> = live.clone();
    while let Some(todo) = walk.pop() {
        let waived = todo.cites.waived.iter().map(|waiver| &waiver.address);
        for n in todo.cites.intent.iter().chain(waived).filter_map(ordinal) {
            if let Some(slot) = n.checked_sub(1).and_then(|at| covered.get_mut(at)) {
                *slot = true;
            }
        }
        walk.extend(todo.children.iter());
    }
    Trace {
        unasked: live
            .iter()
            .filter(|todo| !todo.cites.intent.iter().any(resolves))
            .map(|todo| todo.label.clone())
            .collect(),
        forgotten: covered
            .iter()
            .enumerate()
            .filter(|(_, cited)| !**cited)
            .filter_map(|(at, _)| user_url(at.saturating_add(1)))
            .collect(),
    }
}

fn ordinal(url: &Url) -> Option<usize> {
    (url.scheme() == &Scheme::User)
        .then(|| url.path().parse::<usize>().ok())
        .flatten()
        .filter(|n| *n >= 1)
}

fn user_url(n: usize) -> Option<Url> {
    format!("user://{n}").parse().ok()
}

impl PlanEngine {
    pub fn with_owner_words(self, words: crate::goal::StoreHandle) -> Self {
        Self {
            owner_words: Some(words),
            ..self
        }
    }

    fn owner_messages(&self) -> Option<usize> {
        let store = (self.owner_words.as_ref()?)()?;
        let inputs = crate::fetch::user_inputs(&store).ok()?;
        (!inputs.is_empty()).then_some(inputs.len())
    }

    /// A declared todo citing nothing keeps its label's old cites, else cites the latest message.
    /// ponytail: the latest message stands in for the drafting turn; a wake-driven turn cites it.
    pub(super) fn cite_default(&self, mut op: Op, plan: Option<&PlanId>) -> Op {
        let specs: Vec<&mut TodoSpec> = match &mut op {
            Op::Init { todos, .. }
            | Op::Append { todos }
            | Op::Decompose { todos, .. }
            | Op::Supersede { todos, .. } => todos.iter_mut().collect(),
            Op::Set { rows, .. } => rows.iter_mut().map(|row| &mut row.spec).collect(),
            _ => Vec::new(),
        };
        let Some(latest) = (!specs.is_empty())
            .then(|| self.owner_messages())
            .flatten()
            .and_then(user_url)
        else {
            return op;
        };
        let current = self
            .resolve(plan.cloned())
            .ok()
            .and_then(|id| self.store.read(&id).ok());
        for spec in specs {
            match current.as_ref().and_then(|plan| plan.todo(&spec.label)) {
                Some(held) if spec.cites.is_empty() => spec.cites = held.cites.clone(),
                None if spec.cites.intent.is_empty() => spec.cites.intent = vec![latest.clone()],
                _ => {}
            }
        }
        op
    }

    /// Invariant: the journal record is the flags' authority, so the notice renders from it.
    pub(super) fn trace_into(&self, op: &Op, plan: &Plan, extra: &mut Map<String, Value>) {
        let declares = matches!(
            op,
            Op::Init { .. }
                | Op::Append { .. }
                | Op::Drop { .. }
                | Op::Supersede { .. }
                | Op::Set { .. }
        ) && matches!(plan.tier, PlanTier::Root);
        let Some(found) = self
            .owner_messages()
            .filter(|_| declares)
            .map(|n| trace(plan, n))
        else {
            return;
        };
        if !found.unasked.is_empty() || !found.forgotten.is_empty() {
            let labels: Vec<&str> = found.unasked.iter().map(TodoLabel::as_str).collect();
            let urls: Vec<String> = found.forgotten.iter().map(Url::to_string).collect();
            let flags = json!({"unasked": labels, "forgotten": urls});
            extra.insert(TRACE_KEY.to_owned(), flags);
        }
    }
}

impl super::ops::Delta {
    pub(super) fn noticed(mut self, record: &PlanOpRecord) -> Self {
        self.notices.extend(notices(record));
        self
    }
}

/// What each flag says and the remedy it names, keyed as the journal records it.
const ROWS: [(&str, &str, &str); 2] = [
    (
        "unasked",
        "todo(s) cite no user message that resolves, so nobody asked for them",
        "cite one with intent: [\"user://<n>\"]",
    ),
    (
        "forgotten",
        "user message(s) no todo cites or waives, possibly forgotten",
        "fetch one to read it, then cite it in a todo's intent or waive it with waived: [{address, reason}]",
    ),
];

pub fn notices(record: &PlanOpRecord) -> Vec<String> {
    let Some(found) = record.extra.get(TRACE_KEY) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, what, remedy) in ROWS {
        let items = found.get(key).and_then(Value::as_array);
        let items: Vec<&str> = items
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        if items.is_empty() {
            continue;
        }
        let shown: Vec<String> = items
            .iter()
            .take(TRACE_SHOWN)
            .map(|item| format!("{item:?}"))
            .collect();
        out.push(format!(
            "trace: {} {what}: {}; {remedy}",
            items.len(),
            shown.join(", ")
        ));
        if items.len() > TRACE_SHOWN {
            out.push(format!(
                "[… {TRACE_SHOWN} of {} shown (trace cap {TRACE_SHOWN}); fetch plan://{} shows every todo's intent]",
                items.len(),
                record.plan
            ));
        }
    }
    out
}
