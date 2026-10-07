//! The confirmed user path (plan sections 3.6 and 5.6): `Actor::User` is minted here and
//! nowhere else, for one request, bound to the plan, the op, its argument hash and revision.

//! Invariant: a submission arrives as its surface's own principal and never carries one; the
//! daemon fans a worker's ask to every attached client, so only a tty asker's process confirms.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};
use yi_session::{SharedSession, lock_session};
use yi_types::message::{AgentMessage, UserContent};
use yi_types::plan::canonical::canonical_digest;
use yi_types::plan::doc::{AgentId, Delegation, PlanId, TodoAddr, TouchCount};
use yi_types::plan::ledger::RequestId;
use yi_types::url::Url;

use super::ops::{Actor, Delegate, Op, OpRequest, Outcome, PlanEngine, PlanOpError};
use super::render::render_outcome;
use super::table::{check_actor, op_name};
use super::tool::{ArgError, request};
use crate::permission::{AskOutcome, PermissionAsk, PermissionBroker};

pub const CONFIRM_TTL: Duration = Duration::from_secs(300);

/// The process that owns the human's prompt: its broker asks, its transcript keeps the answer.
pub struct Confirmer {
    pub broker: Arc<PermissionBroker>,
    pub store: SharedSession,
}

pub struct Submission {
    pub args: Map<String, Value>,
    pub request_id: Option<RequestId>,
    pub expected_revision: Option<TouchCount>,
}

/// The session's own prompt and transcript, read when an acceptance asks: the store attaches
/// after the tools are wired.
pub struct Confirming {
    pub broker: Arc<PermissionBroker>,
    pub store: Arc<dyn Fn() -> Option<SharedSession> + Send + Sync>,
}

/// The plan tool's acceptance: with no session prompt wired, the refusal says no one can confirm.
pub(super) fn accept(
    engine: &PlanEngine,
    actor: &Actor,
    confirming: Option<&Confirming>,
    args: &Map<String, Value>,
) -> Result<Applied, SubmitError> {
    let confirmer = confirming.and_then(|confirming| {
        (confirming.store)().map(|store| Confirmer {
            broker: Arc::clone(&confirming.broker),
            store,
        })
    });
    let submission = Submission {
        args: args.clone(),
        request_id: None,
        expected_revision: None,
    };
    submit(engine, actor, confirmer.as_ref(), submission)
}

#[must_use]
#[derive(Debug)]
pub struct Applied {
    pub op: Op,
    pub outcome: Outcome,
}

