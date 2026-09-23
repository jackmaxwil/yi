//! A covered write runs the `cmd` items on a copy of the writer's tree, and journals nothing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use globset::{GlobBuilder, GlobSetBuilder};
use yi_tools::CancelFlag;
use yi_types::plan::canonical::ArtifactRef;
use yi_types::plan::contract::{CheckerManifest, Contract, Decider, ItemVerdict};
use yi_types::plan::doc::{PlanId, PlanState, Todo, TodoState};

use super::artifact::Artifacts;
use super::done::Workspace;
use super::ops::{Actor, OWNER_AGENT, PlanEngine};
use super::snapshot::{Snapshotter, TreeHash};
use super::state::{reduce, root_of};

pub const PREVIEW_BUDGET_MS: u64 = 60_000;

pub type WriteCheck = dyn Fn(&[PathBuf], &Path, &CancelFlag) -> Option<String> + Send + Sync;

#[derive(Default)]
pub(super) struct Previewed(Mutex<HashMap<String, (String, String)>>);

pub fn write_check(service: Option<Arc<super::PlanService>>) -> Option<Arc<WriteCheck>> {
    let service: Weak<super::PlanService> = Arc::downgrade(&service?);
    Some(Arc::new(
        move |paths: &[PathBuf], cwd: &Path, cancel: &CancelFlag| {
            let (engine, actor) = service.upgrade()?.engine()?;
            engine.preview(&actor, paths, cwd, cancel)
        },
    ))
}

struct Tree {
    snapshotter: Arc<dyn Snapshotter>,
    id: Result<String, String>,
}

struct Run<'a> {
    cwd: &'a Path,
    tree: Tree,
    deadline: Instant,
    cancel: &'a CancelFlag,
}

