//! A todo blocked on the user with 3 to 5 options; only the user's reply picks one.

use std::collections::HashSet;

use serde_json::{Map, Value};
use yi_types::plan::ask::{Answer, Ask, AskError, AskOption};
use yi_types::plan::doc::{
    BlockedOn, Plan, PlanId, PlanIssue, Todo, TodoLabel, TodoState, TodoStateName,
};
use yi_types::plan::op::Op;

use super::ops::{PlanEngine, PlanOpError};
use super::table::OpKind;
use super::tool::{ArgError, label, need};
use super::trace::{ordinal, user_url};

pub(crate) fn parse(options: Option<&Value>, on_user: bool) -> Result<Option<Ask>, String> {
    let Some(options) = options.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    if !on_user {
        return Err(AskError::NotOnUser.to_string());
    }
    let options: Vec<AskOption> = serde_json::from_value(options.clone()).map_err(|cause| {
        format!("block options are [{{id, label, preview?}}], 3 to 5 of them: {cause}")
    })?;
    Ask::new(options)
        .map(Some)
        .map_err(|error| error.to_string())
}

pub(super) fn block(args: &Map<String, Value>, kind: OpKind) -> Result<Op, ArgError> {
    let on: BlockedOn = need(args, kind, "on")?;
    let ask = parse(args.get("options"), on == BlockedOn::User).map_err(ArgError::Declared)?;
    Ok(Op::Block {
        label: label(args, kind)?,
        on,
        note: need(args, kind, "note")?,
        ask: ask.map(Box::new),
    })
}

/// The option a reply picks: its first word, ended by punctuation or the reply's end, is an
/// option's number, id or label, and no later word names another; "B, but less copy" picks B.
fn named<'a>(ask: &'a Ask, reply: &str) -> Option<&'a AskOption> {
    let reply = reply.trim();
    let whole = ask
        .options
        .iter()
        .find(|option| option.label.as_str().eq_ignore_ascii_case(reply));
    if whole.is_some() {
        return whole;
    }
    let names = |word: &str| {
        let word = word.trim_start_matches('#');
        let by_number = word
            .parse::<usize>()
            .ok()
            .and_then(|n| ask.options.get(n.checked_sub(1)?));
        by_number.or_else(|| {
            ask.options.iter().find(|option| {
                option.id.as_str().eq_ignore_ascii_case(word)
                    || option.label.as_str().eq_ignore_ascii_case(word)
            })
        })
    };
    let mut words = reply
        .split(|c: char| c.is_whitespace() || ",.):;!?('’".contains(c))
        .filter(|word| !word.is_empty());
    let head = words.next()?;
    let rest = reply.strip_prefix(head)?;
    if !(rest.is_empty() || rest.starts_with([',', '.', ')', ':', ';', '!'])) {
        return None;
    }
    let pick = names(head)?;
    words
        .filter_map(names)
        .all(|other| other.id == pick.id)
        .then_some(pick)
}

/// The user's typed messages oldest first, `None` where a rewind left one off the live branch.
pub(crate) fn said(store: &yi_session::SharedSession) -> Vec<Option<String>> {
    let typed = crate::fetch::user_entries(store).unwrap_or_default();
    let query = yi_session::EntryQuery::default();
    let branch = yi_session::lock_session(store)
        .find_entries_on_branch("main", &query, &yi_session::BranchBounds::default())
        .unwrap_or_default();
    let live: HashSet<&str> = branch.iter().map(yi_types::entry::Entry::id).collect();
    typed
        .iter()
        .map(|(id, content)| {
            live.contains(id.as_str())
                .then(|| crate::rewind::user_text(content))
        })
        .collect()
}

pub(crate) fn stamp(ask: &mut Ask, said: &[Option<String>]) {
    ask.after = Some(said.len()).filter(|n| *n > 0).and_then(user_url);
}

/// The newest live reply since the ask decides, a pick or the user's own words; `Ok(None)` where
/// nobody types, as in a child, whose parent's mail answers unrecorded.
pub(crate) fn reply_to(
    ask: &Ask,
    label: &TodoLabel,
    said: &[Option<String>],
) -> Result<Option<Answer>, PlanIssue> {
    if said.is_empty() {
        return Ok(None);
    }
    let since = ask.after.as_ref().and_then(ordinal).unwrap_or(0);
    let reply = said
        .iter()
        .enumerate()
        .skip(since)
        .rev()
        .find_map(|(at, text)| Some((user_url(at.saturating_add(1))?, text.as_deref()?)));
    let (address, text) = reply.ok_or_else(|| unanswered(ask, label))?;
    Ok(Some(Answer {
        address,
        option: named(ask, text).map(|option| option.id.clone()),
        extra: Map::new(),
    }))
}

