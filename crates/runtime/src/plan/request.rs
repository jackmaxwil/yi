//! `plan.op`: the plan engine as one host request over the tool's parser (D-next-1).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use yi_types::plan::canonical::{Digest, canonical_digest};
use yi_types::plan::doc::{PlanId, TouchCount};
use yi_types::plan::ledger::RequestId;

use super::ops::{Actor, Op, PlanEngine, PlanOpError};
use super::table::OpKind;
use super::tool::{PlanToolError, render_outcome, request};

/// ponytail: a linear scan over 64 replies keyed on request id and args digest, holding only
/// refusals that never reached the journal (a refused parse or actor); the journal replays the rest.
const REPLAY_CAP: usize = 64;

type Replay = Arc<Mutex<VecDeque<(String, Digest, Map<String, Value>)>>>;

/// The two refusals the engine never journals, so the ring is their only memory.
const RING_CODES: [&str; 2] = ["bad_args", "not_owner"];

/// Blobs one request may carry into the plan's store: a contract's criteria for every item
/// (two for an `example`), a product and a cell's source.
const ARTIFACTS_MAX: usize = 2 * yi_types::plan::contract::ITEMS_MAX + 2;
const ARTIFACT_MAX_BYTES: usize = 1024 * 1024;

/// A top-level `plan` merges into `args`; the guard reads the plan back from the parsed request.
struct Payload {
    request_id: String,
    expected_revision: Option<u64>,
    args: Map<String, Value>,
    /// `(media_type, text)` blobs the op's args name by digest, stored before the op applies.
    artifacts: Vec<(String, String)>,
}

fn parse_artifacts(payload: &Map<String, Value>) -> Result<Vec<(String, String)>, String> {
    let list = match payload.get("artifacts") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(list)) => list,
        Some(other) => return Err(format!("plan.op artifacts must be a list, got {other}")),
    };
    if list.len() > ARTIFACTS_MAX {
        return Err(format!(
            "plan.op carries {} artifacts; the cap is {ARTIFACTS_MAX}",
            list.len()
        ));
    }
    list.iter()
        .map(|entry| {
            let field = |name: &str| entry.get(name).and_then(Value::as_str);
            let (Some(media_type), Some(text)) = (field("media_type"), field("text")) else {
                return Err("plan.op artifacts are {media_type, text} strings".to_owned());
            };
            if text.len() > ARTIFACT_MAX_BYTES {
                return Err(format!(
                    "plan.op artifact of {} bytes; the cap is {ARTIFACT_MAX_BYTES}",
                    text.len()
                ));
            }
            Ok((media_type.to_owned(), text.to_owned()))
        })
        .collect()
}

fn parse_payload(payload: &Map<String, Value>) -> Result<Payload, String> {
    let request_id = payload
        .get("request_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or("plan.op requires a request_id")?
        .to_owned();
    let op = payload
        .get("op")
        .and_then(Value::as_str)
        .ok_or("plan.op requires an op name")?;
    let plan = match payload.get("plan") {
        None | Some(Value::Null) => None,
        Some(Value::String(id)) => Some(PlanId::new(id).map_err(|error| error.to_string())?),
        Some(other) => return Err(format!("plan.op plan must be a string, got {other}")),
    };
    let expected_revision = match payload.get("expected_revision") {
        None | Some(Value::Null) => None,
        Some(Value::Number(number)) => Some(
            number
                .as_u64()
                .ok_or("plan.op expected_revision must be a non-negative integer")?,
        ),
        Some(other) => {
            return Err(format!(
                "plan.op expected_revision must be an integer, got {other}"
            ));
        }
    };
    let mut args = match payload.get("args") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(args)) => args.clone(),
        Some(other) => return Err(format!("plan.op args must be an object, got {other}")),
    };
    args.insert("op".to_owned(), Value::String(op.to_owned()));
    if let Some(id) = &plan {
        args.insert("plan".to_owned(), Value::String(id.as_str().to_owned()));
    }
    Ok(Payload {
        request_id,
        expected_revision,
        args,
        artifacts: parse_artifacts(payload)?,
    })
}

