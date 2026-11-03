//! The spec store: one harness-owned object per session holding the requirements, the
//! declared artifacts and the check, every change and every run a session entry.

pub mod gate;

/// The spec's baseline snapshots and run scratch live outside the workspace, so a refute can
/// restore a tree the model never sees.
const DIR: &str = "spec";

/// Build the session's spec store and its tool before the plan engine is wired, so a request
/// that never opens a plan still gets the stop rule.
pub fn wire(
    session: &crate::session::AgentSession,
    wiring: &crate::wiring::RuntimeWiring,
    tools: &mut Vec<Arc<dyn yi_tools::Tool>>,
) {
    let store = SpecStore::new(
        session.store_handle(),
        wiring.cwd.clone(),
        wiring.home.join(DIR),
        wiring.depth == 0,
        wiring.family_dir.clone(),
        wiring.session_sandbox(),
    );
    session.set_spec(Arc::clone(&store));
    tools.push(Arc::new(tool::SpecTool::new(store)));
}
pub mod refute;
pub mod run;
pub mod tap;
pub mod tool;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::Map;
use yi_types::spec::{
    ARTIFACT_MAX_BYTES, Artifact, ArtifactRefused, CHECK_TIMEOUT_MAX, CheckCommand, Draft,
    FreezeFact, FreezeRefusal, Frozen, Kind, RefuteRecord, ReqId, Requirement, Run, RunKind,
    SPEC_ENTRY_TYPE, SPEC_REFUTE_ENTRY_TYPE, SPEC_RUN_ENTRY_TYPE, Spec, SpecError, SpecRecord,
    Status,
};

use crate::goal::StoreHandle;

pub const SPEC_PRELUDE_TYPE: &str = "spec_prelude";
pub const OP_FREEZE: &str = "freeze";
pub const OP_FREEZE_REFUSED: &str = "freeze_refused";
pub const OP_ADD: &str = "add";
pub const FAMILY_SPEC_FILE: &str = "spec.json";

/// The store as a late-bound handle: `spec_handle` fills the slot once the runtime is wired.
pub type SpecHandle = Arc<dyn Fn() -> Option<Arc<SpecStore>> + Send + Sync>;

/// What the prompt named before any spec existed: the enumerated count the freeze checks
/// the requirements against.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Expectation {
    pub enumerated: usize,
    pub expected: bool,
}

#[derive(Debug)]
pub enum FreezeError {
    Refused(FreezeRefusal),
    Failed(SpecError),
}

impl From<SpecError> for FreezeError {
    fn from(error: SpecError) -> Self {
        Self::Failed(error)
    }
}

/// The per-requirement view of the last run, counted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub green: usize,
    pub total: usize,
    pub red: Vec<ReqId>,
    pub missing: Vec<ReqId>,
}

#[derive(Default)]
struct State {
    spec: Option<Spec>,
    last: Option<Run>,
    refute: Option<RefuteRecord>,
    runs: u32,
    writes_since_run: u32,
    expectation: Expectation,
}

pub struct SpecStore {
    state: Mutex<State>,
    store: StoreHandle,
    cwd: PathBuf,
    dir: PathBuf,
    root: bool,
    /// The family dir (D164): the root writes `spec.json` there, a child reads it.
    family: Option<PathBuf>,
    family_cache: crate::file_cache::Slot<Spec>,
    /// What a model-written check runs under: the same profile `bash` gets.
    sandbox: Option<yi_tools::Sandbox>,
}

impl SpecStore {
    /// `dir` is where baseline snapshots live, outside the workspace; `root` is false in a
    /// child, which may run but never freeze.
    pub fn new(
        store: StoreHandle,
        cwd: PathBuf,
        dir: PathBuf,
        root: bool,
        family: Option<PathBuf>,
        sandbox: Option<yi_tools::Sandbox>,
    ) -> Arc<Self> {
        let this = Arc::new(Self {
            state: Mutex::new(State::default()),
            store,
            cwd,
            dir,
            root,
            family,
            family_cache: crate::file_cache::Slot::default(),
            sandbox,
        });
        this.rehydrate();
        this
    }

