use serde_json::{Map, Value, json};
use yi_types::plan::canonical::{ArtifactRef, Digest, canonical_bytes};
use yi_types::plan::contract::MANIFEST_FORMAT;
use yi_types::plan::ids::INLINE_NOTE_MAX_BYTES;

use yi_types::plan::doc::{PlanId, PlanState, Todo, TodoLabel, TodoState};

use super::ops::{Actor, Op, OpRequest, Outcome, PlanEngine, PlanOpError};

const CHECK_TIMEOUT_MS: u64 = 60_000;
pub(super) const NOTE_REF: &str = "note_ref";

pub struct Blob {
    pub media_type: &'static str,
    pub bytes: Vec<u8>,
}

fn stored(bytes: Vec<u8>, media_type: &'static str, blobs: &mut Vec<Blob>) -> Option<Value> {
    let reference = ArtifactRef {
        digest: Digest::of(&bytes),
        media_type: media_type.to_owned(),
        length: u64::try_from(bytes.len()).ok()?,
        provenance: None,
    };
    blobs.push(Blob { media_type, bytes });
    serde_json::to_value(reference).ok()
}

fn cmd_item(item: &mut Value, blobs: &mut Vec<Blob>) -> Option<()> {
    let cmd = item.get_mut("decider")?.get_mut("cmd")?;
    if let Value::String(command) = cmd {
        *cmd = json!({ "checker": command.clone() });
    }
    let cmd = cmd.as_object_mut()?;
    let timeout = cmd
        .entry("timeout_ms")
        .or_insert(json!(CHECK_TIMEOUT_MS))
        .as_u64()?;
    let command = cmd.get("checker")?.as_str()?.to_owned();
    let manifest = json!({
        "manifest": MANIFEST_FORMAT, "command": command, "cwd": "snapshot_root",
        "cwd_subdir": null, "protected": [], "timeout_ms": timeout, "env": [],
        "reads_outside_snapshot": false,
    });
    let bytes = canonical_bytes(&manifest).ok()?;
    cmd.insert(
        "checker".to_owned(),
        stored(bytes, "application/json", blobs)?,
    );
    Some(())
}

fn worktree_command(todo: &Map<String, Value>) -> Option<String> {
    let delegation = todo.get("delegation")?;
    (delegation.pointer("/spec/isolation")?.as_str()? == "worktree").then_some(())?;
    Some(delegation.pointer("/accept/command")?.as_str()?.to_owned())
}

fn stated_worktree(todo: &Map<String, Value>) -> bool {
    todo.get("delegation").is_some_and(|delegation| {
        delegation
            .pointer("/spec/isolation")
            .and_then(Value::as_str)
            == Some("worktree")
            && delegation.pointer("/accept/stated").is_some()
    })
}

fn delegation_of(delegation: &mut Map<String, Value>, blobs: &mut Vec<Blob>) -> Result<(), String> {
    if let Some(isolation) = delegation.remove("isolation") {
        let spec = delegation.entry("spec").or_insert_with(|| json!({}));
        match spec.as_object_mut() {
            Some(spec) if spec.get("isolation").is_none_or(|inner| *inner == isolation) => {
                spec.insert("isolation".to_owned(), isolation);
            }
            _ => return Err("isolation is named twice, on the delegation and in its spec, with different values; keep the one in spec".to_owned()),
        }
    }
    let Some(Value::String(note)) = delegation.get("note") else {
        return Ok(());
    };
    if note.len() <= INLINE_NOTE_MAX_BYTES {
        return Ok(());
    }
    let mut cut = INLINE_NOTE_MAX_BYTES.saturating_sub(64);
    while !note.is_char_boundary(cut) {
        cut = cut.saturating_sub(1);
    }
    let head = format!(
        "{}... (the whole note is linked below)",
        note.get(..cut).unwrap_or_default()
    );
    let reference = stored(note.clone().into_bytes(), "text/markdown", blobs);
    delegation.insert("note".to_owned(), Value::String(head));
    if let Some(reference) = reference {
        delegation.insert(NOTE_REF.to_owned(), reference);
    }
    Ok(())
}

fn todo_of(todo: &mut Map<String, Value>, blobs: &mut Vec<Blob>) -> Result<(), String> {
    if !todo.contains_key("delegation")
        && let Some(delegation) = todo.remove("delegate")
    {
        todo.insert("delegation".to_owned(), delegation);
    }
    let loose = todo
        .get("delegation")
        .is_some_and(|delegation| delegation.get("accept").is_none());
    let accept = if loose { todo.remove("accept") } else { None };
    if let Some(Value::Object(delegation)) = todo.get_mut("delegation") {
        if let Some(accept) = accept {
            delegation.insert("accept".to_owned(), accept);
        }
        delegation_of(delegation, blobs)?;
    }
    let uncontracted = todo.get("contract").is_none_or(Value::is_null);
    if uncontracted && stated_worktree(todo) {
        let label = todo
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or_default();
        return Err(format!(
            r#"todo {label}: a stated accept gives the engine nothing to run on a worktree child's checkout; write it as a command, {{"command": "<shell check>"}} (for example {{"command": "test -s out.txt"}}), or declare a `contract`"#
        ));
    }
    if uncontracted && let Some(command) = worktree_command(todo) {
        let cmd = json!({"checker": command, "timeout_ms": crate::goal::DEFAULT_CHECK_TIMEOUT_MS});
        let item = json!({"id": "accept", "critical": true, "weight": 1, "decider": {"cmd": cmd}});
        todo.insert(
            "contract".to_owned(),
            json!({"class": "writer", "items": [item]}),
        );
    }
    if let Some(items) = todo
        .get_mut("contract")
        .and_then(|contract| contract.get_mut("items"))
    {
        for item in items.as_array_mut().into_iter().flatten() {
            cmd_item(item, blobs);
        }
    }
    Ok(())
}