fn refusal(
    request_id: &str,
    revision: Option<u64>,
    code: &str,
    message: String,
) -> Map<String, Value> {
    let mut reply = Map::new();
    reply.insert("ok".to_owned(), Value::Bool(false));
    reply.insert(
        "request_id".to_owned(),
        Value::String(request_id.to_owned()),
    );
    reply.insert(
        "revision".to_owned(),
        revision.map_or(Value::Null, Value::from),
    );
    reply.insert(
        "refusal".to_owned(),
        json!({"code": code, "message": message}),
    );
    reply
}

fn code_of(error: &PlanToolError) -> &'static str {
    match error {
        PlanToolError::Arg(_) => "bad_args",
        PlanToolError::Op(
            error @ (PlanOpError::NotOwner { .. }
            | PlanOpError::StaleRevision { .. }
            | PlanOpError::RequestIdReused { .. }),
        ) => error.code(),
        PlanToolError::Op(_) => "refused",
    }
}

const OWNER_ONLY: &str = "only the plan owner stores artifacts";

/// Invariant: a child writes one blob, the product of the attempt the plan says it is running.
fn own_product(
    engine: &PlanEngine,
    actor: &Actor,
    id: &PlanId,
    op: &Op,
    artifacts: &[(String, String)],
) -> Result<(), String> {
    let (Actor::Child(agent), Op::Submit { label, attempt, .. }) = (actor, op) else {
        return Err(OWNER_ONLY.to_owned());
    };
    if artifacts.len() != 1 {
        return Err(format!(
            "{OWNER_ONLY}; a submit carries the one product it cites"
        ));
    }
    let plan = engine.store().read(id).map_err(|error| error.to_string())?;
    let running = plan.todo(label).is_some_and(|todo| {
        todo.attempt == *attempt
            && matches!(&todo.state, yi_types::plan::doc::TodoState::Running { by } if super::ops::runs(actor, by))
    });
    if !running {
        return Err(format!(
            "{OWNER_ONLY}; {label:?} is not running by {agent} on attempt {}",
            attempt.get()
        ));
    }
    Ok(())
}

