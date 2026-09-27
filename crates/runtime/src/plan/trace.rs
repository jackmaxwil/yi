//! Two-way traceability: every root todo cites a user message it serves, and every user message
//! is cited or waived. A declaring op journals what fails either way; it never refuses.

use std::collections::HashSet;

use serde_json::{Map, Value};
use yi_types::plan::doc::{Plan, PlanId, PlanTier, Todo, TodoLabel, TodoState};
use yi_types::plan::ledger::{JournalRecord, PlanOpRecord};
use yi_types::plan::op::{Op, TodoSpec};
use yi_types::url::{Scheme, Url};

use super::ops::{PlanEngine, Txn};

pub const TRACE_KEY: &str = "trace";

/// Flags a notice names before it points at the plan for the rest.
pub const TRACE_SHOWN: usize = 8;

pub struct Trace {
    pub unasked: Vec<TodoLabel>,
    pub forgotten: Vec<Url>,
}

impl Trace {
    fn rows(&self) -> [Vec<String>; 2] {
        [
            self.unasked
                .iter()
                .map(|label| label.as_str().to_owned())
                .collect(),
            self.forgotten.iter().map(Url::to_string).collect(),
        ]
    }
}

/// Invariant: `user://<n>` is `asked[n-1]`, the index every `user://` reader shares; a message
/// off the live branch still resolves but is never forgotten, since a rewind took it back.
pub fn trace(plan: &Plan, asked: &[bool]) -> Trace {
    let resolves = |url: &Url| ordinal(url).is_some_and(|n| n <= asked.len());
    let live: Vec<&Todo> = plan
        .todos
        .iter()
        .filter(|todo| !matches!(todo.state, TodoState::Abandoned))
        .collect();
    let mut covered: Vec<bool> = asked.iter().map(|live| !live).collect();
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

    fn owner_messages(&self) -> Option<Vec<bool>> {
        let store = (self.owner_words.as_ref()?)()?;
        let typed = crate::fetch::user_entries(&store).ok()?;
        let query = yi_session::EntryQuery {
            order: yi_session::EntryOrder::OldestFirst,
            ..yi_session::EntryQuery::default()
        };
        let branch = yi_session::lock_session(&store)
            .find_entries_on_branch("main", &query, &yi_session::BranchBounds::default())
            .ok()?;
        let live: HashSet<&str> = branch.iter().map(yi_types::entry::Entry::id).collect();
        let asked: Vec<bool> = typed
            .iter()
            .map(|(id, _)| live.contains(id.as_str()))
            .collect();
        asked.contains(&true).then_some(asked)
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
            .and_then(|asked| asked.iter().rposition(|live| *live))
            .and_then(|at| user_url(at.saturating_add(1)))
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

    /// Invariant: the journal record is the flags' authority, so the notice renders from it. A
    /// flag this plan's journal raised that still held before the op is standing, not raised again.
    pub(super) fn trace_into(
        &self,
        txn: &Txn,
        op: &Op,
        plan: &Plan,
        extra: &mut Map<String, Value>,
    ) {
        let declares = matches!(
            op,
            Op::Init { .. }
                | Op::Append { .. }
                | Op::Drop { .. }
                | Op::Supersede { .. }
                | Op::Set { .. }
        ) && matches!(plan.tier, PlanTier::Root);
        let Some(asked) = self.owner_messages().filter(|_| declares) else {
            return;
        };
        let now = trace(plan, &asked).rows();
        let was = txn
            .state
            .plan(&plan.id)
            .ok()
            .map(|was| trace(was, &asked).rows());
        let mut flags = Map::new();
        for ((row, now), was) in ROWS.iter().zip(now).zip(was.unwrap_or_default()) {
            let raised = raised(&txn.records, &plan.id, row.0);
            let fresh: Vec<String> = now
                .into_iter()
                .filter(|flag| !(was.contains(flag) && raised.contains(flag.as_str())))
                .collect();
            if !fresh.is_empty() {
                flags.insert(row.0.to_owned(), fresh.into());
            }
        }
        if !flags.is_empty() {
            extra.insert(TRACE_KEY.to_owned(), Value::Object(flags));
        }
    }
}

fn raised<'a>(records: &'a [JournalRecord], plan: &PlanId, key: &str) -> HashSet<&'a str> {
    records
        .iter()
        .filter(|record| &record.record.plan == plan)
        .filter_map(|record| record.record.extra.get(TRACE_KEY)?.get(key)?.as_array())
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

impl super::ops::Delta {
    pub(super) fn noticed(mut self, record: &PlanOpRecord) -> Self {
        self.notices.extend(notices(record));
        self
    }
}

/// What each flag says, its remedy, and where a cut list's rest is, keyed as the journal has it.
const ROWS: [(&str, &str, &str, &str); 2] = [
    (
        "unasked",
        "todo(s) cite no user message that resolves, so nobody asked for them",
        "cite one with intent: [\"user://<n>\"]",
        "the rest are the todos whose intent cites no user message",
    ),
    (
        "forgotten",
        "user message(s) no todo cites or waives, possibly forgotten",
        "fetch one to read it, then cite it in a todo's intent or waive it with waived: [{address, reason}]",
        "the rest are the user://<n> no todo's intent or waiver names",
    ),
];

pub fn notices(record: &PlanOpRecord) -> Vec<String> {
    let Some(found) = record.extra.get(TRACE_KEY) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, what, remedy, rest) in ROWS {
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
                "[… {TRACE_SHOWN} of {} shown (trace cap {TRACE_SHOWN}); {rest} in fetch plan://{}]",
                items.len(),
                record.plan
            ));
        }
    }
    out
}
