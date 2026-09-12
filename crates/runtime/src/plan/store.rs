use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use yi_types::plan::doc::{AgentId, DocError, GoalText, Plan, PlanId, TodoLabel, TodoState};

/// Invariant: the frontmatter byte cap is checked before the atomic replace and never trimmed
/// after — refused or reported, never silently applied — so a file on disk is always whole.
pub const FRONTMATTER_CAP_BYTES: usize = 32 * 1024;

const LEASE_NAME: &str = ".lease";
const LEASE_PID_NAME: &str = "pid";
const LEASE_HOLD_PREFIX: &str = "hold.";
const LEASE_ATTEMPTS: u8 = 8;
const ALLOCATE_SUFFIX_MAX: u32 = 9_999;

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
    #[error("{path}: {source}")]
    Document {
        path: PathBuf,
        source: DocumentError,
    },
    #[error("plan {id} did not serialize: {source}")]
    Serialize {
        id: PlanId,
        source: serde_json::Error,
    },
    #[error(
        "plan {id} frontmatter is {bytes} bytes, {over} over the {cap} cap; nothing was written",
        over = bytes.saturating_sub(*cap)
    )]
    FrontmatterOverCap {
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
}

/// Invariant: the frontmatter is JSON between two `---` lines, so the one
/// parser behind a user-editable file is serde_json and nothing hand-rolled.
#[derive(Debug, thiserror::Error)]
pub enum DocumentError {
    #[error("no opening --- delimiter: {head}")]
    NoFrontmatter { head: String },
    #[error("frontmatter opened at line 1 is unclosed after {lines} lines")]
    OpenFrontmatter { lines: usize },
    #[error("frontmatter: {0}")]
    Json(#[from] serde_json::Error),
}

pub fn split_frontmatter(document: &str) -> Result<(&str, &str), DocumentError> {
    let mut offset = 0usize;
    let mut opened = false;
    let mut start = 0usize;
    let mut count = 0usize;
    for raw in document.split_inclusive('\n') {
        count = count.saturating_add(1);
        let closing = raw.trim_end() == "---";
        if !opened {
            if !closing {
                return Err(DocumentError::NoFrontmatter {
                    head: raw.trim_end().to_string(),
                });
            }
            opened = true;
            start = offset.saturating_add(raw.len());
        } else if closing {
            let body = offset.saturating_add(raw.len());
            return Ok((
                document.get(start..offset).unwrap_or(""),
                document.get(body..).unwrap_or(""),
            ));
        }
        offset = offset.saturating_add(raw.len());
    }
    if opened {
        Err(DocumentError::OpenFrontmatter { lines: count })
    } else {
        Err(DocumentError::NoFrontmatter {
            head: String::new(),
        })
    }
}

pub fn parse_document(text: &str) -> Result<PlanFile, DocumentError> {
    let (front, body) = split_frontmatter(text)?;
    Ok(PlanFile {
        plan: serde_json::from_str(front)?,
        body: body.to_owned(),
    })
}

/// The `## <heading>` section of a body with its heading line, up to the next
/// section, trailing blank lines dropped.
pub fn section_of(body: &str, heading: &str) -> Option<String> {
    let header = format!("## {heading}");
    let mut collected: Vec<&str> = Vec::new();
    let mut inside = false;
    for line in body.lines() {
        if line.trim_end() == header {
            inside = true;
            collected.push(line);
        } else if inside && line.starts_with("## ") {
            break;
        } else if inside {
            collected.push(line);
        }
    }
    inside.then(|| collected.join("\n").trim_end().to_owned())
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanFile {
    pub plan: Plan,
    pub body: String,
}

/// What a hand edit moved that the engine must answer for: todos whose recorded Running child
/// is not the one on disk. Every other divergence costs one [`Plan::touched`] bump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandEdit {
    pub left_running: Vec<TodoLabel>,
}

pub(super) fn running_by(state: &TodoState) -> Option<&AgentId> {
    match state {
        TodoState::Running { by } => Some(by),
        TodoState::Pending
        | TodoState::Blocked { .. }
        | TodoState::Done { .. }
        | TodoState::Failed { .. }
        | TodoState::Abandoned
        | TodoState::Other(_) => None,
    }
}

/// Invariant: the arbiter is the uniquely named hold file, never the directory, so a holder
/// whose file is gone releases nothing and cannot unlock whoever took the path over.
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

#[derive(Debug, Clone)]
pub struct PlanStore {
    dir: PathBuf,
}

fn io_at(path: &Path) -> impl FnOnce(std::io::Error) -> StoreError + '_ {
    move |source| StoreError::Io {
        path: path.to_path_buf(),
        source,
    }
}

