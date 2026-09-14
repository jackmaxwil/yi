//! The plan store (plan section 5.1): `<dir>/<id>/plan.json` checkpoints the root's `ops.jsonl`.

use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use yi_types::plan::PLAN_SCHEMA;
use yi_types::plan::canonical::{Digest, sorted};
use yi_types::plan::doc::{DocError, GoalText, JournalMark, Plan, PlanId};
use yi_types::plan::ledger::{AttemptId, JournalRecord, PlanOpRecord, RequestId, Seq};

use super::artifact::{ArtifactError, Artifacts};
use super::journal::{Clock, Damage, Fs, Journal, JournalError, RealFs, SystemClock, has_record};
use super::state::{KIND_IMPORT, ReduceError, RootState, apply, reduce, root_of};

/// Invariant: the checkpoint byte cap is checked before the atomic replace and never trimmed
/// after: refused or reported, never silently applied, so a file on disk is always whole.
pub const PLAN_CAP_BYTES: usize = 32 * 1024;

pub const CHECKPOINT_NAME: &str = "plan.json";
pub const JOURNAL_NAME: &str = "ops.jsonl";
const GITIGNORE: &str = "*/ops.jsonl\n";
const SCHEMA_DIR: &str = "schemas";
const SCHEMA_NAME: &str = "plan.schema.json";
const LEASE_NAME: &str = ".lease";
const LEASE_PID_NAME: &str = "pid";
const LEASE_HOLD_PREFIX: &str = "hold.";
const LEASE_ATTEMPTS: u8 = 8;
const ALLOCATE_SUFFIX_MAX: u32 = 9_999;
const TEMP_PREFIX: &str = ".plan.";
const TEMP_SUFFIX: &str = ".tmp";

/// Invariant: unique per call within a process, so a temp path and a lease
/// hold each name one write and one holder rather than the whole process.
fn nonce() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}.{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("no plan {id} at {path}")]
    Missing { id: PlanId, path: PathBuf },
    #[error("plan {id} is a format-1 document at {path}; read-only until `yi plan import {id}`")]
    NeedsImport { id: PlanId, path: PathBuf },
    #[error("{path}: {detail}")]
    Schema { path: PathBuf, detail: String },
    #[error("{path}: {detail}")]
    Domain { path: PathBuf, detail: String },
    #[error("plan {id} did not serialize: {source}")]
    Serialize {
        id: PlanId,
        source: serde_json::Error,
    },
    #[error(
        "plan {id} is {bytes} bytes, {over} over the {cap} cap; nothing was written",
        over = bytes.saturating_sub(*cap)
    )]
    PlanOverCap {
        id: PlanId,
        bytes: usize,
        cap: usize,
    },
    #[error("lease {path} is held{}", pid.map(|pid| format!(" by pid {pid}")).unwrap_or_default())]
    LeaseHeld { path: PathBuf, pid: Option<u32> },
    #[error(transparent)]
    Id { source: DocError },
    #[error("every id from {slug} through {slug}-{tried} is taken")]
    AllocateExhausted { slug: PlanId, tried: u32 },
    #[error(transparent)]
    Journal(#[from] JournalError),
    #[error("journal of {root} does not reduce: {source}")]
    Reduce { root: PlanId, source: ReduceError },
    #[error(
        "journal of {root} is damaged at byte {offset}{}: {reason}; run `yi plan repair {root}`",
        seq.map(|seq| format!(" (record {seq})")).unwrap_or_default()
    )]
    RecoveryRequired {
        root: PlanId,
        seq: Option<u64>,
        offset: u64,
        reason: String,
    },
    #[error(
        "{path} was changed behind the engine ({detail}); the journal is authoritative and a view is never folded in"
    )]
    ExternalEdit {
        id: PlanId,
        path: PathBuf,
        detail: String,
    },
    #[error(transparent)]
    Artifact(#[from] ArtifactError),
    #[error(
        "plan {id} has a checkpoint at {path}{} and no journal beside it (a clone carries the view, never the journal); `yi plan import local://{path}` adopts it as a new generation",
        seq.map(|seq| format!(" naming journal record {seq}")).unwrap_or_default()
    )]
    JournalMissing {
        id: PlanId,
        path: PathBuf,
        seq: Option<u64>,
    },
}

/// Invariant: the arbiter is the uniquely named hold file, never the directory, so a holder
/// whose file is gone releases nothing and cannot unlock whoever took the path over.
#[must_use]
#[derive(Debug)]
pub struct Lease {
    dir: PathBuf,
    hold: String,
}

fn lease_hold(dir: &Path) -> Option<String> {
    let entries = std::fs::read_dir(dir).ok()?;
    entries.flatten().find_map(|entry| {
        let name = entry.file_name().into_string().ok()?;
        name.starts_with(LEASE_HOLD_PREFIX).then_some(name)
    })
}

/// Invariant: unlinking one named file is the atomic single-winner step and the name changes
/// every takeover, so a racer can only condemn the generation it judged stale.
fn condemn(dir: &Path, arbiter: &str) -> bool {
    if std::fs::remove_file(dir.join(arbiter)).is_err() {
        return false;
    }
    let _ = std::fs::remove_dir_all(dir);
    true
}