    fn rehydrate(&self) {
        let Some(session) = (self.store)() else {
            return;
        };
        let record = latest_record(&session);
        let last = latest_run(&session);
        let refute = latest_refute(&session);
        if let Some(frozen) = record.as_ref().and_then(|record| record.spec.frozen()) {
            let _a_failed_recover_is_retried_by_the_next_take =
                refute::recover(&frozen.draft.artifacts, &self.dir().join("aside"));
        }
        if let Ok(mut state) = self.state.lock() {
            state.spec = record.map(|record| record.spec);
            state.last = last;
            state.refute = refute;
        }
    }

    /// The session's own document, or in a child the family's (read-only), re-read only when
    /// the root's file changed.
    pub fn spec(&self) -> Option<Spec> {
        let own = self.state.lock().ok().and_then(|state| state.spec.clone());
        if own.is_some() || self.root {
            return own;
        }
        let family = self.family.as_deref()?;
        let path = family.join(FAMILY_SPEC_FILE);
        let stamp = crate::file_cache::Stamp::of(&path, None)?;
        self.family_cache
            .load(stamp, || {
                read_family_spec(family).ok_or(SpecError::NotFrozen)
            })
            .ok()
            .map(|spec| spec.as_ref().clone())
    }

    pub fn is_root(&self) -> bool {
        self.root
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub(crate) fn sandbox(&self) -> Option<&yi_tools::Sandbox> {
        self.sandbox.as_ref()
    }

    pub fn last_run(&self) -> Option<Run> {
        self.state.lock().ok().and_then(|state| state.last.clone())
    }

    pub fn expectation(&self) -> Expectation {
        self.state
            .lock()
            .map(|state| state.expectation.clone())
            .unwrap_or_default()
    }

    /// The prompt hook's fact: how many items the request enumerates.
    pub fn note_request(&self, enumerated: usize) {
        if let Ok(mut state) = self.state.lock() {
            state.expectation = Expectation {
                enumerated,
                expected: true,
            };
        }
    }

    pub fn note_write(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.writes_since_run = state.writes_since_run.saturating_add(1);
        }
    }

    pub fn writes_since_run(&self) -> u32 {
        self.state
            .lock()
            .map(|state| state.writes_since_run)
            .unwrap_or(0)
    }

    /// The baseline snapshot and the refute's aside copies, per session, outside the workspace.
    pub fn dir(&self) -> PathBuf {
        match (self.store)() {
            Some(session) => self
                .dir
                .join(&yi_session::lock_session(&session).metadata().id),
            None => self.dir.clone(),
        }
    }

