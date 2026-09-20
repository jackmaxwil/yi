//! F1a, the source record (plan sections 5.4 and 8.3): a cell's source is frozen as an
//! artifact, journaled before the cell's first effect, exported to `program.py`, and never run.

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use yi_kernel::client::HostHandlers;
use yi_runtime::HostRegistry;
use yi_runtime::plan::ops::{Actor, Delegate, PlanEngine};
use yi_runtime::plan::program::PROGRAM_NAME;
use yi_runtime::plan::store::PlanStore;
use yi_types::plan::canonical::{ArtifactRef, Digest};
use yi_types::plan::doc::{AgentId, Delegation, PlanId, TodoAddr, TodoLabel};
use yi_types::url::Url;

type Fallible = Result<(), Box<dyn std::error::Error>>;

#[derive(Default)]
struct Counting(AtomicUsize);

impl Delegate for Counting {
    fn spawn(&self, at: &TodoAddr, _delegation: &Delegation) -> Result<AgentId, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        AgentId::new(format!("kid-{}", at.todo.as_str())).map_err(|error| error.to_string())
    }

    fn reap(&self, _agent: &AgentId, _supplied: &[Url]) -> Result<Option<Url>, String> {
        Ok(None)
    }

    fn follow_up(&self, _dispatched: &[TodoLabel], _held: usize) {}
}

const SOURCE: &str = "import os\nos.remove('never-run')\nplan = await Plan.create('ship it')";

async fn op(
    registry: &HostRegistry,
    id: &str,
    name: &str,
    args: Value,
    artifacts: Value,
) -> Result<Value, Box<dyn std::error::Error>> {
    let payload = json!({"request_id": id, "op": name, "args": args, "artifacts": artifacts});
    let payload = payload.as_object().cloned().ok_or("payload")?;
    let reply = registry.dispatch("plan.op", payload).ok_or("plan.op")?;
    Ok(Value::Object(reply.await?))
}

/// Dies with the control: drop the store read and the unstored source is journaled; drop
/// the export and `program.py` is empty; run the source and `never-run` is asked for.
#[tokio::test]
async fn program_records_source_before_the_first_effect() -> Fallible {
    let dir = Scratch::new("yi-plan-program")?;
    let store = PlanStore::open(dir.to_path_buf())?;
    let delegate = Arc::new(Counting::default());
    let engine = Arc::new(PlanEngine::new(store.clone(), delegate.clone()));
    let mut registry = HostRegistry::default();
    yi_runtime::plan::request::register(Arc::clone(&engine), Actor::Owner, &mut registry);
    let worker = json!({"spec": {"role": "writer"}, "accept": {"stated": "it works"}});
    let todos = json!([{"label": "build", "delegation": worker}]);
    let opened = op(
        &registry,
        "r1",
        "init",
        json!({"goal": "ship it", "todos": todos}),
        json!([]),
    )
    .await?;
    let id = PlanId::new(
        opened["plan"]["plan"]
            .as_str()
            .ok_or_else(|| opened.to_string())?,
    )?;
    let source = ArtifactRef {
        digest: Digest::of(SOURCE.as_bytes()),
        media_type: "text/x-python".to_owned(),
        length: u64::try_from(SOURCE.len())?,
        provenance: None,
    };
    let args = json!({"cell_id": "cell-1", "source_ref": serde_json::to_value(&source)?});

    let unstored = op(&registry, "r2", "program", args.clone(), json!([])).await?;
    assert_eq!(unstored["refusal"]["kind"], "program", "{unstored}");
    let blob = json!([{"media_type": "text/x-python", "text": SOURCE}]);
    let recorded = op(&registry, "r3", "program", args.clone(), blob.clone()).await?;
    assert_eq!(recorded["ok"], true, "{recorded}");
    assert_eq!(
        delegate.0.load(Ordering::SeqCst),
        0,
        "a record is not an effect"
    );

    let path = store.plan_dir(&id).join(PROGRAM_NAME);
    let text = std::fs::read_to_string(&path)?;
    assert!(text.starts_with("# --- cell cell-1 20"), "{text}");
    assert!(text.ends_with(&format!("Z\n{SOURCE}\n")), "{text}");
    let journal = store.journal(&id).read()?;
    let last = journal.records.last().ok_or("no record")?;
    assert_eq!(last.record.op, "program");
    assert_eq!(last.program_hash, Some(Digest::of(text.as_bytes())));
    assert!(
        !last.args.to_string().contains("never-run"),
        "the journal names the source, never holds it"
    );

    let replayed = op(&registry, "r3", "program", args.clone(), blob.clone()).await?;
    assert_eq!(replayed["ok"], true, "{replayed}");
    let twice = op(&registry, "r4", "program", args, blob).await?;
    assert_eq!(
        twice["refusal"]["kind"], "program",
        "a cell id is recorded once: {twice}"
    );
    assert_eq!(
        std::fs::read_to_string(&path)?,
        text,
        "nothing rewrites or repeats a cell"
    );

    let started = op(
        &registry,
        "r5",
        "start",
        json!({"label": "build"}),
        json!([]),
    )
    .await?;
    assert_eq!(started["ok"], true, "{started}");
    assert_eq!(delegate.0.load(Ordering::SeqCst), 1);
    let kinds: Vec<String> = store
        .journal(&id)
        .read()?
        .records
        .iter()
        .filter(|record| !record.is_refusal())
        .map(|record| record.record.op.clone())
        .collect();
    let at = |kind: &str| kinds.iter().position(|op| op == kind);
    assert!(at("program") < at("spawn_intent"), "{kinds:?}");
    assert!(!dir.join("never-run").exists() && !std::path::Path::new("never-run").exists());
    Ok(())
}