impl Drop for Lease {
    fn drop(&mut self) {
        condemn(&self.dir, &self.hold);
    }
}

#[derive(Clone)]
pub struct PlanStore {
    dir: PathBuf,
    fs: Arc<dyn Fs>,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for PlanStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PlanStore")
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

fn io_at(path: &Path) -> impl FnOnce(std::io::Error) -> StoreError + '_ {
    move |source| StoreError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn domain_at(path: PathBuf) -> impl FnOnce(&dyn std::fmt::Display) -> StoreError {
    move |error| StoreError::Domain {
        path,
        detail: error.to_string(),
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "every field of the envelope is named once"
)]
pub(super) fn draft(
    plan: &PlanId,
    op: &str,
    actor: String,
    at: u64,
    todos: u32,
    args: Value,
    request_id: RequestId,
    expected_revision: u64,
    attempt: Option<AttemptId>,
) -> JournalRecord {
    JournalRecord {
        record: PlanOpRecord {
            plan: plan.clone(),
            op: op.to_owned(),
            actor,
            at,
            todo: None,
            from: None,
            to: None,
            todos,
            extra: serde_json::Map::new(),
        },
        seq: Seq::FIRST,
        request_id,
        expected_revision,
        attempt,
        args,
        args_hash: Digest::of(&[]),
        program_hash: None,
        verdict: None,
        digest: Digest::of(&[]),
    }
}

pub(super) struct Loaded {
    pub state: RootState,
    pub records: Vec<JournalRecord>,
}

impl PlanStore {
    pub fn open(dir: PathBuf) -> Result<Self, StoreError> {
        let store = Self {
            dir,
            fs: Arc::new(RealFs),
            clock: Arc::new(SystemClock),
        };
        if store.dir.is_dir() {
            store.publish()?;
        }
        Ok(store)
    }

    pub fn with_fs(self, fs: Arc<dyn Fs>) -> Self {
        Self { fs, ..self }
    }

    pub fn with_clock(self, clock: Arc<dyn Clock>) -> Self {
        Self { clock, ..self }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn now_ms(&self) -> u64 {
        self.clock.now_ms()
    }

    pub fn nonce(&self) -> String {
        nonce()
    }

    /// A request id minted for a caller that brought none: the clock keeps it unique across
    /// processes, since a journal outlives the pid the counter is scoped to.
    pub fn request_nonce(&self) -> String {
        format!("{}-{}", self.now_ms(), nonce())
    }

    pub fn plan_dir(&self, id: &PlanId) -> PathBuf {
        self.dir.join(id.as_str())
    }

    pub fn path(&self, id: &PlanId) -> PathBuf {
        self.plan_dir(id).join(CHECKPOINT_NAME)
    }

    pub fn journal_path(&self, root: &PlanId) -> PathBuf {
        self.plan_dir(root).join(JOURNAL_NAME)
    }

    pub fn journal(&self, root: &PlanId) -> Journal {
        Journal::open(self.journal_path(root), Arc::clone(&self.fs))
    }

    pub fn artifacts(&self, id: &PlanId) -> Artifacts {
        Artifacts::under(&self.plan_dir(id))
    }

    fn legacy_path(&self, id: &PlanId) -> PathBuf {
        self.dir.join(format!("{id}.md"))
    }

    pub fn exists(&self, id: &PlanId) -> bool {
        self.path(id).is_file() || self.legacy_path(id).is_file()
    }

    fn ensure_dir(&self) -> Result<(), StoreError> {
        std::fs::create_dir_all(&self.dir).map_err(io_at(&self.dir))?;
        self.publish()
    }

    fn publish(&self) -> Result<(), StoreError> {
        let ignore = self.dir.join(".gitignore");
        if !ignore.exists() {
            std::fs::write(&ignore, GITIGNORE).map_err(io_at(&ignore))?;
        }
        let Some(parent) = self.dir.parent() else {
            return Ok(());
        };
        if parent.file_name().and_then(OsStr::to_str) != Some(".yi") {
            return Ok(());
        }
        let schemas = parent.join(SCHEMA_DIR);
        let target = schemas.join(SCHEMA_NAME);
        if std::fs::read(&target).is_ok_and(|bytes| bytes == PLAN_SCHEMA.as_bytes()) {
            return Ok(());
        }
        std::fs::create_dir_all(&schemas).map_err(io_at(&schemas))?;
        let tmp = schemas.join(format!(".{SCHEMA_NAME}.{}", nonce()));
        std::fs::write(&tmp, PLAN_SCHEMA).map_err(io_at(&tmp))?;
        std::fs::rename(&tmp, &target).map_err(io_at(&target))
    }

    fn read_checkpoint(&self, id: &PlanId) -> Result<Plan, StoreError> {
        let path = self.path(id);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                let legacy = self.legacy_path(id);
                if legacy.is_file() {
                    return Err(StoreError::NeedsImport {
                        id: id.clone(),
                        path: legacy,
                    });
                }
                return Err(StoreError::Missing {
                    id: id.clone(),
                    path,
                });
            }
            Err(source) => return Err(StoreError::Io { path, source }),
        };
        Self::decode_checkpoint(path, &bytes, id)
    }

