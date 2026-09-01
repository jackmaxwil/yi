use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use yi_types::plan::doc::{
    DocError, GoalText, Plan, PlanId, PlanState, Spawns, Todo, TodoLabel, TodoState,
};

use super::yaml::{self, YamlError};

/// Invariant: the frontmatter byte cap is checked before the atomic replace
/// and never trimmed after — a cap is refused or reported, never silently
/// applied — so a file on disk is always whole.
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
    Yaml { path: PathBuf, source: YamlError },
    #[error("{path}: {source}")]
    Doc {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("plan {id} did not serialize: {source}")]
    Serialize {
        id: PlanId,
        source: serde_json::Error,
    },
    #[error("plan {id} frontmatter left the yaml subset: {source}")]
    Emit { id: PlanId, source: YamlError },
    #[error("plan {id} frontmatter did not read back: {source}; nothing was written")]
    Unreadable { id: PlanId, source: YamlError },
    #[error("plan {id} frontmatter read back as a different document; nothing was written")]
    Drifted { id: PlanId },
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

/// One `## <heading>` section of the Markdown body; `text` is the verbatim
/// prose after the heading line, up to the next section.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BodySection {
    pub heading: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlanBody {
    pub preamble: String,
    pub sections: Vec<BodySection>,
}

impl PlanBody {
    pub fn parse(text: &str) -> Self {
        let mut body = Self::default();
        for line in text.split_inclusive('\n') {
            if let Some(rest) = line.strip_prefix("## ") {
                body.sections.push(BodySection {
                    heading: rest.trim_end().to_owned(),
                    text: String::new(),
                });
            } else {
                match body.sections.last_mut() {
                    Some(section) => section.text.push_str(line),
                    None => body.preamble.push_str(line),
                }
            }
        }
        body
    }

    pub fn render(&self) -> String {
        let mut out = self.preamble.clone();
        for section in &self.sections {
            out.push_str("## ");
            out.push_str(&section.heading);
            out.push('\n');
            out.push_str(&section.text);
        }
        out
    }

    pub fn section(&self, heading: &str) -> Option<&str> {
        self.sections
            .iter()
            .find(|section| section.heading == heading)
            .map(|section| section.text.as_str())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanFile {
    pub plan: Plan,
    pub body: PlanBody,
}

/// What one hand edit of the plan file changed, attributable to the user;
/// [`std::fmt::Display`] renders the human-readable diff line.
#[derive(Debug, Clone, PartialEq)]
pub enum UserEdit {
    GoalReworded {
        from: GoalText,
        to: GoalText,
    },
    PlanStateChanged {
        from: PlanState,
        to: PlanState,
    },
    SpawnsChanged {
        from: Spawns,
        to: Spawns,
    },
    TodoAdded {
        todo: Box<Todo>,
    },
    TodoDropped {
        label: TodoLabel,
    },
    TodoStateChanged {
        label: TodoLabel,
        from: TodoState,
        to: TodoState,
    },
    TodoEdgesChanged {
        label: TodoLabel,
        from: Vec<TodoLabel>,
        to: Vec<TodoLabel>,
    },
    TodoEdited {
        label: TodoLabel,
    },
    TodoReordered {
        order: Vec<TodoLabel>,
    },
    PreambleEdited,
    SectionEdited {
        heading: String,
    },
}

fn state_name(state: &TodoState) -> &str {
    match state {
        TodoState::Pending => "pending",
        TodoState::Running { .. } => "running",
        TodoState::Blocked { .. } => "blocked",
        TodoState::Done { .. } => "done",
        TodoState::Failed { .. } => "failed",
        TodoState::Abandoned => "abandoned",
        TodoState::Other(tag) => tag,
    }
}

fn plan_state_name(state: &PlanState) -> &str {
    match state {
        PlanState::Active => "active",
        PlanState::Done => "done",
        PlanState::Superseded { .. } => "superseded",
        PlanState::Abandoned => "abandoned",
        PlanState::Other(tag) => tag,
    }
}

impl std::fmt::Display for UserEdit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GoalReworded { from, to } => {
                write!(
                    formatter,
                    "goal reworded from {:?} to {:?}",
                    from.as_str(),
                    to.as_str()
                )
            }
            Self::PlanStateChanged { from, to } => write!(
                formatter,
                "plan state changed from {} to {}",
                plan_state_name(from),
                plan_state_name(to)
            ),
            Self::SpawnsChanged { from, to } => {
                write!(
                    formatter,
                    "spawns changed from {} to {}",
                    from.get(),
                    to.get()
                )
            }
            Self::TodoAdded { todo } => write!(formatter, "todo {:?} added", todo.label.as_str()),
            Self::TodoDropped { label } => write!(formatter, "todo {:?} dropped", label.as_str()),
            Self::TodoStateChanged { label, from, to } => write!(
                formatter,
                "todo {:?} moved from {} to {}",
                label.as_str(),
                state_name(from),
                state_name(to)
            ),
            Self::TodoEdgesChanged { label, from, to } => {
                let from: Vec<&str> = from.iter().map(TodoLabel::as_str).collect();
                let to: Vec<&str> = to.iter().map(TodoLabel::as_str).collect();
                write!(
                    formatter,
                    "todo {:?} after edges changed from {from:?} to {to:?}",
                    label.as_str()
                )
            }
            Self::TodoEdited { label } => write!(formatter, "todo {:?} edited", label.as_str()),
            Self::TodoReordered { order } => {
                let order: Vec<&str> = order.iter().map(TodoLabel::as_str).collect();
                write!(formatter, "todos reordered to {order:?}")
            }
            Self::PreambleEdited => write!(formatter, "body preamble edited"),
            Self::SectionEdited { heading } => {
                write!(formatter, "body section {heading:?} edited")
            }
        }
    }
}

