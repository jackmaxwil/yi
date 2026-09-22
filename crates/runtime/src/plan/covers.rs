//! A covered write runs the `cmd` items on the live tree: a preview that journals nothing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use globset::{GlobBuilder, GlobSetBuilder};
use yi_types::plan::canonical::{ArtifactRef, Digest};
use yi_types::plan::contract::{CheckerManifest, Contract, Decider, ItemVerdict};
use yi_types::plan::doc::{PlanId, PlanState, TodoState};

use super::artifact::Artifacts;
use super::ops::{Actor, OWNER_AGENT, PlanEngine};
use super::state::{reduce, root_of};

pub type WriteCheck = dyn Fn(&[PathBuf], &Path) -> Option<String> + Send + Sync;

#[derive(Default)]
pub(super) struct Previewed(Mutex<HashMap<String, (Digest, String)>>);

pub fn write_check(service: Option<Arc<super::PlanService>>) -> Option<Arc<WriteCheck>> {
    let service: Weak<super::PlanService> = Arc::downgrade(&service?);
    Some(Arc::new(move |paths: &[PathBuf], cwd: &Path| {
        let (engine, actor) = service.upgrade()?.engine()?;
        engine.preview(&actor, paths, cwd)
    }))
}

impl PlanEngine {
    pub fn preview(&self, actor: &Actor, paths: &[PathBuf], cwd: &Path) -> Option<String> {
        let me = match actor {
            Actor::Owner => OWNER_AGENT,
            Actor::Child(me) => me.as_str(),
            _ => return None,
        };
        let roots = match me.split_once('/') {
            Some((plan, _)) => vec![root_of(&PlanId::new(plan).ok()?).ok()?],
            None => self.store.roots().ok()?,
        };
        let mut lines = Vec::new();
        for root in roots {
            let Ok(reading) = self.store.journal(&root).read() else {
                continue;
            };
            let Ok(state) = reduce(&reading.records) else {
                continue;
            };
            for (id, plan) in state
                .plans
                .iter()
                .filter(|(_, plan)| plan.state == PlanState::Active)
            {
                for todo in &plan.todos {
                    let running =
                        matches!(&todo.state, TodoState::Running { by } if by.as_str() == me);
                    let Some(contract) = todo.contract.as_ref().filter(|_| running) else {
                        continue;
                    };
                    let covered = covered(contract, paths, cwd);
                    let key = format!("{id}/{}", todo.label);
                    if let Some(verdict) = self.check_items(&key, id, contract, &covered, cwd) {
                        lines.push(format!(
                            "contract {:?} check: {verdict}",
                            todo.label.as_str()
                        ));
                    }
                }
            }
        }
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    fn check_items(
        &self,
        key: &str,
        plan: &PlanId,
        contract: &Contract,
        covered: &[&PathBuf],
        cwd: &Path,
    ) -> Option<String> {
        if covered.is_empty() {
            return None;
        }
        let digest = covered_digest(covered);
        let artifacts = self.store.artifacts(plan);
        let mut verdicts = Vec::new();
        for item in &contract.items {
            let Decider::Cmd {
                checker,
                timeout_ms,
            } = &item.decider
            else {
                continue;
            };
            let slot = format!("{key}/{}", item.id);
            let seen = self.previewed.0.lock().ok().and_then(|seen| {
                seen.get(&slot)
                    .filter(|(was, _)| *was == digest)
                    .map(|(_, verdict)| verdict.clone())
            });
            let verdict = seen.unwrap_or_else(|| {
                let verdict = self.run_item(&artifacts, checker, *timeout_ms, cwd);
                if let Ok(mut seen) = self.previewed.0.lock() {
                    seen.insert(slot, (digest, verdict.clone()));
                }
                verdict
            });
            verdicts.push(verdict);
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

    fn run_item(
        &self,
        artifacts: &Artifacts,
        checker: &ArtifactRef,
        timeout_ms: u64,
        cwd: &Path,
    ) -> String {
        let manifest = match artifacts
            .get(&checker.digest)
            .map_err(|error| error.to_string())
            .and_then(|bytes| CheckerManifest::parse(&bytes))
        {
            Ok(manifest) => manifest,
            Err(reason) => return ItemVerdict::Abstain { reason }.to_string(),
        };
        let millis = timeout_ms
            .min(manifest.timeout_ms)
            .min(crate::goal::DEFAULT_CHECK_TIMEOUT_MS);
        let now = Instant::now();
        let mut deadline = now
            .checked_add(Duration::from_millis(millis))
            .unwrap_or(now);
        if let Some(session) = self.verifier.deadline() {
            deadline = deadline.min(session);
        }
        super::verify::run_cmd(&manifest, cwd, deadline).to_string()
    }
}

fn covered<'a>(contract: &Contract, paths: &'a [PathBuf], cwd: &Path) -> Vec<&'a PathBuf> {
    let mut set = GlobSetBuilder::new();
    for glob in &contract.covers {
        if let Ok(glob) = GlobBuilder::new(glob).literal_separator(true).build() {
            set.add(glob);
        }
    }
    let Ok(set) = set.build() else {
        return Vec::new();
    };
    let root = yi_permission::lexical_normalize(cwd);
    paths
        .iter()
        .filter(|path| {
            yi_permission::lexical_normalize(path)
                .strip_prefix(&root)
                .is_ok_and(|relative| set.is_match(relative))
        })
        .collect()
}

fn covered_digest(paths: &[&PathBuf]) -> Digest {
    let mut bytes = Vec::new();
    for path in paths {
        bytes.extend_from_slice(path.as_os_str().as_encoded_bytes());
        bytes.push(0);
        bytes.extend(std::fs::read(path).unwrap_or_default());
        bytes.push(0);
    }
    Digest::of(&bytes)
}