    /// Schema, then domain, on bytes already read: the reader's path and the importer's.
    pub(super) fn decode_checkpoint(
        path: PathBuf,
        bytes: &[u8],
        id: &PlanId,
    ) -> Result<Plan, StoreError> {
        if bytes.len() > PLAN_CAP_BYTES {
            return Err(StoreError::PlanOverCap {
                id: id.clone(),
                bytes: bytes.len(),
                cap: PLAN_CAP_BYTES,
            });
        }
        let value: Value = serde_json::from_slice(bytes).map_err(|error| StoreError::Schema {
            path: path.clone(),
            detail: format!("not JSON: {error}"),
        })?;
        let schema = serde_json::from_str(PLAN_SCHEMA)
            .map_err(|error| error.to_string())
            .and_then(crate::schema::Schema::from_value)
            .map_err(|detail| StoreError::Schema {
                path: path.clone(),
                detail,
            })?;
        schema
            .validate(&value)
            .map_err(|detail| StoreError::Schema {
                path: path.clone(),
                detail,
            })?;
        serde_json::from_value(value).map_err(|error| domain_at(path)(&error))
    }

    pub fn read(&self, id: &PlanId) -> Result<Plan, StoreError> {
        let root = root_of(id).map_err(|_| StoreError::Id {
            source: DocError::PlanIdEmpty,
        })?;
        let checkpoint = match self.read_checkpoint(id) {
            Ok(checkpoint) => Some(checkpoint),
            // Invariant: beside a live journal the checkpoint is derived, so its absence is
            // regenerated, and a legacy file left in place after import names nothing.
            Err(StoreError::Missing { .. } | StoreError::NeedsImport { .. })
                if self.journal_path(&root).is_file() =>
            {
                None
            }
            Err(error) => return Err(error),
        };
        let reading = self.journal(&root).read()?;
        // Invariant: damage is judged before the record count, or a damaged first record
        // would hand back the unverified checkpoint as if the chain had been consulted.
        if let Some(Damage::Corrupt {
            offset,
            seq,
            reason,
            ..
        }) = reading.damage
        {
            return Err(StoreError::RecoveryRequired {
                root,
                seq: seq.map(Seq::get),
                offset,
                reason,
            });
        }
        if reading.records.is_empty() {
            let Some(checkpoint) = checkpoint else {
                return Err(StoreError::Missing {
                    id: id.clone(),
                    path: self.path(id),
                });
            };
            return Err(self.detached(id, &root, &checkpoint));
        }
        let state = reduce(&reading.records).map_err(|source| StoreError::Reduce {
            root: root.clone(),
            source,
        })?;
        let Some(reduced) = state.plans.get(id) else {
            if checkpoint.is_none() {
                return Err(StoreError::Missing {
                    id: id.clone(),
                    path: self.path(id),
                });
            }
            return Err(StoreError::ExternalEdit {
                id: id.clone(),
                path: self.path(id),
                detail: "the journal never made this plan".to_owned(),
            });
        };
        let mark = state.mark.map(|(seq, digest)| JournalMark { seq, digest });
        if let Some(checkpoint) = checkpoint
            && checkpoint.journal == mark
            && &checkpoint.clone().unmarked() == reduced
        {
            return Ok(checkpoint);
        }
        if let Ok(_lease) = self.lease() {
            self.checkpoint_family(&state)?;
        }
        let mut plan = reduced.clone();
        plan.journal = mark;
        Ok(plan)
    }

    /// A checkpoint the journal did not produce: with no journal file it is a clone's view,
    /// named so the remedy is the import; beside an empty journal it is an edit.
    fn detached(&self, id: &PlanId, root: &PlanId, checkpoint: &Plan) -> StoreError {
        let path = self.path(id);
        if has_record(&self.journal_path(root)) {
            return StoreError::ExternalEdit {
                id: id.clone(),
                path,
                detail: "its journal has no record behind it".to_owned(),
            };
        }
        StoreError::JournalMissing {
            id: id.clone(),
            path,
            seq: checkpoint.journal.as_ref().map(|mark| mark.seq.get()),
        }
    }