fn unanswered(ask: &Ask, label: &TodoLabel) -> PlanIssue {
    PlanIssue::Unanswered {
        label: label.clone(),
        options: ask.options.len(),
    }
}

fn asking<'a>(plan: &'a Plan, label: &TodoLabel) -> Option<&'a Ask> {
    let todo = plan.todo(label)?;
    let blocked = matches!(
        &todo.state,
        TodoState::Blocked {
            on: BlockedOn::User,
            ..
        }
    );
    todo.ask
        .as_ref()
        .filter(|ask| blocked && ask.answer.is_none())
}

impl PlanEngine {
    /// Stamps a new ask, answers an unblock of an open one from the user's reply, and refuses a
    /// `set` that would move an open ask off blocked around that reply.
    pub(super) fn ask_default(&self, mut op: Op, plan: Option<&PlanId>) -> Result<Op, PlanOpError> {
        let typed = || {
            let store = self.owner_words.as_ref().and_then(|words| words());
            store.map(|store| said(&store)).unwrap_or_default()
        };
        let current = || {
            let id = self.resolve(plan.cloned()).ok()?;
            self.store.read(&id).ok()
        };
        let invalid = |issue| PlanOpError::Invalid { issue };
        match &mut op {
            Op::Block { ask: Some(ask), .. } => stamp(ask, &typed()),
            Op::Unblock {
                label,
                answer: answer @ None,
            } => {
                let current = current();
                if let Some(ask) = current.as_ref().and_then(|plan| asking(plan, label)) {
                    *answer = reply_to(ask, label, &typed())
                        .map_err(invalid)?
                        .map(Box::new);
                }
            }
            Op::Set { rows, .. } => {
                let current = current();
                let moved = rows
                    .iter()
                    .filter(|row| row.state != TodoStateName::Blocked);
                for row in moved {
                    if let Some(ask) = current
                        .as_ref()
                        .and_then(|plan| asking(plan, &row.spec.label))
                    {
                        return Err(invalid(unanswered(ask, &row.spec.label)));
                    }
                }
            }
            _ => {}
        }
        Ok(op)
    }
}

/// Keeps the question on the todo past its unblock, and joins the reply's address to its intent.
pub(super) fn record<'a>(plan: &'a mut Plan, op: &Op) -> &'a mut Plan {
    let (label, ask, answer) = match op {
        Op::Block {
            label,
            ask: Some(ask),
            ..
        } => (label, Some(ask), None),
        Op::Unblock {
            label,
            answer: Some(answer),
        } => (label, None, Some(answer)),
        _ => return plan,
    };
    if let Some(todo) = plan.todos.iter_mut().find(|todo| &todo.label == label) {
        if let Some(ask) = ask {
            todo.ask = Some(Ask::clone(ask));
        }
        if let (Some(answer), Some(ask)) = (answer, todo.ask.as_mut()) {
            ask.answer = Some(Answer::clone(answer));
            if !todo.cites.intent.contains(&answer.address) {
                todo.cites.intent.push(answer.address.clone());
            }
        }
    }
    plan
}

pub(super) fn line(todo: &Todo) -> String {
    let Some(ask) = &todo.ask else {
        return String::new();
    };
    let Some(answer) = &ask.answer else {
        return format!(
            "; options {ask}; the user's next reply decides: opening with one option's number, id or label picks it, anything else is kept in their own words; unattended it stays blocked: nothing picks for the user"
        );
    };
    let rejected: Vec<&str> = ask
        .options
        .iter()
        .filter(|option| Some(&option.id) != answer.option.as_ref())
        .map(|option| option.id.as_str())
        .collect();
    match ask.picked() {
        Some(pick) => format!(
            " · picked {} {} by {}, the exemplar; rejected {}",
            pick.id,
            pick.label,
            answer.address,
            rejected.join(", ")
        ),
        None => format!(" · answered in the user's own words by {}", answer.address),
    }
}

/// A question with fewer options than the user is offered is asked open, and the reply says so.
pub(crate) fn few_options(args: &mut Map<String, Value>, said: &mut Vec<String>) {
    use yi_types::plan::ask::{OPTIONS_MAX, OPTIONS_MIN};
    let few = args.get("options").and_then(Value::as_array);
    if let Some(count) = few
        .map(Vec::len)
        .filter(|count| (1..OPTIONS_MIN).contains(count))
    {
        args.remove("options");
        said.push(format!(
            "a question offers {OPTIONS_MIN} to {OPTIONS_MAX} options and this one had {count}, so it is asked open"
        ));
    }
}