impl PlanStore {
    pub fn open(dir: PathBuf) -> Result<Self, StoreError> {
        Ok(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path(&self, id: &PlanId) -> PathBuf {
        self.dir.join(format!("{id}.md"))
    }

    pub fn exists(&self, id: &PlanId) -> bool {
        self.path(id).is_file()
    }

    pub fn read(&self, id: &PlanId) -> Result<PlanFile, StoreError> {
        let path = self.path(id);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Err(StoreError::Missing {
                    id: id.clone(),
                    path,
                });
            }
            Err(source) => return Err(StoreError::Io { path, source }),
        };
        parse_document(&text).map_err(|source| StoreError::Document { path, source })
    }

    /// The frontmatter as [`PlanStore::write`] puts it on disk; the cap is the only refusal.
    pub fn render(plan: &Plan) -> Result<String, StoreError> {
        let front = serde_json::to_string_pretty(plan).map_err(|source| StoreError::Serialize {
            id: plan.id.clone(),
            source,
        })?;
        if front.len() > FRONTMATTER_CAP_BYTES {
            return Err(StoreError::FrontmatterOverCap {
                id: plan.id.clone(),
                bytes: front.len(),
                cap: FRONTMATTER_CAP_BYTES,
            });
        }
        Ok(front)
    }