    /// `adopting` is the importer's load: a checkpoint with no journal is the view it adopts.
    pub(super) fn load(&self, root: &PlanId, adopting: bool) -> Result<Loaded, StoreError> {
        let (state, records, _torn) = super::recovery::reconstruct(self, root)?;
        if records.is_empty() && !adopting && self.path(root).is_file() {
            let checkpoint = self.read_checkpoint(root)?;
            return Err(self.detached(root, root, &checkpoint));
        }
        for (id, plan) in &state.plans {
            self.sweep_temps(id);
            let path = self.path(id);
            if !path.is_file() {
                continue;
            }
            let on_disk = match self.read_checkpoint(id) {
                Ok(on_disk) => on_disk,
                Err(error) => {
                    return Err(StoreError::ExternalEdit {
                        id: id.clone(),
                        path,
                        detail: error.to_string(),
                    });
                }
            };
            let mark = state.mark.map(|(seq, digest)| JournalMark { seq, digest });
            let lagging = match (on_disk.journal, mark) {
                (Some(disk), Some(now)) => disk.seq < now.seq,
                _ => false,
            };
            if lagging {
                continue;
            }
            if on_disk.journal != mark {
                return Err(StoreError::ExternalEdit {
                    id: id.clone(),
                    path,
                    detail: "its journal mark is not the journal's last record".to_owned(),
                });
            }
            if &on_disk.unmarked() != plan {
                return Err(StoreError::ExternalEdit {
                    id: id.clone(),
                    path,
                    detail: "its state is not what the journal reduces to".to_owned(),
                });
            }
        }
        Ok(Loaded { state, records })
    }