impl PlanEngine {
    pub fn preview(
        &self,
        actor: &Actor,
        paths: &[PathBuf],
        cwd: &Path,
        cancel: &CancelFlag,
    ) -> Option<String> {
        let me = match actor {
            Actor::Owner => OWNER_AGENT,
            Actor::Child(me) => me.as_str(),
            _ => return None,
        };
        let root = match me.split_once('/') {
            Some((plan, _)) => root_of(&PlanId::new(plan).ok()?).ok()?,
            None => self.resolve(None).ok()?,
        };
        let reading = self.store.journal(&root).read().ok()?;
        let state = reduce(&reading.records).ok()?;
        let mut run: Option<Run<'_>> = None;
        let mut lines = Vec::new();
        for (id, plan) in state
            .plans
            .iter()
            .filter(|(_, plan)| plan.state == PlanState::Active)
        {
            for todo in &plan.todos {
                let running = matches!(&todo.state, TodoState::Running { by } if by.as_str() == me);
                let Some(contract) = todo.contract.as_ref().filter(|_| running) else {
                    continue;
                };
                if !covers(contract, paths, cwd) {
                    continue;
                }
                let run = run.get_or_insert_with(|| self.start_run(cwd, cancel));
                if let Some(verdict) = self.check_items(id, todo, contract, run) {
                    lines.push(format!(
                        "contract {:?} check: {verdict}",
                        todo.label.as_str()
                    ));
                }
            }
        }
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    fn start_run<'a>(&self, cwd: &'a Path, cancel: &'a CancelFlag) -> Run<'a> {
        let now = Instant::now();
        let mut deadline = now
            .checked_add(Duration::from_millis(PREVIEW_BUDGET_MS))
            .unwrap_or(now);
        if let Some(session) = self.verifier.deadline() {
            deadline = deadline.min(session);
        }
        let own =
            yi_permission::lexical_normalize(cwd) == yi_permission::lexical_normalize(&self.cwd);
        let snapshotter: Arc<dyn Snapshotter> = match &self.lane_home {
            _ if own => Arc::clone(&self.snapshotter),
            Some((home, _)) => super::snapshot::shadow_tree(home, cwd, self.store.dir())
                .unwrap_or_else(|| Arc::new(TreeHash::excluding(self.store.dir()))),
            None => Arc::new(TreeHash::excluding(self.store.dir())),
        };
        let id = snapshotter.capture(cwd);
        Run {
            cwd,
            tree: Tree { snapshotter, id },
            deadline,
            cancel,
        }
    }

    fn check_items(
        &self,
        plan: &PlanId,
        todo: &Todo,
        contract: &Contract,
        run: &Run<'_>,
    ) -> Option<String> {
        let artifacts = self.store.artifacts(plan);
        let mut workspace: Option<Result<Workspace, String>> = None;
        let mut verdicts = Vec::new();
        for item in &contract.items {
            let Decider::Cmd {
                checker,
                timeout_ms,
            } = &item.decider
            else {
                continue;
            };
            let slot = format!(
                "{plan}/{}/{}/{}/{}",
                todo.label,
                todo.attempt.get(),
                item.id,
                checker.digest.hex()
            );
            let tree = match &run.tree.id {
                Ok(tree) => tree,
                Err(reason) => {
                    verdicts.push(format!("abstain: the tree could not be read: {reason}"));
                    break;
                }
            };
            let seen = self.previewed.0.lock().ok().and_then(|seen| {
                seen.get(&slot)
                    .filter(|(was, _)| was == tree)
                    .map(|(_, verdict)| verdict.clone())
            });
            if let Some(verdict) = seen {
                verdicts.push(verdict);
                continue;
            }
            if (run.cancel)() || Instant::now() >= run.deadline {
                verdicts.push(format!(
                    "abstain: the preview stopped (cancelled, or past its {} s)",
                    PREVIEW_BUDGET_MS / 1000
                ));
                break;
            }
            let root = workspace.get_or_insert_with(|| self.materialize(run, tree));
            let verdict = match root {
                Ok(root) => self.run_item(&artifacts, checker, *timeout_ms, &root.0, run),
                Err(reason) => ItemVerdict::Abstain {
                    reason: reason.clone(),
                },
            };
            let text = verdict.to_string();
            if !matches!(verdict, ItemVerdict::Abstain { .. })
                && let Ok(mut seen) = self.previewed.0.lock()
            {
                seen.insert(slot, (tree.clone(), text.clone()));
            }
            verdicts.push(text);
        }
        if verdicts.is_empty() {
            return None;
        }
        let failing: Vec<String> = verdicts.into_iter().filter(|v| v != "pass").collect();
        Some(if failing.is_empty() {
            "pass".to_owned()
        } else {
            failing.join("; ")
        })
    }

    fn materialize(&self, run: &Run<'_>, tree: &str) -> Result<Workspace, String> {
        let workspace = Workspace(
            std::env::temp_dir().join(format!("yi-preview-{}", self.store.request_nonce())),
        );
        run.tree
            .snapshotter
            .materialize(run.cwd, tree, &workspace.0)
            .map_err(|reason| format!("snapshot could not be materialized: {reason}"))?;
        Ok(workspace)
    }

    fn run_item(
        &self,
        artifacts: &Artifacts,
        checker: &ArtifactRef,
        timeout_ms: u64,
        root: &Path,
        run: &Run<'_>,
    ) -> ItemVerdict {
        let manifest = match artifacts
            .get(&checker.digest)
            .map_err(|error| error.to_string())
            .and_then(|bytes| CheckerManifest::parse(&bytes))
        {
            Ok(manifest) => manifest,
            Err(reason) => return ItemVerdict::Abstain { reason },
        };
        let millis = timeout_ms
            .min(manifest.timeout_ms)
            .min(crate::goal::DEFAULT_CHECK_TIMEOUT_MS);
        let now = Instant::now();
        let deadline = now
            .checked_add(Duration::from_millis(millis))
            .unwrap_or(now)
            .min(run.deadline);
        super::verify::protecting(&manifest, root, || {
            super::verify::run_cmd(&manifest, root, deadline, Some(run.cancel))
        })
    }
}

fn covers(contract: &Contract, paths: &[PathBuf], cwd: &Path) -> bool {
    let mut set = GlobSetBuilder::new();
    for glob in &contract.covers {
        if let Ok(glob) = GlobBuilder::new(glob).literal_separator(true).build() {
            set.add(glob);
        }
    }
    let Ok(set) = set.build() else {
        return false;
    };
    let root = yi_permission::lexical_normalize(cwd);
    paths.iter().any(|path| {
        yi_permission::lexical_normalize(path)
            .strip_prefix(&root)
            .is_ok_and(|relative| set.is_match(relative))
    })
}