    pub fn write(&self, file: &PlanFile) -> Result<(), StoreError> {
        std::fs::create_dir_all(&self.dir).map_err(io_at(&self.dir))?;
        let id = &file.plan.id;
        let front = Self::render(&file.plan)?;
        let document = format!("---\n{front}\n---\n{}", file.body);
        let tmp = self.dir.join(format!(".{id}.{}.tmp", nonce()));
        std::fs::write(&tmp, &document).map_err(io_at(&tmp))?;
        let target = self.path(id);
        if let Err(source) = std::fs::rename(&tmp, &target) {
            let _ = std::fs::remove_file(&tmp);
            return Err(StoreError::Io {
                path: target,
                source,
            });
        }
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
            if path.extension().and_then(OsStr::to_str) != Some("md") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(OsStr::to_str) else {
                continue;
            };
            if let Ok(id) = PlanId::new(stem) {
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

    /// Invariant: K1's lock rule branches — a parseable pid file makes staleness dead-pid
    /// only, and the 30 s mtime rule covers the arms K1 leaves plus an unnamed generation.
    pub fn lease(&self) -> Result<Lease, StoreError> {
        std::fs::create_dir_all(&self.dir).map_err(io_at(&self.dir))?;
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
                    match yi_kernel::bootstrap::read_lock_pid(&dir) {
                        Some(pid) if yi_kernel::bootstrap::process_is_running(pid) => {
                            return Err(StoreError::LeaseHeld {
                                path: dir,
                                pid: Some(pid),
                            });
                        }
                        None if !yi_kernel::bootstrap::lock_missing_pid_is_stale(&dir) => {
                            return Err(StoreError::LeaseHeld {
                                path: dir,
                                pid: None,
                            });
                        }
                        _ => {}
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

    pub fn user_edits(&self, known: &PlanFile) -> Result<Option<HandEdit>, StoreError> {
        let disk = self.read(&known.plan.id)?;
        if disk == *known {
            return Ok(None);
        }
        let left_running = known
            .plan
            .todos
            .iter()
            .filter(|todo| {
                running_by(&todo.state).is_some_and(|was| {
                    disk.plan
                        .todo(&todo.label)
                        .and_then(|now| running_by(&now.state))
                        != Some(was)
                })
            })
            .map(|todo| todo.label.clone())
            .collect();
        Ok(Some(HandEdit { left_running }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;
    use yi_types::plan::PlanVersion;
    use yi_types::plan::doc::{PlanTier, RetryCount, Todo, TouchCount};

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
                    extra: serde_json::Map::new(),
                },
            ],
        );
        plan.touched = TouchCount(0);
        Ok(plan)
    }

    #[test]
    fn round_trips_through_a_real_directory() -> Fallible {
        let temp = TempStore::new("round-trip")?;
        let file = PlanFile {
            plan: plan("Ship OAuth login end to end")?,
            body: "\n## Implement refresh flow\n\nSeam notes.\n".to_owned(),
        };
        temp.store.write(&file)?;
        let back = temp.store.read(&file.plan.id)?;
        assert_eq!(back, file);
        assert_eq!(
            section_of(&back.body, "Implement refresh flow").as_deref(),
            Some("## Implement refresh flow\n\nSeam notes.")
        );
        assert_eq!(temp.store.list()?, vec![file.plan.id.clone()]);
        assert_eq!(temp.store.roots()?, vec![file.plan.id.clone()]);
        Ok(())
    }

    #[test]
    fn reads_a_hand_written_document() -> Fallible {
        let temp = TempStore::new("example")?;
        let document = concat!(
            "---\n",
            "{\n",
            "  \"format\": 1,\n",
            "  \"plan\": \"7f3a-auth-refactor\",\n",
            "  \"goal\": \"Ship OAuth login end to end\",\n",
            "  \"version\": 3,\n",
            "  \"tier\": \"root\",\n",
            "  \"state\": \"active\",\n",
            "  \"todos\": [\n",
            "    {\"label\": \"Freeze the token API seam\", \"state\": \"done\",\n",
            "     \"output\": \"kernel://token_api_seam\"},\n",
            "    {\"label\": \"Implement refresh flow\", \"state\": \"running\", \"by\": \"child-1\",\n",
            "     \"after\": [\"Freeze the token API seam\"],\n",
            "     \"delegation\": {\n",
            "       \"spec\": {\"role\": \"coder\", \"effort\": \"med\", \"isolation\": \"worktree\"},\n",
            "       \"accept\": {\"command\": \"cargo test -p yi-ai refresh\"},\n",
            "       \"output\": {\"schema\": \"local://.yi/schemas/refresh_result.json\"},\n",
            "       \"context\": [\"plan://7f3a-auth-refactor/seam-notes\", \"local://docs/auth.md\"]\n",
            "     },\n",
            "     \"retries\": 1}\n",
            "  ]\n",
            "}\n",
            "---\n",
            "\n",
            "## seam-notes\n",
            "\n",
            "The refresh endpoint owns rotation.\n",
        );
        let id = PlanId::new("7f3a-auth-refactor")?;
        std::fs::write(temp.store.path(&id), document)?;
        let file = temp.store.read(&id)?;
        assert_eq!(file.plan.goal.as_str(), "Ship OAuth login end to end");
        assert_eq!(file.plan.version, PlanVersion(3));
        assert_eq!(file.plan.tier, PlanTier::Root);
        assert_eq!(file.plan.todos.len(), 2);
        assert!(matches!(
            file.plan.todos.first().map(|todo| &todo.state),
            Some(TodoState::Done { output: Some(_) })
        ));
        let running = file.plan.todos.get(1).ok_or("no second todo")?;
        assert!(matches!(&running.state, TodoState::Running { by } if by.as_str() == "child-1"));
        let delegation = running.delegation.as_ref().ok_or("no delegation")?;
        assert_eq!(delegation.context.len(), 2);
        assert_eq!(
            section_of(&file.body, "seam-notes").as_deref(),
            Some("## seam-notes\n\nThe refresh endpoint owns rotation.")
        );
        temp.store.write(&file)?;
        assert_eq!(temp.store.read(&id)?, file);
        assert!(matches!(
            temp.store.read(&PlanId::new("nowhere")?),
            Err(StoreError::Missing { .. })
        ));
        std::fs::write(temp.store.path(&id), "# not a plan\n")?;
        assert!(matches!(
            temp.store.read(&id),
            Err(StoreError::Document {
                source: DocumentError::NoFrontmatter { .. },
                ..
            })
        ));
        std::fs::write(temp.store.path(&id), "---\n{\"format\": 1\n")?;
        assert!(matches!(
            temp.store.read(&id),
            Err(StoreError::Document {
                source: DocumentError::OpenFrontmatter { lines: 2 },
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn over_cap_frontmatter_is_refused_and_the_file_is_untouched() -> Fallible {
        let temp = TempStore::new("cap")?;
        let mut file = PlanFile {
            plan: plan("Ship OAuth login end to end")?,
            body: String::new(),
        };
        temp.store.write(&file)?;
        let before = std::fs::read_to_string(temp.store.path(&file.plan.id))?;
        file.plan
            .extra
            .insert("notes".to_owned(), "x".repeat(FRONTMATTER_CAP_BYTES).into());
        let refused = temp.store.write(&file);
        assert!(matches!(
            refused,
            Err(StoreError::FrontmatterOverCap { bytes, cap, .. })
                if bytes > cap && cap == FRONTMATTER_CAP_BYTES
        ));
        let after = std::fs::read_to_string(temp.store.path(&file.plan.id))?;
        assert_eq!(before, after);
        Ok(())
    }

    #[test]
    fn allocate_suffixes_on_collision() -> Fallible {
        let temp = TempStore::new("collide")?;
        let goal = GoalText::new("Ship OAuth login end to end")?;
        let first = temp.store.allocate(&goal)?;
        assert_eq!(first.as_str(), "ship-oauth-login-end-to-end");
        let mut file = PlanFile {
            plan: plan(goal.as_str())?,
            body: String::new(),
        };
        temp.store.write(&file)?;
        let second = temp.store.allocate(&goal)?;
        assert_eq!(second.as_str(), "ship-oauth-login-end-to-end-2");
        file.plan.id = second;
        temp.store.write(&file)?;
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
    fn two_writers_in_one_process_never_publish_a_torn_file() -> Fallible {
        let temp = TempStore::new("torn-write")?;
        let file = PlanFile {
            plan: plan("Ship OAuth login end to end")?,
            body: format!("\n## seam-notes\n\n{}\n", "x".repeat(200_000)),
        };
        temp.store.write(&file)?;
        let path = temp.store.path(&file.plan.id);
        let whole = std::fs::read_to_string(&path)?.len();
        let done = std::sync::atomic::AtomicBool::new(false);
        let torn: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            scope.spawn(|| {
                while !done.load(Ordering::Relaxed) {
                    let seen = match std::fs::read_to_string(&path) {
                        Ok(text) => text.len(),
                        Err(error) => {
                            if let Ok(mut log) = torn.lock() {
                                log.push(format!("read: {error}"));
                            }
                            continue;
                        }
                    };
                    if seen != whole
                        && let Ok(mut log) = torn.lock()
                    {
                        log.push(format!("published {seen} of {whole} bytes"));
                    }
                }
            });
            let writers: Vec<_> = (0..2)
                .map(|_| {
                    scope.spawn(|| {
                        for _ in 0..150 {
                            if let Err(error) = temp.store.write(&file)
                                && let Ok(mut log) = torn.lock()
                            {
                                log.push(format!("write: {error}"));
                            }
                        }
                    })
                })
                .collect();
            for writer in writers {
                let _ = writer.join();
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
        let mut file = PlanFile {
            plan: plan("Ship OAuth login end to end")?,
            body: String::new(),
        };
        file.plan.todos.clear();
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
            let mut extra = serde_json::Map::new();
            extra.insert(format!("note{label}"), "read the seam notes".into());
            file.plan.todos.push(Todo {
                label: TodoLabel::new(label)?,
                after: Vec::new(),
                state: TodoState::Pending,
                delegation: None,
                subplan: None,
                retries: RetryCount(0),
                children: Vec::new(),
                extra,
            });
        }
        temp.store.write(&file)?;
        assert_eq!(temp.store.read(&file.plan.id)?, file);
        Ok(())
    }

    #[test]
    fn a_hand_edit_names_the_todos_that_left_their_running_child() -> Fallible {
        let temp = TempStore::new("edits")?;
        let known = PlanFile {
            plan: plan("Ship OAuth login end to end")?,
            body: "\n## seam-notes\n\nold\n".to_owned(),
        };
        temp.store.write(&known)?;
        assert_eq!(temp.store.user_edits(&known)?, None);
        let mut edited = known.clone();
        edited.body = "\n## seam-notes\n\nnew\n".to_owned();
        assert_eq!(
            temp.store
                .write(&edited)
                .and_then(|()| temp.store.user_edits(&known))?,
            Some(HandEdit {
                left_running: Vec::new()
            }),
            "prose moved, no child did"
        );
        if let Some(todo) = edited.plan.todos.get_mut(1) {
            todo.state = TodoState::Running {
                by: AgentId::new("child-2")?,
            };
        }
        temp.store.write(&edited)?;
        let swapped = temp.store.user_edits(&known)?.ok_or("no divergence")?;
        assert_eq!(
            swapped.left_running,
            vec![TodoLabel::new("Implement refresh flow")?],
            "a swapped agent leaves the first child behind"
        );
        edited.plan.todos.remove(1);
        temp.store.write(&edited)?;
        let dropped = temp.store.user_edits(&known)?.ok_or("no divergence")?;
        assert_eq!(
            dropped.left_running,
            vec![TodoLabel::new("Implement refresh flow")?],
            "a todo deleted by hand leaves its child behind too"
        );
        Ok(())
    }
}