    /// Validate, snapshot, run the baseline, then type every requirement by it. Every
    /// failing fact is collected and the refusal carries them all.
    pub fn freeze(
        &self,
        requirements: Vec<(String, Kind)>,
        artifacts: &[String],
        check: &str,
    ) -> Result<Frozen, FreezeError> {
        if !self.root {
            return Err(SpecError::ChildCannotFreeze.into());
        }
        if self.spec().is_some_and(|spec| spec.frozen().is_some()) {
            return Err(SpecError::AlreadyFrozen.into());
        }
        let check = CheckCommand::try_from(check)?;
        let mut facts = Vec::new();
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut declared = Vec::new();
        for raw in artifacts {
            match Artifact::declare(raw, &self.cwd, home.as_deref()) {
                Ok(artifact) => {
                    let bytes = tree_size(artifact.path());
                    if bytes > ARTIFACT_MAX_BYTES {
                        facts.push(FreezeFact::Artifact {
                            raw: raw.clone(),
                            why: ArtifactRefused::TooLarge {
                                bytes,
                                max: ARTIFACT_MAX_BYTES,
                            },
                        });
                    } else {
                        declared.push(artifact);
                    }
                }
                Err(why) => facts.push(FreezeFact::Artifact {
                    raw: raw.clone(),
                    why,
                }),
            }
        }
        let requirements: Vec<Requirement> = requirements
            .into_iter()
            .enumerate()
            .map(|(index, (text, kind))| Requirement {
                id: ReqId::numbered(index.saturating_add(1)),
                text: text.trim().to_owned(),
                kind,
            })
            .filter(|requirement| !requirement.text.is_empty())
            .collect();
        let enumerated = self.expectation().enumerated;
        if requirements.len() < enumerated {
            facts.push(FreezeFact::Fewer {
                held: requirements.len(),
                enumerated,
            });
        }
        if !requirements.iter().any(|r| r.kind == Kind::F2p) {
            facts.push(FreezeFact::NoF2p);
        }
        let draft = Draft {
            requirements,
            artifacts: declared,
            check,
            enumerated,
        };
        if !facts.is_empty() {
            let refusal = FreezeRefusal { facts };
            self.record_spec(OP_FREEZE_REFUSED, &Spec::Draft(draft), Some(&refusal));
            return Err(FreezeError::Refused(refusal));
        }
        let snapshot = self.snapshot(&draft.artifacts)?;
        let baseline = run::run_check(
            &draft.check,
            &self.cwd,
            CHECK_TIMEOUT_MAX,
            RunKind::Baseline,
            self.sandbox.as_ref(),
        );
        self.record_run(&baseline);
        let mut facts = Vec::new();
        match &baseline.outcome {
            yi_types::spec::Outcome::NotRun(cause) => {
                facts.push(FreezeFact::DidNotRun {
                    cause: cause.clone(),
                });
            }
            yi_types::spec::Outcome::Ran { metrics, .. } => {
                let mut green = Vec::new();
                let mut red = Vec::new();
                let mut uncovered = Vec::new();
                let mut no_metric = Vec::new();
                for requirement in &draft.requirements {
                    let status = baseline.status(&requirement.id);
                    match (requirement.kind, status) {
                        (Kind::F2p, Status::Green) => green.push(requirement.id.clone()),
                        (Kind::P2p, Status::Red) => red.push(requirement.id.clone()),
                        (Kind::F2p | Kind::P2p, Status::Missing) => {
                            uncovered.push(requirement.id.clone());
                        }
                        (Kind::Metric, _) if metrics.is_empty() => {
                            no_metric.push(requirement.id.clone());
                        }
                        (Kind::F2p, Status::Red)
                        | (Kind::P2p, Status::Green)
                        | (Kind::Metric, _) => {}
                    }
                }
                if !green.is_empty() {
                    facts.push(FreezeFact::GreenAtBaseline { ids: green });
                }
                if !red.is_empty() {
                    facts.push(FreezeFact::RedAtBaseline { ids: red });
                }
                if !uncovered.is_empty() {
                    facts.push(FreezeFact::Uncovered { ids: uncovered });
                }
                if !no_metric.is_empty() {
                    facts.push(FreezeFact::MetricWithoutValue { ids: no_metric });
                }
            }
        }
        if !facts.is_empty() {
            let refusal = FreezeRefusal { facts };
            self.record_spec(OP_FREEZE_REFUSED, &Spec::Draft(draft), Some(&refusal));
            return Err(FreezeError::Refused(refusal));
        }
        let frozen = Frozen {
            timeout: run::timeout_for(baseline.duration),
            frozen_at: yi_session::now_ms(),
            snapshot,
            baseline: baseline.clone(),
            draft,
        };
        let spec = Spec::Frozen(Box::new(frozen.clone()));
        self.record_spec(OP_FREEZE, &spec, None);
        {
            let mut state = self.state.lock().map_err(|_| SpecError::Poisoned)?;
            state.spec = Some(spec.clone());
            state.last = Some(baseline.clone());
            state.runs = state.runs.saturating_add(1);
            state.writes_since_run = 0;
        }
        Ok(frozen)
    }

    /// The harness runs the frozen check now and records the result.
    pub fn run(&self, kind: RunKind) -> Result<Run, SpecError> {
        let frozen = self
            .spec()
            .and_then(|spec| spec.frozen().cloned())
            .ok_or(SpecError::NotFrozen)?;
        let run = run::run_check(
            &frozen.draft.check,
            &self.cwd,
            frozen.timeout,
            kind,
            self.sandbox.as_ref(),
        );
        self.record_run(&run);
        {
            let mut state = self.state.lock().map_err(|_| SpecError::Poisoned)?;
            state.last = Some(run.clone());
            state.runs = state.runs.saturating_add(1);
            state.writes_since_run = 0;
        }
        Ok(run)
    }