impl Applied {
    pub fn text(&self) -> String {
        render_outcome(&self.op, &self.outcome)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SubmitError {
    #[error("{0}")]
    Arg(#[from] ArgError),
    #[error(
        "{op} needs the user's confirmation, and nothing in this session can ask for it: say in your answer what you ran and its exit line, and leave the todo open"
    )]
    NoConfirmer { op: &'static str },
    #[error("the user declined {op} on plan {plan}")]
    Declined { op: &'static str, plan: PlanId },
    #[error("the answer to {op} on plan {plan} came after {ttl_ms} ms; ask again")]
    Expired {
        op: &'static str,
        plan: PlanId,
        ttl_ms: u128,
    },
    #[error("the confirmation could not be cited: {0}")]
    Citation(String),
    #[error(transparent)]
    Op(#[from] PlanOpError),
}

/// Invariant: the peer is anything running as the user, so a submission carries no principal;
/// only a caller holding the human's prompt passes a confirmer.
pub fn submit_request(
    service: &super::PlanService,
    payload: &Map<String, Value>,
    confirmer: Option<&Confirmer>,
) -> Result<Value, String> {
    if payload.contains_key("actor") {
        return Err(ArgError::ActorArg.to_string());
    }
    let (engine, actor) = service
        .engine()
        .ok_or_else(|| "no plan engine is attached".to_owned())?;
    let applied =
        submit(&engine, &actor, confirmer, submission_of(payload)?).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "plan": applied.outcome.plan.id.as_str(),
        "revision": applied.outcome.plan.touched.0,
        "text": applied.text(),
        "notices": applied.outcome.notices,
    }))
}

/// A submission by a principal that may apply the op runs as that principal; one the owner may
/// not apply is confirmed by the human, or refused when no prompt can reach one.
pub fn submit(
    engine: &PlanEngine,
    actor: &Actor,
    confirmer: Option<&Confirmer>,
    submission: Submission,
) -> Result<Applied, SubmitError> {
    let Submission {
        args,
        request_id,
        expected_revision,
    } = submission;
    let mut request = request(actor, &args)?;
    request.request_id = request_id;
    request.expected_revision = expected_revision;
    let Err(refused) = check_actor(actor, &request.op) else {
        let op = request.op.clone();
        let outcome = engine.apply(request)?;
        return Ok(Applied { op, outcome });
    };
    if *actor != Actor::Owner {
        return Err(refused.into());
    }
    let op = op_name(request.op.kind());
    let Some(confirmer) = confirmer.filter(|confirmer| confirmer.broker.can_confirm()) else {
        return Err(SubmitError::NoConfirmer { op });
    };
    confirmed(engine, confirmer, request, op)
}

fn confirmed(
    engine: &PlanEngine,
    confirmer: &Confirmer,
    mut request: OpRequest,
    op: &'static str,
) -> Result<Applied, SubmitError> {
    let current = engine
        .apply(OpRequest {
            plan: request.plan.clone(),
            actor: Actor::Owner,
            op: Op::View { full: false },
            request_id: None,
            expected_revision: None,
        })?
        .plan;
    let revision = request.expected_revision.unwrap_or(current.touched);
    let args = request.op.args().map_err(PlanOpError::from)?;
    let args_hash = canonical_digest(&args).map_err(PlanOpError::from)?;
    let accepting = match &request.op {
        Op::Accept { label, note, .. } => Some(format!("{label}: {note}")),
        _ => None,
    };
    let description = format!(
        "{}{op} on plan {} at revision {} (args {args_hash}): an op the plan owner may not apply alone",
        accepting
            .as_deref()
            .map(|why| format!("{why} · "))
            .unwrap_or_default(),
        current.id,
        revision.0,
    );
    let cwd = engine.cwd.to_string_lossy();
    let call = accepting.as_deref().map(|display| crate::classifier::Call {
        tool: "plan",
        display,
        command: None,
        reason: "the plan owner asks to close a todo its check did not verify",
        cwd: &cwd,
    });
    let asked_at = Instant::now();
    let (answer, classified) = confirmer.broker.confirm_judged(
        &PermissionAsk {
            title: &format!("yi plan {op} asks for your confirmation"),
            description: &description,
            patch: None,
            changes: &[],
            // No standing grant for an administrative op: D193 wants a confirmed
            // channel per op, so "always" here is the one answer, not a rule kept.
            grants: &[],
            tool_call_id: None,
        },
        call.as_ref(),
    );
    if !matches!(answer, AskOutcome::AllowOnce | AskOutcome::AllowAlways(_)) {
        return Err(SubmitError::Declined {
            op,
            plan: current.id,
        });
    }
    if asked_at.elapsed() > CONFIRM_TTL {
        return Err(SubmitError::Expired {
            op,
            plan: current.id,
            ttl_ms: CONFIRM_TTL.as_millis(),
        });
    }
    request.actor = if classified {
        Actor::Classifier
    } else {
        Actor::User(cite(
            &confirmer.store,
            &format!("confirmed: {description}"),
        )?)
    };
    // Invariant: the answer binds the root the human saw; unnamed resolution runs before the
    // ask, never again after it.
    request.plan = Some(current.id.clone());
    request.expected_revision = Some(revision);
    if request.request_id.is_none() {
        request.request_id =
            RequestId::new(format!("confirm-{}", engine.store().request_nonce())).ok();
    }
    let op = request.op.clone();
    let outcome = engine.apply(request)?;
    Ok(Applied { op, outcome })
}

/// The answer crossed the process boundary: an attributed user message, cited by its ordinal.
fn cite(store: &SharedSession, text: &str) -> Result<Url, SubmitError> {
    lock_session(store)
        .append_message(
            "main",
            AgentMessage::user_input(UserContent::Text(text.to_owned()), yi_session::now_ms()),
        )
        .map_err(|error| SubmitError::Citation(error.to_string()))?;
    let held = crate::fetch::user_inputs(store)
        .map_err(SubmitError::Citation)?
        .len();
    format!("user://{held}")
        .parse()
        .map_err(|error: yi_types::url::UrlError| SubmitError::Citation(error.to_string()))
}

/// The wire shape of a `plan submit`: `{args, request_id?, expected_revision?}`, no actor.
pub fn submission_of(payload: &Map<String, Value>) -> Result<Submission, String> {
    let args = match payload.get("args") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(args)) => args.clone(),
        Some(other) => return Err(format!("plan submit args must be an object, got {other}")),
    };
    let request_id = match payload.get("request_id") {
        None | Some(Value::Null) => None,
        Some(Value::String(id)) => {
            Some(RequestId::new(id.as_str()).map_err(|error| error.to_string())?)
        }
        Some(other) => {
            return Err(format!(
                "plan submit request_id must be a string, got {other}"
            ));
        }
    };
    let expected_revision = match payload.get("expected_revision") {
        None | Some(Value::Null) => None,
        Some(Value::Number(number)) => {
            Some(TouchCount(number.as_u64().ok_or(
                "plan submit expected_revision must be a non-negative integer",
            )?))
        }
        Some(other) => {
            return Err(format!(
                "plan submit expected_revision must be an integer, got {other}"
            ));
        }
    };
    Ok(Submission {
        args,
        request_id,
        expected_revision,
    })
}

