//! A todo blocked on the user with 3 to 5 options; only the user's reply picks one.

use serde_json::{Map, Value};
use yi_types::plan::ask::{Answer, Ask, AskError, AskOption};
use yi_types::plan::doc::{BlockedOn, Plan, PlanId, PlanIssue, Todo, TodoState};
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

/// The option a reply names by its number, its id or its whole label; the first word decides,
/// so "B, but less copy" picks B.
fn named<'a>(ask: &'a Ask, reply: &str) -> Option<&'a AskOption> {
    let reply = reply.trim();
    let head = reply
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | '.' | ')' | ':' | ';'))
        .next()?
        .trim_start_matches('#');
    let by_number = head
        .parse::<usize>()
        .ok()
        .and_then(|n| n.checked_sub(1))
        .and_then(|at| ask.options.get(at));
    let by_name = ask.options.iter().find(|option| {
        option.id.as_str().eq_ignore_ascii_case(head)
            || option.label.as_str().eq_ignore_ascii_case(reply)
    });
    by_name.or(by_number)
}

fn asking<'a>(plan: &'a Plan, label: &yi_types::plan::doc::TodoLabel) -> Option<&'a Ask> {
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
    fn said(&self) -> Vec<String> {
        let typed = self
            .owner_words
            .as_ref()
            .and_then(|words| words())
            .and_then(|store| crate::fetch::user_entries(&store).ok());
        typed
            .unwrap_or_default()
            .iter()
            .map(|(_, content)| crate::rewind::user_text(content))
            .collect()
    }

    /// Stamps a new ask with the newest user message, and an unblock of an open ask with the
    /// reply after it: the newest that names an option, else the newest in the user's own words.
    pub(super) fn ask_default(&self, mut op: Op, plan: Option<&PlanId>) -> Result<Op, PlanOpError> {
        match &mut op {
            Op::Block { ask: Some(ask), .. } => {
                ask.after = Some(self.said().len())
                    .filter(|n| *n > 0)
                    .and_then(user_url);
            }
            Op::Unblock {
                label,
                answer: answer @ None,
            } => {
                let current = self.resolve(plan.cloned()).ok();
                let current = current.and_then(|id| self.store.read(&id).ok());
                let Some(ask) = current.as_ref().and_then(|plan| asking(plan, label)) else {
                    return Ok(op);
                };
                let since = ask.after.as_ref().and_then(ordinal).unwrap_or(0);
                let replies: Vec<(usize, String)> =
                    self.said().into_iter().enumerate().skip(since).collect();
                let picked = replies.iter().rev().find_map(|(at, reply)| {
                    named(ask, reply).map(|option| (*at, Some(option.id.clone())))
                });
                let reply = picked.or_else(|| replies.last().map(|(at, _)| (*at, None)));
                let address =
                    reply.and_then(|(at, option)| Some((user_url(at.saturating_add(1))?, option)));
                let Some((address, option)) = address else {
                    return Err(PlanOpError::Invalid {
                        issue: PlanIssue::Unanswered {
                            label: label.clone(),
                            options: ask.options.len(),
                        },
                    });
                };
                *answer = Some(Box::new(Answer {
                    address,
                    option,
                    extra: Map::new(),
                }));
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
            "; options {ask}; the user's reply picks by number, id or label, and unattended it stays blocked: nothing picks for the user"
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