    /// The refute for this check and these artifacts: cached by its key, run otherwise, and
    /// recorded as a `spec_refute` entry.
    pub fn ensure_refute(&self, frozen: &Frozen) -> Result<RefuteRecord, SpecError> {
        let key = refute::key_of(&self.cwd, frozen);
        let cached = self
            .state
            .lock()
            .map_err(|_| SpecError::Poisoned)?
            .refute
            .clone()
            .filter(|record| record.key == key && key.check_hash.is_some());
        if let Some(record) = cached {
            return Ok(record);
        }
        let record = refute::refute(self, frozen)?;
        self.record_refute(&record);
        let mut state = self.state.lock().map_err(|_| SpecError::Poisoned)?;
        state.refute = Some(record.clone());
        Ok(record)
    }

    /// A requirement found after the freeze; covered once a TAP line names it.
    pub fn add(&self, text: &str, kind: Kind) -> Result<ReqId, SpecError> {
        let mut state = self.state.lock().map_err(|_| SpecError::Poisoned)?;
        let Some(Spec::Frozen(frozen)) = state.spec.as_mut() else {
            return Err(SpecError::NotFrozen);
        };
        let next = frozen
            .draft
            .requirements
            .iter()
            .map(|r| r.id.number())
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let id = ReqId::numbered(next);
        frozen.draft.requirements.push(Requirement {
            id: id.clone(),
            text: text.trim().to_owned(),
            kind,
        });
        let spec = Spec::Frozen(frozen.clone());
        drop(state);
        self.record_spec(OP_ADD, &spec, None);
        Ok(id)
    }

    /// Green over total for the non-metric requirements, and the ids that are not green.
    pub fn summary(&self) -> Option<Summary> {
        let spec = self.spec()?;
        self.summary_of(&spec)
    }

    fn summary_of(&self, spec: &Spec) -> Option<Summary> {
        let frozen = spec.frozen()?;
        let last = self.last_run()?;
        let mut summary = Summary::default();
        for requirement in &frozen.draft.requirements {
            if requirement.kind == Kind::Metric {
                continue;
            }
            summary.total = summary.total.saturating_add(1);
            match last.status(&requirement.id) {
                Status::Green => summary.green = summary.green.saturating_add(1),
                Status::Red => summary.red.push(requirement.id.clone()),
                Status::Missing => summary.missing.push(requirement.id.clone()),
            }
        }
        Some(summary)
    }

    /// The environment line: the state as a fact, refreshed every turn.
    pub fn line(&self) -> Option<String> {
        let Some(spec) = self.spec() else {
            return self
                .expectation()
                .expected
                .then(|| "spec: none (expected: write check.py, then `spec freeze`)".to_owned());
        };
        let Some(frozen) = spec.frozen() else {
            return Some("spec: unfrozen".to_owned());
        };
        let summary = self.summary_of(&spec)?;
        let mut line = format!("spec: {}/{} green", summary.green, summary.total);
        if !summary.red.is_empty() {
            line.push_str(&format!(" · red {}", ids_text(&summary.red)));
        }
        if !summary.missing.is_empty() {
            line.push_str(&format!(" · missing {}", ids_text(&summary.missing)));
        }
        let writes = self.writes_since_run();
        if writes > 0 {
            line.push_str(&format!(" · stale ({writes} writes since run)"));
        } else {
            line.push_str(" · check after last write");
        }
        if let Some(yi_types::spec::Outcome::Ran { metrics, .. }) =
            self.last_run().map(|run| run.outcome)
            && frozen
                .draft
                .requirements
                .iter()
                .any(|r| r.kind == Kind::Metric)
        {
            // ponytail: the last value only; a best-so-far needs the metric's direction.
            for (name, value) in metrics {
                line.push_str(&format!(" · {name} {}", value.get()));
            }
        }
        Some(line)
    }