pub struct Unhosted;

impl Delegate for Unhosted {
    fn spawn(&self, at: &TodoAddr, _delegation: &Delegation) -> Result<AgentId, String> {
        Err(format!(
            "no session is running to host a child for {at}; start it from the session"
        ))
    }

    fn reap(&self, _agent: &AgentId, _supplied: &[Url]) -> Result<Option<Url>, String> {
        Ok(None)
    }

    fn hosts(&self) -> bool {
        false
    }
}

/// `<op> [<plan>] [<json args>]`, the one line the CLI reads; `fuse reset` spells `fuse_reset`.
/// The words name the op and the plan; every other argument is the JSON object the tool takes.
pub fn cli_args(line: &str) -> Result<Map<String, Value>, String> {
    let (mut op, mut rest) = split_word(line);
    if op == "fuse" {
        let (word, tail) = split_word(rest);
        if word != "reset" {
            return Err("usage: yi plan fuse reset [<plan>]".to_owned());
        }
        (op, rest) = ("fuse_reset", tail);
    }
    if op.is_empty() {
        return Err("usage: yi plan <op> [<plan>] [<json args>]".to_owned());
    }
    let mut args = Map::new();
    let mut plan = None;
    if !rest.is_empty() && !rest.starts_with('{') {
        let (word, tail) = split_word(rest);
        (plan, rest) = (Some(word), tail);
    }
    if !rest.is_empty() {
        match serde_json::from_str::<Value>(rest) {
            Ok(Value::Object(extra)) => args.extend(extra),
            Ok(other) => return Err(format!("args must be a JSON object, got {other}")),
            Err(error) => return Err(format!("args are not a JSON object: {error}")),
        }
    }
    args.insert("op".to_owned(), Value::String(op.to_owned()));
    if let Some(plan) = plan {
        args.insert("plan".to_owned(), Value::String(plan.to_owned()));
    }
    Ok(args)
}

fn split_word(text: &str) -> (&str, &str) {
    let text = text.trim_start();
    text.split_once(char::is_whitespace)
        .map_or((text, ""), |(word, tail)| (word, tail.trim_start()))
}