pub fn normalize(args: &Map<String, Value>) -> Result<(Map<String, Value>, Vec<Blob>), String> {
    let mut args = args.clone();
    let mut blobs = Vec::new();
    if let Some(Value::Array(todos)) = args.get_mut("todos") {
        for todo in todos.iter_mut().filter_map(Value::as_object_mut) {
            todo_of(todo, &mut blobs)?;
        }
    }
    if let Some(Value::Object(delegation)) = args.get_mut("delegation") {
        delegation_of(delegation, &mut blobs)?;
    }
    Ok((args, blobs))
}

fn standing_of(todo: &Todo) -> String {
    match &todo.state {
        TodoState::Pending => {
            "pending; the engine starts it once its edges clear and a slot is free".to_owned()
        }
        TodoState::Running { by } => {
            format!("running by {by}; the engine submits and verifies its finish")
        }
        TodoState::Blocked { .. } => "blocked; unblock hands it back to the engine".to_owned(),
        TodoState::Failed { cause, .. } => format!("failed ({cause}); retry or fail are yours"),
        TodoState::Done { .. } => "done: the engine accepted it".to_owned(),
        other => yi_types::plan::doc::TodoStateName::of(other).to_string(),
    }
}

impl PlanEngine {
    fn engine_steps(&self, request: &OpRequest) -> Option<Result<Outcome, PlanOpError>> {
        let verb = match &request.op {
            Op::Start { .. } => "starts",
            Op::Submit { .. } if self.delegate.finishes() => "submits",
            Op::Done { .. } if self.delegate.finishes() => "accepts",
            _ => return None,
        };
        let label = request
            .op
            .label()
            .filter(|_| request.actor == Actor::Owner)?;
        let family = |root: &PlanId| self.family(root);
        let (id, family) = match &request.plan {
            Some(id) => (id.clone(), family(&super::state::root_of(id).ok()?)?),
            None => self.owner_root(label)?,
        };
        let plan = family.plan(&id).ok()?;
        let todo = plan
            .todo(label)
            .filter(|_| matches!(plan.state, PlanState::Active | PlanState::Done))?;
        todo.delegation.as_ref()?;
        if let TodoState::Running { by } = &todo.state
            && self.liveness.alive(by) == Some(false)
        {
            return None;
        }
        let intent = family.intent_for(&id, label);
        if intent.is_some_and(|(_, intent)| self.stranded(&intent.outcome)) {
            return None;
        }
        let note = format!(
            "nothing to do: the engine {verb} delegated todos; {:?} is {}",
            label.as_str(),
            standing_of(todo)
        );
        Some(self.view(Some(id), false).map(|mut outcome| {
            outcome.notices.insert(0, note);
            outcome
        }))
    }

    /// With no plan open, the closed plan holding the label: the engine closes one at its last accept.
    fn owner_root(&self, label: &TodoLabel) -> Option<(PlanId, super::state::RootState)> {
        let mut closed = None;
        for root in self.roots().ok()? {
            let Some(family) = self.family(&root) else {
                continue;
            };
            let Ok(plan) = family.plan(&root) else {
                continue;
            };
            let delegated = plan
                .todo(label)
                .is_some_and(|todo| todo.delegation.is_some());
            match plan.state {
                PlanState::Active => return Some((root, family)),
                PlanState::Done if delegated && closed.is_none() => closed = Some((root, family)),
                _ => {}
            }
        }
        closed
    }

    pub(super) fn apply_with(
        &self,
        request: OpRequest,
        blobs: &[Blob],
    ) -> Result<Outcome, PlanOpError> {
        let dispatches = self.delegate.hosts()
            && !matches!(request.op, Op::View { .. })
            && !matches!(request.actor, Actor::Child(_));
        if dispatches && let Some(answer) = self.engine_steps(&request) {
            return answer;
        }
        let mut outcome = self.apply_once(request)?;
        if let Err(error) = self.keep(&outcome, blobs) {
            outcome.notices.push(format!(
                "the op is recorded, but a checker or note it names was not stored: {error}"
            ));
        }
        if dispatches {
            self.dispatch_ready(&mut outcome);
        }
        Ok(outcome)
    }

    pub(super) fn latest(&self) -> Option<yi_types::plan::doc::PlanId> {
        let written = |id: &yi_types::plan::doc::PlanId| {
            std::fs::metadata(self.store.path(id))
                .and_then(|meta| meta.modified())
                .ok()
        };
        let roots = self.roots().ok()?;
        roots
            .into_iter()
            .filter(|id| self.store.read(id).is_ok())
            .max_by_key(written)
    }

    pub(super) fn keep(&self, outcome: &Outcome, blobs: &[Blob]) -> Result<(), PlanOpError> {
        for id in std::iter::once(&outcome.plan.id).chain(&outcome.subplan) {
            let artifacts = self.store.artifacts(id);
            for blob in blobs {
                artifacts
                    .put(&blob.bytes, blob.media_type, &self.store.nonce())
                    .map_err(super::store::StoreError::from)?;
            }
        }
        Ok(())
    }
}
