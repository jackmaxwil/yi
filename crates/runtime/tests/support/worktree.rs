//! An engine over a scratch repository with several worktree todos, each child a branch cut
//! from the parent's head, and one frozen `cmd` checker: what the acceptance tests drive.

use std::collections::HashMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use yi_runtime::lane::settle::{Candidate, Held, Quiescence};
use yi_runtime::lane::{BranchName, Pool, Sha};
use yi_runtime::plan::ops::{Actor, Delegate, Op, OpRequest, Outcome, PlanEngine, PlanOpError};
use yi_runtime::plan::output::OutputResolve;
use yi_runtime::plan::store::PlanStore;
use yi_runtime::plan::verify::Verifier;
use yi_types::plan::contract::Contract;
use yi_types::plan::doc::{AgentId, Delegation, GoalText, PlanId, TodoAddr, TodoLabel, TodoState};
use yi_types::plan::ledger::AttemptId;
use yi_types::plan::op::{Choice, TodoSpec};
use yi_types::url::Url;

use crate::scratch::Scratch;

const GOAL: &str = "land the children";

pub fn git(cwd: &Path, args: &[&str]) -> Result<String, Box<dyn Error>> {
    let output = yi_tools::command("git")
        .current_dir(cwd)
        .args(args)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git {}: {stderr}", args.join(" ")).into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Each child is a fixture branch: no process, and a candidate the engine reads by agent id,
/// its `local://` output read in the parent checkout, as the parent's own resolver reads it.
struct Children(HashMap<String, Candidate>, PathBuf);

impl OutputResolve for Children {
    fn resolve(&self, url: &Url) -> Result<Option<String>, String> {
        std::fs::read_to_string(self.1.join(url.path().trim_start_matches('/')))
            .map(Some)
            .map_err(|error| error.to_string())
    }
}

impl Delegate for Children {
    fn spawn(&self, at: &TodoAddr, _delegation: &Delegation) -> Result<AgentId, String> {
        AgentId::new(format!("child-{}", at.todo)).map_err(|error| error.to_string())
    }

    fn reap(&self, _agent: &AgentId, _supplied: &[Url]) -> Result<Option<Url>, String> {
        Ok(None)
    }

    fn candidate(&self, agent: &AgentId) -> Result<Option<Held>, String> {
        let Some(candidate) = self.0.get(agent.as_str()) else {
            return Ok(None);
        };
        Ok(Some(Held {
            candidate: candidate.clone(),
            quiescence: Quiescence {
                at: 0,
                running: Vec::new(),
            },
            path: self.1.clone(),
        }))
    }

    fn mark(&self, _agent: &AgentId, _choice: Choice) -> Result<(), String> {
        Ok(())
    }
}

pub struct Bench {
    pub engine: PlanEngine,
    pub store: PlanStore,
    pub plan: PlanId,
    _root: Scratch,
}

/// What the fixture varies: the parent gains an unrelated commit after the children branch off
/// it, the checker declares that it reads outside the snapshot, or the parent serves no output.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Unmoved,
    Moved,
    ReadsOutside,
    Unserved,
}