    fn sweep_temps(&self, id: &PlanId) {
        let Ok(entries) = std::fs::read_dir(self.plan_dir(id)) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.starts_with(TEMP_PREFIX) && name.ends_with(TEMP_SUFFIX) {
                let _orphan_from_a_crash = std::fs::remove_file(entry.path());
            }
        }
    }

    pub fn render(plan: &Plan) -> Result<String, StoreError> {
        let serialize = |source| StoreError::Serialize {
            id: plan.id.clone(),
            source,
        };
        let value = serde_json::to_value(plan).map_err(serialize)?;
        let mut text = serde_json::to_string_pretty(&sorted(&value)).map_err(serialize)?;
        text.push('\n');
        if text.len() > PLAN_CAP_BYTES {
            return Err(StoreError::PlanOverCap {
                id: plan.id.clone(),
                bytes: text.len(),
                cap: PLAN_CAP_BYTES,
            });
        }
        Ok(text)
    }

    pub fn checkpoint(&self, plan: &Plan) -> Result<(), StoreError> {
        let text = Self::render(plan)?;
        let dir = self.plan_dir(&plan.id);
        std::fs::create_dir_all(&dir).map_err(io_at(&dir))?;
        let tmp = dir.join(format!("{TEMP_PREFIX}{}{TEMP_SUFFIX}", nonce()));
        let target = self.path(&plan.id);
        let written = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .and_then(|mut file| {
                file.write_all(text.as_bytes())?;
                file.sync_all()
            })
            .and_then(|()| std::fs::rename(&tmp, &target))
            .and_then(|()| std::fs::File::open(&dir)?.sync_all());
        if let Err(source) = written {
            let _removed_best_effort = std::fs::remove_file(&tmp);
            return Err(StoreError::Io {
                path: target,
                source,
            });
        }
        Ok(())
    }

    pub fn checkpoint_family(&self, state: &RootState) -> Result<Vec<PlanId>, StoreError> {
        let mark = state.mark.map(|(seq, digest)| JournalMark { seq, digest });
        let mut plans: Vec<Plan> = state
            .plans
            .values()
            .map(|plan| {
                let mut plan = plan.clone();
                plan.journal = mark;
                plan
            })
            .collect();
        plans.sort_by_key(|plan| plan.id.is_root());
        for plan in &plans {
            Self::render(plan)?;
        }
        for plan in &plans {
            self.checkpoint(plan)?;
        }
        Ok(plans.into_iter().map(|plan| plan.id).collect())
    }

    pub fn write(&self, plan: &Plan) -> Result<(), StoreError> {
        self.ensure_dir()?;
        let _lease = self.lease()?;
        let root = root_of(&plan.id).map_err(|_| StoreError::Id {
            source: DocError::PlanIdEmpty,
        })?;
        super::table::validate_plan(plan)
            .map_err(|error| domain_at(self.path(&plan.id))(&error))?;
        let Loaded { mut state, records } = self.load(&root, true)?;
        let args = serde_json::json!({
            "source": null,
            "artifact": null,
            "format": yi_types::plan::doc::PLAN_FORMAT,
            "plan": plan.clone().unmarked(),
        });
        let request = RequestId::new(format!("seed-{}", self.request_nonce()))
            .map_err(|error| domain_at(self.path(&plan.id))(&error))?;
        let record = draft(
            &plan.id,
            KIND_IMPORT,
            super::ops::OWNER_AGENT.to_owned(),
            self.now_ms(),
            u32::try_from(plan.todos.len()).unwrap_or(u32::MAX),
            args,
            request,
            plan.touched.0,
            None,
        );
        let journal = self.journal(&root);
        let sealed = journal.seal(record, records.last())?;
        journal.append(&sealed)?;
        apply(&mut state, &sealed.record).map_err(|source| StoreError::Reduce { root, source })?;
        self.checkpoint_family(&state)?;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<PlanId>, StoreError> {
        let mut ids = Vec::new();
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(ids),
            Err(source) => return Err(io_at(&self.dir)(source)),
        };
        for entry in entries {
            let path = entry.map_err(io_at(&self.dir))?.path();
            if !path.join(CHECKPOINT_NAME).is_file() && !has_record(&path.join(JOURNAL_NAME)) {
                continue;
            }
            let Some(name) = path.file_name().and_then(OsStr::to_str) else {
                continue;
            };
            if let Ok(id) = PlanId::new(name) {
                ids.push(id);
            }
        }
        ids.sort();
        Ok(ids)
    }

    pub fn roots(&self) -> Result<Vec<PlanId>, StoreError> {
        let mut ids = self.list()?;
        ids.retain(PlanId::is_root);
        Ok(ids)
    }

    pub fn allocate(&self, goal: &GoalText) -> Result<PlanId, StoreError> {
        let base = PlanId::slug(goal.as_str()).map_err(|source| StoreError::Id { source })?;
        if !self.exists(&base) {
            return Ok(base);
        }
        for suffix in 2..=ALLOCATE_SUFFIX_MAX {
            let id = PlanId::new(format!("{base}-{suffix}"))
                .map_err(|source| StoreError::Id { source })?;
            if !self.exists(&id) {
                return Ok(id);
            }
        }
        Err(StoreError::AllocateExhausted {
            slug: base,
            tried: ALLOCATE_SUFFIX_MAX,
        })
    }

    /// Invariant: K1's `lock_is_stale` decides: a dead pid, else the 30 s mtime rule, which also
    /// covers an unnamed generation. Taking the lease is what condemns a stale one.
    pub fn lease(&self) -> Result<Lease, StoreError> {
        self.ensure_dir()?;
        let dir = self.dir.join(LEASE_NAME);
        for _ in 0..LEASE_ATTEMPTS {
            match std::fs::create_dir(&dir) {
                Ok(()) => {
                    let hold = format!("{LEASE_HOLD_PREFIX}{}", nonce());
                    std::fs::write(dir.join(&hold), []).map_err(io_at(&dir))?;
                    std::fs::write(
                        dir.join(LEASE_PID_NAME),
                        format!("{}\n", std::process::id()),
                    )
                    .map_err(io_at(&dir))?;
                    if lease_hold(&dir).as_deref() == Some(hold.as_str()) {
                        return Ok(Lease { dir, hold });
                    }
                }
                Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
                    let generation = lease_hold(&dir);
                    let pid = yi_kernel::bootstrap::read_lock_pid(&dir);
                    let probe = pid.map(yi_kernel::bootstrap::process_is_running);
                    if !yi_kernel::bootstrap::lock_is_stale(&dir, probe) {
                        return Err(StoreError::LeaseHeld { path: dir, pid });
                    }
                    match generation {
                        Some(hold) => {
                            condemn(&dir, &hold);
                        }
                        None if yi_kernel::bootstrap::lock_missing_pid_is_stale(&dir) => {
                            condemn(&dir, LEASE_PID_NAME);
                        }
                        None => {
                            let _ = std::fs::remove_dir(&dir);
                        }
                    }
                }
                Err(source) => return Err(StoreError::Io { path: dir, source }),
            }
        }
        Err(StoreError::LeaseHeld {
            path: dir,
            pid: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::ops::{Actor, Delegate, Op, OpRequest, PlanEngine, PlanOpError, TodoSpec};
    use crate::scratch::Scratch;
    use yi_types::plan::PlanVersion;
    use yi_types::plan::doc::{
        AgentId, AttemptId, Delegation, PlanTier, RetryCount, Todo, TodoAddr, TodoLabel, TodoState,
        TouchCount,
    };
    use yi_types::url::Url;

    type Fallible = Result<(), Box<dyn std::error::Error>>;

    struct TempStore {
        store: PlanStore,
        dir: Scratch,
    }

    impl TempStore {
        fn new(name: &str) -> Result<Self, Box<dyn std::error::Error>> {
            let dir = Scratch::new(&format!("yi-plan-store-{name}"))?;
            let store = PlanStore::open(dir.to_path_buf())?;
            Ok(Self { store, dir })
        }
    }

    struct NoChildren;

    impl Delegate for NoChildren {
        fn spawn(&self, _at: &TodoAddr, _delegation: &Delegation) -> Result<AgentId, String> {
            Err("no children in this test".to_owned())
        }

        fn reap(&self, _agent: &AgentId, _supplied: &[Url]) -> Result<Option<Url>, String> {
            Ok(None)
        }

        fn follow_up(&self, _dispatched: &[TodoLabel], _held: usize) {}
    }

    fn owner(op: Op) -> OpRequest {
        OpRequest {
            plan: None,
            actor: Actor::Owner,
            op,
            request_id: None,
            expected_revision: None,
        }
    }

    fn plan(goal: &str) -> Result<Plan, Box<dyn std::error::Error>> {
        let mut plan = Plan::opening(
            PlanId::slug(goal)?,
            GoalText::new(goal)?,
            PlanTier::Root,
            vec![
                Todo {
                    label: TodoLabel::new("Freeze the token API seam")?,
                    after: Vec::new(),
                    state: TodoState::Done {
                        output: Some("kernel://token_api_seam".parse()?),
                    },
                    delegation: None,
                    subplan: None,
                    retries: RetryCount(0),
                    children: Vec::new(),
                    note: None,
                    attempt: AttemptId::FIRST,
                    refusals: 0,
                    extra: serde_json::Map::new(),
                },
                Todo {
                    label: TodoLabel::new("Implement refresh flow")?,
                    after: vec![TodoLabel::new("Freeze the token API seam")?],
                    state: TodoState::Running {
                        by: AgentId::new("child-1")?,
                    },
                    delegation: None,
                    subplan: None,
                    retries: RetryCount(1),
                    children: Vec::new(),
                    note: None,
                    attempt: AttemptId::FIRST,
                    refusals: 0,
                    extra: serde_json::Map::new(),
                },
            ],
        );
        plan.touched = TouchCount(0);
        Ok(plan)
    }

    #[test]
    fn round_trips_through_a_real_directory_with_a_journal_mark() -> Fallible {
        let temp = TempStore::new("round-trip")?;
        let plan = plan("Ship OAuth login end to end")?;
        temp.store.write(&plan)?;
        let back = temp.store.read(&plan.id)?;
        let mark = back.journal.ok_or("no journal mark on the checkpoint")?;
        assert_eq!(mark.seq, Seq::FIRST);
        assert_eq!(back.unmarked(), plan);
        assert_eq!(temp.store.list()?, vec![plan.id.clone()]);
        assert_eq!(temp.store.roots()?, vec![plan.id.clone()]);
        let text = std::fs::read_to_string(temp.store.path(&plan.id))?;
        assert!(
            text.starts_with("{\n  \"constraints\""),
            "sorted keys: {text}"
        );
        assert!(
            std::fs::read_to_string(temp.dir.join(".gitignore"))?.contains("*/ops.jsonl"),
            "the store ignores its journals once"
        );
        Ok(())
    }

    #[test]
    fn the_schema_is_published_beside_a_dot_yi_plans_dir() -> Fallible {
        let scratch = Scratch::new("yi-plan-store-schema")?;
        let plans = scratch.join(".yi/plans");
        std::fs::create_dir_all(&plans)?;
        let target = scratch.join(".yi/schemas/plan.schema.json");
        std::fs::create_dir_all(scratch.join(".yi/schemas"))?;
        std::fs::write(&target, "stale")?;
        PlanStore::open(plans)?;
        assert_eq!(std::fs::read_to_string(&target)?, PLAN_SCHEMA);
        Ok(())
    }

    #[test]
    fn a_corrupted_plan_json_is_refused_with_the_failing_path() -> Fallible {
        let temp = TempStore::new("corrupt")?;
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/plans/format2/corrupt/plan.json");
        let id = PlanId::new("ship-logrotate-lite-with-a-packaged")?;
        std::fs::create_dir_all(temp.store.plan_dir(&id))?;
        std::fs::copy(&fixture, temp.store.path(&id))?;
        let refused = temp.store.read(&id);
        match refused {
            Err(StoreError::Schema { path, detail }) => {
                assert_eq!(path, temp.store.path(&id));
                assert!(
                    detail.starts_with("$.todos[1].state"),
                    "the validator's own path: {detail}"
                );
            }
            other => return Err(format!("expected a schema refusal, got {other:?}").into()),
        }
        let mut domain: Value = serde_json::from_str(&std::fs::read_to_string(&fixture)?)?;
        domain["todos"][1]["state"] = Value::String("running".to_owned());
        std::fs::write(temp.store.path(&id), serde_json::to_string(&domain)?)?;
        assert!(
            matches!(temp.store.read(&id), Err(StoreError::Domain { .. })),
            "running without by is the domain validator's refusal"
        );
        Ok(())
    }

    #[test]
    fn a_format_1_read_is_read_only_until_import() -> Fallible {
        let temp = TempStore::new("legacy")?;
        let id = PlanId::new("7f3a-auth-refactor")?;
        std::fs::create_dir_all(&*temp.dir)?;
        std::fs::write(
            temp.dir.join(format!("{id}.md")),
            "---\n{\"format\": 1, \"plan\": \"7f3a-auth-refactor\", \"goal\": \"g\", \"version\": 1, \"tier\": \"root\", \"state\": \"active\"}\n---\n",
        )?;
        let refused = temp.store.read(&id);
        match refused {
            Err(StoreError::NeedsImport { id: named, .. }) => assert_eq!(named, id),
            other => return Err(format!("expected NeedsImport, got {other:?}").into()),
        }
        assert!(temp.store.exists(&id), "a legacy id is taken");
        assert!(temp.store.list()?.is_empty(), "no checkpoint, no listing");
        let document = crate::plan::import::read_legacy(&temp.store, &id)?;
        assert_eq!(document.plan.id, id);
        assert!(
            !temp.store.journal_path(&id).exists(),
            "a read journals nothing"
        );
        Ok(())
    }

    #[test]
    fn edited_export_cannot_overwrite_authoritative_state() -> Fallible {
        let temp = TempStore::new("edited")?;
        let engine = PlanEngine::new(temp.store.clone(), Arc::new(NoChildren));
        let out = engine.apply(owner(Op::Init {
            goal: GoalText::new("ship the seam")?,
            todos: vec![TodoSpec {
                label: TodoLabel::new("cut")?,
                after: Vec::new(),
                delegation: None,
                children: Vec::new(),
            }],
        }))?;
        let id = out.plan.id.clone();
        let path = temp.store.path(&id);
        let mut forged: Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
        forged["todos"][0]["state"] = Value::String("done".to_owned());
        forged["state"] = Value::String("done".to_owned());
        std::fs::write(&path, serde_json::to_string_pretty(&forged)?)?;
        let refused = engine.apply(owner(Op::Start {
            label: TodoLabel::new("cut")?,
        }));
        assert!(
            matches!(
                refused,
                Err(PlanOpError::Store(StoreError::ExternalEdit { .. }))
            ),
            "a schema-valid, digest-consistent edit is detected on a mutation: {refused:?}"
        );
        let journal = temp.store.journal(&id).read()?;
        assert_eq!(journal.records.len(), 1, "the edited view wrote no record");
        let read = temp.store.read(&id)?;
        assert_eq!(
            read.todos[0].state,
            TodoState::Pending,
            "regenerated on a read"
        );
        let back: Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
        assert_eq!(
            back["todos"][0]["state"],
            Value::String("pending".to_owned())
        );
        Ok(())
    }

    #[test]
    fn a_file_changed_behind_the_engine_is_detected_not_diffed_in() -> Fallible {
        let temp = TempStore::new("behind")?;
        let engine = PlanEngine::new(temp.store.clone(), Arc::new(NoChildren));
        let out = engine.apply(owner(Op::Init {
            goal: GoalText::new("ship the seam")?,
            todos: vec![TodoSpec {
                label: TodoLabel::new("cut")?,
                after: Vec::new(),
                delegation: None,
                children: Vec::new(),
            }],
        }))?;
        let id = out.plan.id.clone();
        let path = temp.store.path(&id);
        let mut edited: Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
        edited["goal"] = Value::String("ship the seam and the docs".to_owned());
        std::fs::write(&path, serde_json::to_string_pretty(&edited)?)?;
        let refused = engine.apply(owner(Op::Append {
            todos: vec![TodoSpec {
                label: TodoLabel::new("polish")?,
                after: Vec::new(),
                delegation: None,
                children: Vec::new(),
            }],
        }));
        assert!(
            matches!(
                refused,
                Err(PlanOpError::Store(StoreError::ExternalEdit { .. }))
            ),
            "{refused:?}"
        );
        let viewed = engine.apply(owner(Op::View { full: true }))?;
        assert_eq!(viewed.plan.goal.as_str(), "ship the seam", "not diffed in");
        assert_eq!(
            viewed.plan.touched,
            TouchCount(1),
            "no user-attributed bump"
        );
        let landed = engine.apply(owner(Op::Append {
            todos: vec![TodoSpec {
                label: TodoLabel::new("polish")?,
                after: Vec::new(),
                delegation: None,
                children: Vec::new(),
            }],
        }))?;
        assert_eq!(landed.plan.todos.len(), 2, "once regenerated, work resumes");
        Ok(())
    }

    #[test]
    fn over_cap_plan_is_refused_and_the_file_is_untouched() -> Fallible {
        let temp = TempStore::new("cap")?;
        let mut plan = plan("Ship OAuth login end to end")?;
        temp.store.write(&plan)?;
        let before = std::fs::read_to_string(temp.store.path(&plan.id))?;
        plan.extra
            .insert("notes".to_owned(), "x".repeat(PLAN_CAP_BYTES).into());
        let refused = temp.store.write(&plan);
        assert!(matches!(
            refused,
            Err(StoreError::PlanOverCap { bytes, cap, .. })
                if bytes > cap && cap == PLAN_CAP_BYTES
        ));
        let after = std::fs::read_to_string(temp.store.path(&plan.id))?;
        assert_eq!(before, after);
        Ok(())
    }

    #[test]
    fn allocate_suffixes_on_collision() -> Fallible {
        let temp = TempStore::new("collide")?;
        let goal = GoalText::new("Ship OAuth login end to end")?;
        let first = temp.store.allocate(&goal)?;
        assert_eq!(first.as_str(), "ship-oauth-login-end-to-end");
        let mut plan = plan(goal.as_str())?;
        temp.store.write(&plan)?;
        let second = temp.store.allocate(&goal)?;
        assert_eq!(second.as_str(), "ship-oauth-login-end-to-end-2");
        plan.id = second;
        temp.store.write(&plan)?;
        assert_eq!(
            temp.store.allocate(&goal)?.as_str(),
            "ship-oauth-login-end-to-end-3"
        );
        Ok(())
    }

    #[test]
    fn a_second_lease_from_one_process_is_refused_until_drop() -> Fallible {
        let temp = TempStore::new("lease")?;
        let held = temp.store.lease()?;
        let own = std::process::id();
        assert!(matches!(
            temp.store.lease(),
            Err(StoreError::LeaseHeld { pid: Some(pid), .. }) if pid == own
        ));
        drop(held);
        let retaken = temp.store.lease()?;
        drop(retaken);
        Ok(())
    }

    #[test]
    fn a_stale_lease_with_a_dead_pid_is_taken_over() -> Fallible {
        let temp = TempStore::new("stale-lease")?;
        let dir = temp.dir.join(LEASE_NAME);
        std::fs::create_dir(&dir)?;
        std::fs::write(dir.join(format!("{LEASE_HOLD_PREFIX}4000000000.0")), [])?;
        std::fs::write(dir.join(LEASE_PID_NAME), "4000000000\n")?;
        let taken = temp.store.lease()?;
        drop(taken);
        assert!(!dir.exists());
        std::fs::create_dir(&dir)?;
        std::fs::write(dir.join(LEASE_PID_NAME), "4000000000\n")?;
        assert!(
            matches!(temp.store.lease(), Err(StoreError::LeaseHeld { .. })),
            "a dead pid with no generation marker was raced instead of waited out"
        );
        Ok(())
    }

    #[test]
    fn a_hold_whose_arbiter_was_taken_over_releases_nothing() -> Fallible {
        let temp = TempStore::new("lease-drop")?;
        let dir = temp.dir.join(LEASE_NAME);
        let held = temp.store.lease()?;
        std::fs::remove_dir_all(&dir)?;
        std::fs::create_dir(&dir)?;
        let successor = format!("{LEASE_HOLD_PREFIX}4000000000.0");
        std::fs::write(dir.join(&successor), [])?;
        drop(held);
        assert!(
            dir.join(&successor).is_file(),
            "the previous holder released a lease another racer had taken over"
        );
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }

    #[test]
    fn a_stale_lease_admits_one_taker_under_contention() -> Fallible {
        for trial in 0..50u32 {
            let temp = TempStore::new(&format!("lease-race-{trial}"))?;
            let dir = temp.dir.join(LEASE_NAME);
            std::fs::create_dir(&dir)?;
            std::fs::write(dir.join(LEASE_PID_NAME), "4000000000\n")?;
            let ready = std::sync::Barrier::new(6);
            let taken: std::sync::Mutex<Vec<Lease>> = std::sync::Mutex::new(Vec::new());
            std::thread::scope(|scope| {
                for _ in 0..6 {
                    scope.spawn(|| {
                        ready.wait();
                        if let Ok(lease) = temp.store.lease()
                            && let Ok(mut held) = taken.lock()
                        {
                            held.push(lease);
                        }
                    });
                }
            });
            let held = taken.into_inner().map_err(|_| "the holder list poisoned")?;
            assert!(
                held.len() <= 1,
                "trial {trial}: {} concurrent holders of a mutual-exclusion lease",
                held.len()
            );
        }
        Ok(())
    }

    #[test]
    fn a_reader_never_sees_a_torn_checkpoint() -> Fallible {
        let temp = TempStore::new("torn-write")?;
        let mut plan = plan("Ship OAuth login end to end")?;
        plan.extra
            .insert("pad".to_owned(), "x".repeat(20_000).into());
        temp.store.write(&plan)?;
        let plan = temp.store.read(&plan.id)?;
        let path = temp.store.path(&plan.id);
        let whole = std::fs::read_to_string(&path)?.len();
        let done = std::sync::atomic::AtomicBool::new(false);
        let torn: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            scope.spawn(|| {
                while !done.load(Ordering::Relaxed) {
                    match std::fs::read_to_string(&path) {
                        Ok(text) if text.len() == whole => {}
                        Ok(text) => {
                            if let Ok(mut log) = torn.lock() {
                                log.push(format!("published {} of {whole} bytes", text.len()));
                            }
                        }
                        Err(error) => {
                            if let Ok(mut log) = torn.lock() {
                                log.push(format!("read: {error}"));
                            }
                        }
                    }
                }
            });
            for _ in 0..60 {
                if let Err(error) = temp.store.checkpoint(&plan)
                    && let Ok(mut log) = torn.lock()
                {
                    log.push(format!("write: {error}"));
                }
            }
            done.store(true, Ordering::Relaxed);
        });
        let log = torn.into_inner().map_err(|_| "the tear log poisoned")?;
        assert!(
            log.is_empty(),
            "{} tears, first: {:?}",
            log.len(),
            log.first()
        );
        Ok(())
    }

    #[test]
    fn metacharacters_in_labels_and_keys_survive_write_then_read() -> Fallible {
        let temp = TempStore::new("metacharacters")?;
        let mut plan = plan("Ship OAuth login end to end")?;
        plan.todos.clear();
        for label in [
            "hint[see docs",
            "brace{open",
            "close]bracket",
            "close}brace",
            "comma,separated",
            "colon:tight",
            "hash#tag",
            "-leading dash",
            "trailing space ",
            "quote\"and\\slash",
            "--- looks like a fence",
        ] {
            let mut todo = Todo::pending(TodoLabel::new(label)?);
            todo.extra
                .insert(format!("note{label}"), "read the seam notes".into());
            plan.todos.push(todo);
        }
        temp.store.write(&plan)?;
        assert_eq!(temp.store.read(&plan.id)?.unmarked(), plan);
        assert_eq!(plan.version, PlanVersion(1));
        Ok(())
    }
}