    fn snapshot(&self, artifacts: &[Artifact]) -> Result<PathBuf, SpecError> {
        let dir = self.dir().join("baseline");
        let io = |source| SpecError::Io {
            path: dir.clone(),
            source,
        };
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(io)?;
        }
        std::fs::create_dir_all(&dir).map_err(io)?;
        for (index, artifact) in artifacts.iter().enumerate() {
            let target = dir.join(index.to_string());
            if artifact.path().exists() {
                copy_tree(artifact.path(), &target)?;
            }
        }
        Ok(dir)
    }

    fn record_spec(&self, op: &str, spec: &Spec, refusal: Option<&FreezeRefusal>) {
        if self.root
            && refusal.is_none()
            && let Some(family) = &self.family
            && let Ok(text) = serde_json::to_string(spec)
        {
            let _a_family_write_never_fails_an_op = std::fs::create_dir_all(family)
                .and_then(|()| std::fs::write(family.join(FAMILY_SPEC_FILE), text));
        }
        let Some(session) = (self.store)() else {
            return;
        };
        let mut extra = Map::new();
        if let Some(refusal) = refusal
            && let Ok(facts) = serde_json::to_value(&refusal.facts)
        {
            extra.insert("facts".to_owned(), facts);
        }
        let record = SpecRecord {
            at: yi_session::now_ms(),
            op: op.to_owned(),
            spec: spec.clone(),
            extra,
        };
        let Ok(payload) = serde_json::to_value(&record) else {
            return;
        };
        let _a_ledger_write_never_fails_an_op = yi_session::lock_session(&session).append_custom(
            "main",
            SPEC_ENTRY_TYPE,
            Some(payload),
        );
    }

    fn record_refute(&self, record: &RefuteRecord) {
        let Some(session) = (self.store)() else {
            return;
        };
        let Ok(payload) = serde_json::to_value(record) else {
            return;
        };
        let _a_ledger_write_never_fails_a_refute = yi_session::lock_session(&session)
            .append_custom("main", SPEC_REFUTE_ENTRY_TYPE, Some(payload));
    }

    fn record_run(&self, run: &Run) {
        let Some(session) = (self.store)() else {
            return;
        };
        let Ok(payload) = serde_json::to_value(run) else {
            return;
        };
        let _a_ledger_write_never_fails_a_run = yi_session::lock_session(&session).append_custom(
            "main",
            SPEC_RUN_ENTRY_TYPE,
            Some(payload),
        );
    }
}

/// A session event that wrote the workspace: the last run no longer covers the tree.
pub fn note_event(store: &SpecStore, event: &yi_types::event::AgentEvent) {
    if crate::environment::GitSummary::wrote(event) {
        store.note_write();
    }
}
pub fn ids_text(ids: &[ReqId]) -> String {
    ids.iter().map(ReqId::as_str).collect::<Vec<_>>().join(" ")
}

/// The newest `spec` entry that holds a document (a refused freeze records its draft for
/// the extractor, and is not the document).
pub fn latest_record(session: &yi_session::SharedSession) -> Option<SpecRecord> {
    let entries = yi_session::lock_session(session)
        .find_entries(&yi_session::EntryQuery {
            custom_type: Some(SPEC_ENTRY_TYPE.to_owned()),
            order: yi_session::EntryOrder::NewestFirst,
            limit: None,
            ..yi_session::EntryQuery::default()
        })
        .unwrap_or_default();
    entries.into_iter().find_map(|entry| match entry {
        yi_types::entry::Entry::Custom {
            data: Some(data), ..
        } => serde_json::from_value::<SpecRecord>(data)
            .ok()
            .filter(|record| record.op != OP_FREEZE_REFUSED),
        _ => None,
    })
}

pub fn latest_run(session: &yi_session::SharedSession) -> Option<Run> {
    let entries = yi_session::lock_session(session)
        .find_entries(&yi_session::EntryQuery {
            custom_type: Some(SPEC_RUN_ENTRY_TYPE.to_owned()),
            order: yi_session::EntryOrder::NewestFirst,
            limit: Some(1),
            ..yi_session::EntryQuery::default()
        })
        .unwrap_or_default();
    entries.into_iter().find_map(|entry| match entry {
        yi_types::entry::Entry::Custom {
            data: Some(data), ..
        } => serde_json::from_value::<Run>(data).ok(),
        _ => None,
    })
}