/// One worktree todo per label over a pool of `slots`, every contract the one `check`.
pub fn bench(
    name: &str,
    labels: &[&str],
    slots: u8,
    check: &str,
    shape: Shape,
) -> Result<Bench, Box<dyn Error>> {
    let root = Scratch::new(&format!("yi-accept-{name}"))?;
    let parent = root.join("parent");
    crate::repo::init_repo(&parent)?;
    let commit = |file: &str, message: &str| -> Result<String, Box<dyn Error>> {
        std::fs::write(parent.join(file), format!("{message}\n"))?;
        git(&parent, &["add", file])?;
        git(&parent, &["commit", "-qm", message])?;
        git(&parent, &["rev-parse", "HEAD"])
    };
    let base = commit("README.md", "the parent")?;
    let mut children = HashMap::new();
    for label in labels {
        let branch = format!("yi/child-{label}");
        git(&parent, &["checkout", "-q", "-b", &branch, &base])?;
        let head = commit(&format!("{label}.txt"), label)?;
        git(&parent, &["checkout", "-q", "main"])?;
        let candidate = Candidate {
            branch: BranchName::parse(&branch)?,
            commit: Sha::parse(&head).ok_or("not a sha")?,
            base: Sha::parse(&base),
        };
        children.insert(format!("child-{label}"), candidate);
    }
    if shape == Shape::Moved {
        commit("NOTES.md", "an unrelated commit")?;
    }
    let store = PlanStore::open(root.join("plans"))?;
    let plan = PlanId::slug(GOAL)?;
    let checker = crate::checker::put_checker(
        &store.artifacts(&plan),
        &crate::checker::checker_manifest(check, 60_000, &[], shape == Shape::ReadsOutside),
        &store.nonce(),
    )?;
    let contract: Contract = serde_json::from_value(json!({
        "class": "writer",
        "items": [{"id": "checked", "critical": true, "weight": 100,
                   "decider": {"cmd": {"checker": checker, "timeout_ms": 60_000}}}],
        "threshold": 1000, "min_coverage": 1000
    }))?;
    let pool = Pool::open(&root.join("home"), &parent, slots)?;
    let children = Arc::new(Children(children, parent.clone()));
    let mut engine = PlanEngine::new(store.clone(), children.clone())
        .with_cwd(parent.clone())
        .with_verifier(Verifier::new(60_000))
        .with_lanes(pool);
    if shape != Shape::Unserved {
        engine = engine.with_output_resolve(children);
    }
    let todos = labels
        .iter()
        .map(|label| {
            Ok(TodoSpec {
                label: TodoLabel::new(*label)?,
                after: Vec::new(),
                delegation: Some(serde_json::from_value(json!({
                    "spec": {"isolation": "worktree"}, "accept": {"command": "true"}
                }))?),
                contract: Some(contract.clone()),
                children: Vec::new(),
                cites: Default::default(),
            })
        })
        .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
    let opened = engine.apply(OpRequest {
        plan: None,
        actor: Actor::Owner,
        op: Op::Init {
            goal: GoalText::new(GOAL)?,
            todos,
        },
        request_id: None,
        expected_revision: None,
    })?;
    if opened.spawned.len() != labels.len() {
        return Err(format!("not every todo started: {:?}", opened.notices).into());
    }
    Ok(Bench {
        engine,
        store,
        plan,
        _root: root,
    })
}

impl Bench {
    /// The same bench under a session that ends at `at`.
    pub fn until(self, at: std::time::Instant) -> Self {
        Self {
            engine: self
                .engine
                .with_verifier(Verifier::new(60_000).with_deadline(at)),
            ..self
        }
    }

    fn apply(&self, actor: Actor, op: Op) -> Result<Outcome, PlanOpError> {
        self.engine.apply(OpRequest {
            plan: Some(self.plan.clone()),
            actor,
            op,
            request_id: None,
            expected_revision: None,
        })
    }

    /// The child's own `submit`, naming the parent's README as its output.
    pub fn submit(&self, label: &str) -> Result<Outcome, PlanOpError> {
        let child = AgentId::new(format!("child-{label}")).map_err(PlanOpError::Doc)?;
        let op = Op::Submit {
            label: TodoLabel::new(label).map_err(PlanOpError::Doc)?,
            attempt: AttemptId::FIRST,
            output: "local://README.md"
                .parse()
                .map_err(|_| PlanOpError::NoPlan)?,
        };
        self.apply(Actor::Child(child), op)
    }

    pub fn done(&self, label: &str) -> Result<Outcome, PlanOpError> {
        let op = Op::Done {
            label: TodoLabel::new(label).map_err(PlanOpError::Doc)?,
            output: None,
        };
        self.apply(Actor::Owner, op)
    }

    pub fn state(&self, label: &str) -> Result<TodoState, Box<dyn Error>> {
        Ok(self
            .store
            .read(&self.plan)?
            .todo(&TodoLabel::new(label)?)
            .map(|todo| todo.state.clone())
            .ok_or("todo missing")?)
    }
}