/// Invariant: the arbiter is the uniquely named hold file, never the
/// directory, so a holder whose file is gone releases nothing and cannot
/// unlock whoever took the path over.
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

/// Invariant: unlinking one named file is the atomic single-winner step, and
/// the name changes every takeover, so a racer can only ever condemn the
/// generation it judged stale.
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
        std::fs::create_dir_all(&dir).map_err(io_at(&dir))?;
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
        let (front, body) = yaml::split_frontmatter(&text).map_err(|source| StoreError::Yaml {
            path: path.clone(),
            source,
        })?;
        let value = yaml::from_yaml(front).map_err(|source| StoreError::Yaml {
            path: path.clone(),
            source,
        })?;
        let plan: Plan =
            serde_json::from_value(value).map_err(|source| StoreError::Doc { path, source })?;
        Ok(PlanFile {
            plan,
            body: PlanBody::parse(body),
        })
    }

    pub fn write(&self, file: &PlanFile) -> Result<(), StoreError> {
        let id = &file.plan.id;
        let value = serde_json::to_value(&file.plan).map_err(|source| StoreError::Serialize {
            id: id.clone(),
            source,
        })?;
        let front = yaml::to_yaml(&value).map_err(|source| StoreError::Emit {
            id: id.clone(),
            source,
        })?;
        if front.len() > FRONTMATTER_CAP_BYTES {
            return Err(StoreError::FrontmatterOverCap {
                id: id.clone(),
                bytes: front.len(),
                cap: FRONTMATTER_CAP_BYTES,
            });
        }
        match yaml::from_yaml(&front) {
            Ok(back) if back == value => {}
            Ok(_) => return Err(StoreError::Drifted { id: id.clone() }),
            Err(source) => {
                return Err(StoreError::Unreadable {
                    id: id.clone(),
                    source,
                });
            }
        }
        let document = format!("---\n{front}---\n{}", file.body.render());
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
        for entry in std::fs::read_dir(&self.dir).map_err(io_at(&self.dir))? {
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

    /// Invariant: K1's lock rule branches — a parseable pid file makes
    /// staleness dead-pid only, and the 30 s mtime rule covers the arms K1
    /// leaves it, plus a lease whose generation cannot be named.
    pub fn lease(&self) -> Result<Lease, StoreError> {
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

    pub fn user_edits(&self, known: &PlanFile) -> Result<Vec<UserEdit>, StoreError> {
        let disk = self.read(&known.plan.id)?;
        Ok(diff(known, &disk))
    }
}

fn todo_changes(known: &Plan, disk: &Plan, edits: &mut Vec<UserEdit>) {
    for todo in &known.todos {
        if disk.todo(&todo.label).is_none() {
            edits.push(UserEdit::TodoDropped {
                label: todo.label.clone(),
            });
        }
    }
    for todo in &disk.todos {
        let Some(old) = known.todo(&todo.label) else {
            edits.push(UserEdit::TodoAdded {
                todo: Box::new(todo.clone()),
            });
            continue;
        };
        if old.state != todo.state {
            edits.push(UserEdit::TodoStateChanged {
                label: todo.label.clone(),
                from: old.state.clone(),
                to: todo.state.clone(),
            });
        }
        if old.after != todo.after {
            edits.push(UserEdit::TodoEdgesChanged {
                label: todo.label.clone(),
                from: old.after.clone(),
                to: todo.after.clone(),
            });
        }
        if old.delegation != todo.delegation
            || old.subplan != todo.subplan
            || old.retries != todo.retries
            || old.extra != todo.extra
        {
            edits.push(UserEdit::TodoEdited {
                label: todo.label.clone(),
            });
        }
    }
    let known_common: Vec<&TodoLabel> = known
        .todos
        .iter()
        .map(|todo| &todo.label)
        .filter(|label| disk.todo(label).is_some())
        .collect();
    let disk_common: Vec<&TodoLabel> = disk
        .todos
        .iter()
        .map(|todo| &todo.label)
        .filter(|label| known.todo(label).is_some())
        .collect();
    if known_common != disk_common {
        edits.push(UserEdit::TodoReordered {
            order: disk.todos.iter().map(|todo| todo.label.clone()).collect(),
        });
    }
}

fn diff(known: &PlanFile, disk: &PlanFile) -> Vec<UserEdit> {
    let mut edits = Vec::new();
    if known.plan.goal != disk.plan.goal {
        edits.push(UserEdit::GoalReworded {
            from: known.plan.goal.clone(),
            to: disk.plan.goal.clone(),
        });
    }
    if known.plan.state != disk.plan.state {
        edits.push(UserEdit::PlanStateChanged {
            from: known.plan.state.clone(),
            to: disk.plan.state.clone(),
        });
    }
    if known.plan.spawns() != disk.plan.spawns() {
        edits.push(UserEdit::SpawnsChanged {
            from: known.plan.spawns(),
            to: disk.plan.spawns(),
        });
    }
    todo_changes(&known.plan, &disk.plan, &mut edits);
    if known.body.preamble != disk.body.preamble {
        edits.push(UserEdit::PreambleEdited);
    }
    for section in &known.body.sections {
        if disk.body.section(&section.heading) != Some(section.text.as_str()) {
            edits.push(UserEdit::SectionEdited {
                heading: section.heading.clone(),
            });
        }
    }
    for section in &disk.body.sections {
        if known.body.section(&section.heading).is_none() {
            edits.push(UserEdit::SectionEdited {
                heading: section.heading.clone(),
            });
        }
    }
    edits
}

#[cfg(test)]
mod tests {
    use super::*;
    use yi_types::plan::PlanVersion;
    use yi_types::plan::doc::{PlanTier, RetryCount, TouchCount};

    type Fallible = Result<(), Box<dyn std::error::Error>>;

    struct TempStore {
        dir: PathBuf,
        store: PlanStore,
    }

    impl TempStore {
        fn new(name: &str) -> Result<Self, StoreError> {
            let dir =
                std::env::temp_dir().join(format!("yi-plan-store-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let store = PlanStore::open(dir.clone())?;
            Ok(Self { dir, store })
        }
    }

    impl Drop for TempStore {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
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
                    extra: serde_json::Map::new(),
                },
                Todo {
                    label: TodoLabel::new("Implement refresh flow")?,
                    after: vec![TodoLabel::new("Freeze the token API seam")?],
                    state: TodoState::Pending,
                    delegation: None,
                    subplan: None,
                    retries: RetryCount(1),
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
            body: PlanBody::parse("\n## Implement refresh flow\n\nSeam notes.\n"),
        };
        temp.store.write(&file)?;
        let back = temp.store.read(&file.plan.id)?;
        assert_eq!(back, file);
        assert_eq!(
            back.body.section("Implement refresh flow"),
            Some("\nSeam notes.\n")
        );
        assert_eq!(temp.store.list()?, vec![file.plan.id.clone()]);
        assert_eq!(temp.store.roots()?, vec![file.plan.id.clone()]);
        Ok(())
    }

    #[test]
    fn reads_the_section_3_1_example_document() -> Fallible {
        let temp = TempStore::new("example")?;
        let document = concat!(
            "---\n",
            "format: 1\n",
            "plan: 7f3a-auth-refactor\n",
            "goal: Ship OAuth login end to end\n",
            "version: 3\n",
            "tier: root\n",
            "state: active\n",
            "todos:\n",
            "  - label: Freeze the token API seam\n",
            "    state: done\n",
            "    output: kernel://token_api_seam\n",
            "  - label: Implement refresh flow\n",
            "    state: running\n",
            "    by: child-1\n",
            "    after:\n",
            "      - Freeze the token API seam\n",
            "    delegation:\n",
            "      spec:\n",
            "        role: coder\n",
            "        effort: med\n",
            "        isolation: worktree\n",
            "      accept:\n",
            "        command: cargo test -p yi-ai refresh\n",
            "      output:\n",
            "        schema: local://.yi/schemas/refresh_result.json\n",
            "      context:\n",
            "        - plan://7f3a-auth-refactor/seam-notes\n",
            "        - local://docs/auth.md\n",
            "    retries: 1\n",
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
            file.body.section("seam-notes"),
            Some("\nThe refresh endpoint owns rotation.\n")
        );
        temp.store.write(&file)?;
        assert_eq!(temp.store.read(&id)?, file);
        Ok(())
    }

    #[test]
    fn over_cap_frontmatter_is_refused_and_the_file_is_untouched() -> Fallible {
        let temp = TempStore::new("cap")?;
        let mut file = PlanFile {
            plan: plan("Ship OAuth login end to end")?,
            body: PlanBody::default(),
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
            body: PlanBody::default(),
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
            body: PlanBody::parse(&format!("\n## seam-notes\n\n{}\n", "x".repeat(200_000))),
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
    fn yaml_metacharacters_in_labels_and_keys_survive_write_then_read() -> Fallible {
        let temp = TempStore::new("metacharacters")?;
        let mut file = PlanFile {
            plan: plan("Ship OAuth login end to end")?,
            body: PlanBody::default(),
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
                extra,
            });
        }
        temp.store.write(&file)?;
        assert_eq!(temp.store.read(&file.plan.id)?, file);
        Ok(())
    }

    #[test]
    fn a_hand_edit_diffs_into_user_edits() -> Fallible {
        let temp = TempStore::new("edits")?;
        let known = PlanFile {
            plan: plan("Ship OAuth login end to end")?,
            body: PlanBody::parse("\n## seam-notes\n\nold\n"),
        };
        temp.store.write(&known)?;
        let mut edited = known.clone();
        edited.plan.goal = GoalText::new("Ship OAuth login and refresh")?;
        edited.plan.todos.reverse();
        if let Some(todo) = edited.plan.todos.first_mut() {
            todo.state = TodoState::Done { output: None };
        }
        edited.plan.todos.push(Todo {
            label: TodoLabel::new("Write the rollout note")?,
            after: vec![TodoLabel::new("A step nobody added")?],
            state: TodoState::Pending,
            delegation: None,
            subplan: None,
            retries: RetryCount(0),
            extra: serde_json::Map::new(),
        });
        edited.body = PlanBody::parse("\n## seam-notes\n\nnew\n");
        temp.store.write(&edited)?;
        let disk = temp.store.read(&known.plan.id)?;
        assert!(
            !disk.plan.validate().is_empty(),
            "dangling edge still parses"
        );
        let edits = temp.store.user_edits(&known)?;
        assert!(edits.iter().any(|edit| matches!(
            edit,
            UserEdit::GoalReworded { to, .. } if to.as_str() == "Ship OAuth login and refresh"
        )));
        assert!(edits.iter().any(|edit| matches!(
            edit,
            UserEdit::TodoStateChanged { label, to: TodoState::Done { .. }, .. }
                if label.as_str() == "Implement refresh flow"
        )));
        assert!(edits.iter().any(|edit| matches!(
            edit,
            UserEdit::TodoAdded { todo } if todo.label.as_str() == "Write the rollout note"
        )));
        assert!(
            edits
                .iter()
                .any(|edit| matches!(edit, UserEdit::TodoReordered { .. }))
        );
        assert!(edits.iter().any(|edit| matches!(
            edit,
            UserEdit::SectionEdited { heading } if heading == "seam-notes"
        )));
        assert!(temp.store.user_edits(&disk)?.is_empty());
        for edit in &edits {
            assert!(!edit.to_string().is_empty());
        }
        Ok(())
    }
}