/// The root's document as the family sees it.
pub fn read_family_spec(family: &Path) -> Option<Spec> {
    let text = std::fs::read_to_string(family.join(FAMILY_SPEC_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn latest_refute(session: &yi_session::SharedSession) -> Option<RefuteRecord> {
    let entries = yi_session::lock_session(session)
        .find_entries(&yi_session::EntryQuery {
            custom_type: Some(SPEC_REFUTE_ENTRY_TYPE.to_owned()),
            order: yi_session::EntryOrder::NewestFirst,
            limit: Some(1),
            ..yi_session::EntryQuery::default()
        })
        .unwrap_or_default();
    entries.into_iter().find_map(|entry| match entry {
        yi_types::entry::Entry::Custom {
            data: Some(data), ..
        } => serde_json::from_value::<RefuteRecord>(data).ok(),
        _ => None,
    })
}

/// Bytes under a path, files summed, saturating; a missing path is zero.
pub fn tree_size(path: &Path) -> u64 {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if !metadata.is_dir() {
        return metadata.len();
    }
    let mut total = 0_u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                stack.push(entry.path());
            } else if let Ok(meta) = entry.metadata() {
                total = total.saturating_add(meta.len());
            }
        }
    }
    total
}

/// Copy a file or a tree; a symlink is copied as a link, so the copy is the tree
/// [`tree_size`] measured and a link loop or a link to `$HOME` is not followed.
pub fn copy_tree(from: &Path, to: &Path) -> Result<(), SpecError> {
    let io = |path: &Path, source: std::io::Error| SpecError::Io {
        path: path.to_path_buf(),
        source,
    };
    let kind = std::fs::symlink_metadata(from)
        .map_err(|e| io(from, e))?
        .file_type();
    if kind.is_symlink() {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
        }
        let target = std::fs::read_link(from).map_err(|e| io(from, e))?;
        return std::os::unix::fs::symlink(target, to).map_err(|e| io(to, e));
    }
    if kind.is_dir() {
        std::fs::create_dir_all(to).map_err(|e| io(to, e))?;
        let entries = std::fs::read_dir(from).map_err(|e| io(from, e))?;
        for entry in entries {
            let entry = entry.map_err(|e| io(from, e))?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        return Ok(());
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
    }
    std::fs::copy(from, to).map(|_| ()).map_err(|e| io(from, e))
}

/// How many items the request enumerates: its numbered or bulleted lines, or the largest
/// `N constraints|requirements|rules|checks|tests|cases|items` phrase, whichever is larger.
pub fn enumerated_count(text: &str) -> usize {
    let lines = text
        .lines()
        .filter(|line| crate::plan::loop_coupling::gate::enumerated(line))
        .count();
    let mut phrase = 0_usize;
    let words: Vec<&str> = text.split_whitespace().collect();
    for pair in words.windows(2) {
        let [number, noun] = pair else {
            continue;
        };
        let noun = noun
            .trim_matches(|c: char| !c.is_ascii_alphabetic())
            .to_ascii_lowercase();
        if !matches!(
            noun.as_str(),
            "constraints" | "requirements" | "rules" | "checks" | "tests" | "cases" | "items"
        ) {
            continue;
        }
        if let Ok(n) = number
            .trim_matches(|c: char| !c.is_ascii_digit())
            .parse::<usize>()
            && number.bytes().any(|b| b.is_ascii_digit())
        {
            phrase = phrase.max(n);
        }
    }
    lines.max(phrase)
}

/// The prompt-time prelude when a spec is expected and none exists.
pub fn prelude_text(text: &str, checker: Option<&str>) -> String {
    let lines: Vec<String> = text
        .lines()
        .filter(|line| crate::plan::loop_coupling::gate::enumerated(line))
        .enumerate()
        .map(|(index, line)| {
            let cut: String = line.trim().chars().take(72).collect();
            format!("R{} {cut}", index.saturating_add(1))
        })
        .collect();
    let mut out = String::from(
        "A spec is expected before the first write: write `check.py` printing one TAP line per requirement (`ok N - R7 …` / `not ok N - R7 …`, one `R` id per requirement, reading the files the task names), then call `spec freeze` with the requirements, the artifact paths and the command. The freeze runs it on the untouched tree: every f2p requirement must be red there.",
    );
    if !lines.is_empty() {
        out.push_str(&format!(
            "\n{} enumerated lines found:\n{}",
            lines.len(),
            lines.join("\n")
        ));
    }
    if let Some(checker) = checker {
        out.push_str(&format!(
            "\n`{checker}` found in the workspace; the check may wrap it."
        ));
    }
    out
}