/// Invariant: only the owner writes blobs, into a plan that already exists, and the store names
/// each by its own digest: an op that cites a digest the bytes do not have finds nothing.
fn store_artifacts(
    engine: &PlanEngine,
    actor: &Actor,
    plan: Option<PlanId>,
    op: &Op,
    artifacts: &[(String, String)],
) -> Result<(), String> {
    if artifacts.is_empty() {
        return Ok(());
    }
    let id = engine.resolve(plan).map_err(|error| error.to_string())?;
    if *actor != Actor::Owner {
        own_product(engine, actor, &id, op, artifacts)?;
    }
    let store = engine.store();
    for (media_type, text) in artifacts {
        store
            .artifacts(&id)
            .put(text.as_bytes(), media_type, &store.nonce())
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// What a caller branches on: the engine's own code, the first refusal's on a replay, and
/// the verdict a refused `done` carries.
fn detail_of(error: &PlanToolError, refusal: &mut Map<String, Value>) {
    let PlanToolError::Op(error) = error else {
        return;
    };
    let kind = match error {
        PlanOpError::RecordedRefusal { code, .. } => code.clone(),
        other => other.code().to_owned(),
    };
    refusal.insert("kind".to_owned(), Value::String(kind));
    if let PlanOpError::Refused { verdict, .. } = error
        && let Ok(verdict) = serde_json::to_value(verdict)
    {
        refusal.insert("verdict".to_owned(), verdict);
    }
}

/// `view` reads and `init` opens, so neither has a revision to compare.
fn guarded(op: &Op) -> bool {
    !matches!(op.kind(), OpKind::View | OpKind::Init)
}

fn answer(engine: &PlanEngine, actor: &Actor, payload: Payload) -> Map<String, Value> {
    let Payload {
        request_id,
        expected_revision,
        args,
        artifacts,
    } = payload;
    let mut request = match request(actor, &args) {
        Ok(request) => request,
        Err(error) => {
            let error = PlanToolError::from(error);
            return refusal(&request_id, None, code_of(&error), error.to_string());
        }
    };
    let plan = request.plan.clone();
    // Invariant: a caller that brought an id keeps it or is refused; the engine mints one only
    // for a caller that brought none, so the journal's key is always the caller's own.
    request.request_id = match RequestId::new(request_id.as_str()) {
        Ok(id) => Some(id),
        Err(error) => {
            return refusal(
                &request_id,
                None,
                "bad_args",
                format!("request_id: {error}"),
            );
        }
    };
    if guarded(&request.op) {
        request.expected_revision = expected_revision.map(TouchCount);
    }
    let op = request.op.clone();
    if let Err(message) = store_artifacts(engine, actor, plan.clone(), &op, &artifacts) {
        // The rider refuses the principal, not the arguments, so a caller that may not write
        // blobs reads the same code the step table gives it and can stop asking.
        let code = if message.starts_with(OWNER_ONLY) {
            "not_owner"
        } else {
            "bad_args"
        };
        return refusal(&request_id, None, code, message);
    }
    match engine.apply(request) {
        Ok(outcome) => {
            let mut reply = Map::new();
            reply.insert("ok".to_owned(), Value::Bool(true));
            reply.insert("request_id".to_owned(), Value::String(request_id));
            reply.insert("revision".to_owned(), Value::from(outcome.plan.touched.0));
            reply.insert(
                "text".to_owned(),
                Value::String(render_outcome(&op, &outcome)),
            );
            // The typed result (plan section 4.3): a program reads state, never the text.
            reply.insert(
                "plan".to_owned(),
                super::plan_json(&outcome.plan).unwrap_or(Value::Null),
            );
            reply.insert("notices".to_owned(), json!(outcome.notices));
            reply
        }
        Err(error) => {
            let revision = match &error {
                PlanOpError::StaleRevision { current, .. } => Some(*current),
                _ => engine.revision(plan).ok().map(|touched| touched.0),
            };
            let error = PlanToolError::from(error);
            let mut reply = refusal(&request_id, revision, code_of(&error), error.to_string());
            if let Some(Value::Object(refusal)) = reply.get_mut("refusal") {
                detail_of(&error, refusal);
            }
            reply
        }
    }
}

fn replayed(replay: &Replay, request_id: &str, args: &Digest) -> Option<Map<String, Value>> {
    replay
        .lock()
        .ok()?
        .iter()
        .find(|(id, digest, _)| id == request_id && digest == args)
        .map(|(_, _, reply)| reply.clone())
}

fn remember(replay: &Replay, request_id: String, args: Digest, reply: &Map<String, Value>) {
    let code = reply
        .get("refusal")
        .and_then(|refusal| refusal.get("code"))
        .and_then(Value::as_str);
    if !code.is_some_and(|code| RING_CODES.contains(&code)) {
        return;
    }
    if let Ok(mut kept) = replay.lock() {
        if kept.len() >= REPLAY_CAP {
            kept.pop_front();
        }
        kept.push_back((request_id, args, reply.clone()));
    }
}

/// The principal is fixed here (`Owner` on the root kernel, `Child(name)` on a child's),
/// never read from a payload.
pub fn register(engine: Arc<PlanEngine>, actor: Actor, registry: &mut crate::kernel::HostRegistry) {
    let replay: Replay = Arc::new(Mutex::new(VecDeque::new()));
    registry.register("plan.op", move |payload| {
        let engine = Arc::clone(&engine);
        let actor = actor.clone();
        let replay = Arc::clone(&replay);
        Box::pin(async move {
            let payload = parse_payload(&payload)?;
            let args = canonical_digest(&Value::Object(payload.args.clone()))
                .map_err(|error| error.to_string())?;
            if let Some(reply) = replayed(&replay, &payload.request_id, &args) {
                return Ok(reply);
            }
            let request_id = payload.request_id.clone();
            let reply = tokio::task::spawn_blocking(move || answer(&engine, &actor, payload))
                .await
                .map_err(|error| format!("plan.op task failed: {error}"))?;
            remember(&replay, request_id, args, &reply);
            Ok(reply)
        })
    });
}

#[cfg(test)]
pub(super) fn refusal_of(actor: &Actor, args: &Map<String, Value>) -> Map<String, Value> {
    match request(actor, args) {
        Ok(_) => Map::new(),
        Err(error) => {
            let error = PlanToolError::from(error);
            refusal("test", None, code_of(&error), error.to_string())
        }
    }
}
