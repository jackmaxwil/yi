use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, StopReason, Usage};
use yi_types::model::{Effort, Model};
use yi_types::subagent::FailClass;
pub use yi_types::subagent::{ChildActivity, ChildExit, ChildId, ChildStatus, ChildUpdate};

use crate::mail::Draft;
use crate::mailbox::{ParentLink, timeout_of};
use crate::session::AgentSession;

mod build;
pub mod models;
mod record;
mod service;
pub use record::ChildFeed;
pub(crate) use record::Step;
use record::preview;
pub(crate) use service::Standing;

pub const DEFAULT_MAX_DEPTH: u8 = 1;
// A completed child holds its slot until closed: the cap forces the parent to
// reap with rlm.delete_subagent instead of leaking children (design B2).
pub const DEFAULT_MAX_CHILDREN: usize = 8;
/// live sessions across the whole family, so a deeper fan-out cannot multiply (D165).
pub const FAMILY_CAP: usize = 16;
pub const PARENT_NAME: &str = "parent";
/// The cause a cut-short run carries on its terminal update and in the parent's notice.
pub(crate) const INTERRUPTED: &str = "interrupted";

pub(crate) struct ChildRecord {
    pub(crate) session_name: String,
    session_dir: PathBuf,
    pub(crate) worktree: Option<crate::lane::Lane>,
    /// The worker share of the lane pool this child's checkout was taken from; dropped with
    /// the lane, so a settled child stops holding a slot nobody is standing in.
    pub(crate) lane_permit: Option<crate::plan::capacity::Permit>,
    /// How the worktree goes at reap, once the engine has journaled it (plan section 6.6).
    pub(crate) disposition: Option<yi_types::plan::op::Choice>,
    /// Dispatched by the plan engine: its worktree goes through `submit` or a journaled
    /// disposition, never the kernel's merge or discard.
    pub(crate) managed: bool,
    /// A juror or a service stands outside the worker cap and the owner's lifecycle notice.
    pub(crate) standing: Standing,
    /// `None` while the run is live; every status, state and notice is read from it.
    pub(crate) exit: Option<ChildExit>,
    pub(crate) phase: crate::family::Phase,
    pub(crate) lease: yi_types::lease::Lease,
    pub(crate) parent_close: yi_types::lease::ParentClose,
    activity: ChildActivity,
    /// Where this incarnation's own turns start in the kept transcript: 0 for every child but
    /// a respawned service, whose predecessor's turns were billed to the lease that ended.
    pub(crate) billed_from: usize,
    tool_use_count: u64,
    token_count: u64,
    answer_preview: Option<String>,
    pub(crate) error: Option<String>,
    /// L3: set makes this a protocol child — its answer must decode as a
    /// [`yi_types::subagent::ChildResult`] and this check must be green.
    pub(crate) check: Option<String>,
    pub(crate) replied: bool,
    pub(crate) changed_at_epoch: u64,
    pub(crate) session: Arc<AgentSession>,
}

/// One lock for the records and the epoch they move under, so the two are never seen apart.
#[derive(Default)]
pub(crate) struct Children {
    records: HashMap<String, ChildRecord>,
    pub(crate) epoch: u64,
    /// Children being built outside the lock; each holds a slot, its name and its tokens.
    building: Vec<(String, u64)>,
}

impl Children {
    /// Tokens out on lease: every record still held, and every build in flight.
    pub(crate) fn reserved(&self) -> u64 {
        let held = self
            .records
            .values()
            .filter_map(|record| record.lease.tokens);
        held.chain(self.building.iter().map(|(_, tokens)| *tokens))
            .fold(0, u64::saturating_add)
    }

    /// A build whose record is listed: its reservation is the record's from here on.
    pub(crate) fn release_build(&mut self, name: &str) {
        self.building.retain(|(held, _)| held != name);
    }

    /// A reaped record still moves the epoch, so an older cursor wakes and re-reads `states`.
    pub(crate) fn touch(&mut self, key: &str) -> u64 {
        self.epoch = self.epoch.saturating_add(1);
        let epoch = self.epoch;
        if let Some(record) = self.records.get_mut(key) {
            record.changed_at_epoch = epoch;
        }
        epoch
    }
}

impl std::ops::Deref for Children {
    type Target = HashMap<String, ChildRecord>;

    fn deref(&self) -> &Self::Target {
        &self.records
    }
}

impl std::ops::DerefMut for Children {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.records
    }
}

/// The feed is event-stream and store access; control goes through the host.
#[derive(Clone)]
pub struct ChildView {
    pub update: ChildUpdate,
    pub session: ChildFeed,
}

pub struct ChildBuild<'a> {
    pub model: Model,
    pub thinking: Option<Effort>,
    pub session_dir: &'a Path,
    /// `Some` only for a B11 worktree child; otherwise the parent's own cwd.
    pub cwd: Option<&'a Path>,
    pub link: ParentLink,
    pub wall: crate::wall::Wall,
    /// The child's lease: its own clock and the tokens its own children may draw on.
    pub deadline: Option<std::time::Duration>,
    pub tokens: Option<u64>,
}

pub type ChildFactory = dyn Fn(ChildBuild<'_>) -> Result<AgentSession, String> + Send + Sync;
pub type NoticeFn = dyn Fn(&str) + Send + Sync;
pub type AttributeFn = dyn Fn(&Usage) + Send + Sync;

pub struct SubagentHostOptions {
    pub depth: u8,
    pub max_depth: u8,
    pub max_children: usize,
    pub parent_session_dir: PathBuf,
    pub defaults: Arc<dyn Fn() -> (Model, Effort) + Send + Sync>,
    pub factory: Arc<ChildFactory>,
    /// A host status notice, delivered as a user-role message.
    pub notice: Arc<NoticeFn>,
    /// The parent's bus: a child's B7 updates ride it, never the child's own.
    pub events: tokio::sync::broadcast::Sender<AgentEvent>,
    /// The parent's live history, read at spawn for a B5 fork seed.
    pub parent_messages: Arc<dyn Fn() -> Vec<AgentMessage> + Send + Sync>,
    /// Repository an isolated child branches its worktree from, and the directory a
    /// non-isolated child simply runs in — the wall is rooted here either way (B11).
    pub cwd: PathBuf,
    /// Where the lane pool lives (`~/.yi/lanes`), and how many slots it has.
    pub home: PathBuf,
    pub lane_slots: u8,
    /// A child's B6 report, injected into the parent's own transcript.
    pub report: Arc<dyn Fn(AgentMessage) + Send + Sync>,
    /// Folds a child's billable usage onto the parent's last assistant message.
    pub attribute: Arc<AttributeFn>,
    /// The plan a discovery's named ancestor task is resolved against.
    pub store: crate::goal::StoreHandle,
    pub plans_dir: PathBuf,
    /// how many sessions the family holds live right now (the shared kernel map) (D165).
    pub family_live: Arc<dyn Fn() -> usize + Send + Sync>,
}

pub struct SubagentHost {
    pub(crate) options: SubagentHostOptions,
    /// The split of the lane pool this host's workers draw on, shared with the plan engine's
    /// verification reserve so both shares are counted over one pool (section 7.6).
    pub(crate) capacity: Arc<crate::plan::capacity::Capacity>,
    /// The run's end (D177): every settle, merge and discard of a lane is bounded by it.
    deadline: Mutex<Option<std::time::Instant>>,
    pub(crate) children: Mutex<Children>,
    /// Invariant: a reap pin names `history://<child>`, and a child's file lives
    /// under a `sub-*` directory no session repo scans, so it is kept by name here.
    pub(crate) reaped: Mutex<HashMap<String, yi_session::SharedSession>>,
    pub(crate) mail: Mutex<crate::mail::Desk>,
    /// Lock order: `mail`, then `children`, then `grant`, then a session store.
    pub(crate) grant: Mutex<crate::lease::Grant>,
}

impl SubagentHost {
    /// A child lane branches from the parent's own HEAD, so its merge lands back in
    /// the parent's checkout rather than on `main`.
    fn claim_child_lane(
        &self,
        child_id: &str,
    ) -> Result<(crate::lane::Lane, crate::plan::capacity::Permit), String> {
        // The worker share, never the whole pool: a lane the workers take here is one the
        // engine cannot verify a candidate in (plan section 7.6).
        let permit = self
            .capacity
            .reserve(crate::plan::capacity::Purpose::Worker)
            .map_err(|error| error.to_string())?;
        let pool = crate::lane::Pool::open(
            &self.options.home,
            &self.options.cwd,
            self.options.lane_slots,
        )
        .map_err(|error| error.to_string())?;
        let head = crate::lane::git(
            &self.options.cwd,
            &["rev-parse", "--verify", "HEAD^{commit}"],
        )
        .map_err(|error| error.to_string())?;
        let lane = pool
            .claim(
                child_id,
                crate::lane::ClaimBase::Commit(head.trim().to_owned()),
            )
            .map_err(|error| error.to_string())?;
        Ok((lane, permit))
    }
}

pub(crate) fn random_suffix() -> Result<String, String> {
    use std::io::Read;
    let mut buffer = [0_u8; 4];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut buffer))
        .map_err(|error| format!("/dev/urandom: {error}"))?;
    let mut out = String::with_capacity(8);
    for byte in buffer {
        out.push_str(&format!("{byte:02x}"));
    }
    Ok(out)
}

fn default_session_name(prompt: &str, child_id: &str) -> String {
    let slug: String = prompt
        .to_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug
        .split('-')
        .filter(|word| !word.is_empty())
        .take(4)
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        format!("subagent-{child_id}")
    } else {
        format!("{slug}-{child_id}")
    }
}

/// How much parent history seeds a child's transcript. `LastN(n)` counts turn boundaries
/// rather than messages, so a child never opens on half of an exchange (B1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fork {
    None,
    All,
    LastN(u64),
}

/// Whether a child edits the parent's checkout or gets a git worktree of its own, so
/// children writing files in parallel cannot overwrite each other (B11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Isolation {
    None,
    Worktree,
}

fn parse_isolation(kwargs: &Map<String, Value>) -> Result<Isolation, String> {
    match kwargs.get("isolation") {
        None | Some(Value::Null) => Ok(Isolation::None),
        Some(Value::String(value)) => match value.trim() {
            "none" => Ok(Isolation::None),
            "worktree" => Ok(Isolation::Worktree),
            other => Err(format!(
                "rlm.run isolation must be \"none\" or \"worktree\", got {other}"
            )),
        },
        Some(other) => Err(format!("rlm.run isolation must be a string, got {other}")),
    }
}

fn parse_fork(kwargs: &Map<String, Value>) -> Result<Fork, String> {
    match kwargs.get("fork") {
        None | Some(Value::Null) => Ok(Fork::None),
        Some(Value::String(value)) => match value.trim() {
            "none" => Ok(Fork::None),
            "all" => Ok(Fork::All),
            other => other
                .parse::<u64>()
                .ok()
                .filter(|turns| *turns > 0)
                .map(Fork::LastN)
                .ok_or_else(|| {
                    format!("rlm.run fork must be \"none\", \"all\", or a positive turn count, got {other}")
                }),
        },
        Some(Value::Number(number)) => number
            .as_u64()
            .filter(|turns| *turns > 0)
            .map(Fork::LastN)
            .ok_or_else(|| format!("rlm.run fork turn count must be positive, got {number}")),
        Some(other) => Err(format!("rlm.run fork must be a string or number, got {other}")),
    }
}

/// The seed budget is the child's window less the reserve compaction needs.
fn seed_for_fork(parent: &[AgentMessage], fork: Fork, window: u64) -> Vec<AgentMessage> {
    let start = match fork {
        Fork::None => return Vec::new(),
        Fork::All => 0,
        Fork::LastN(turns) => {
            let boundaries: Vec<usize> = parent
                .iter()
                .enumerate()
                .filter(|(_, message)| matches!(message, AgentMessage::User { .. }))
                .map(|(index, _)| index)
                .collect();
            let wanted = usize::try_from(turns).unwrap_or(usize::MAX);
            boundaries
                .len()
                .checked_sub(wanted)
                .and_then(|index| boundaries.get(index).copied())
                .unwrap_or(0)
        }
    };
    let budget = window.saturating_sub(yi_context::Settings::default().reserve_tokens.0);
    let mut kept: Vec<AgentMessage> = Vec::new();
    let mut used = 0_u64;
    for message in parent[start.min(parent.len())..].iter().rev() {
        used = used.saturating_add(yi_context::estimate_message(message).0);
        if used > budget {
            break;
        }
        kept.push(message.clone());
    }
    kept.reverse();
    kept
}

fn require_kwargs(kwargs: &Map<String, Value>) -> Result<(), String> {
    let mut unsupported: Vec<&str> = kwargs
        .keys()
        .map(String::as_str)
        .filter(|key| {
            !matches!(
                *key,
                "name"
                    | "model"
                    | "thinking"
                    | "fork"
                    | "isolation"
                    | "deny_write"
                    | "deny_read"
                    | "deny_url"
                    | "context"
                    | "check"
                    | "deadline_s"
                    | "tokens"
                    | "parent_close"
            )
        })
        .collect();
    if unsupported.is_empty() {
        return Ok(());
    }
    unsupported.sort_unstable();
    Err(format!(
        "Unsupported rlm.run kwargs: {}",
        unsupported.join(", ")
    ))
}

fn optional_string(kwargs: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match kwargs.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_owned()))
            }
        }
        Some(other) => Err(format!("rlm.run {key} must be a string, got {other}")),
    }
}

fn child_entry(child_id: &str, record: &ChildRecord) -> Value {
    json!({
        "rlm_child_id": child_id,
        "active_session_id": Value::Null,
        "session_id": Value::Null,
        "session_name": record.session_name,
        "session_dir": record.session_dir.to_string_lossy(),
        "status": crate::family::read_exit(record.exit).status.as_str(),
    })
}

pub(crate) fn last_assistant_text(messages: &[AgentMessage]) -> Option<String> {
    messages.iter().rev().find_map(|message| match message {
        AgentMessage::Assistant { content, .. } => {
            let text = content
                .iter()
                .filter_map(|content| match content {
                    yi_types::message::Content::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            if text.is_empty() { None } else { Some(text) }
        }
        _ => None,
    })
}

/// How a settled run ended, read off its last assistant message; a run a cancel or the
/// deadline stopped at a message boundary ends on a tool call with no request after it.
fn exit_of(session: &AgentSession) -> (ChildExit, Option<String>) {
    let messages = session.messages();
    let last = messages.iter().rev().find_map(|message| match message {
        AgentMessage::Assistant {
            stop_reason,
            error_message,
            ..
        } => Some((stop_reason, error_message)),
        _ => None,
    });
    let out_of_clock = session.deadline().is_some_and(|clock| clock.winding_down());
    match last {
        Some((StopReason::Error, message)) => (
            ChildExit::Failed {
                class: FailClass::Provider,
            },
            Some(
                message
                    .clone()
                    .unwrap_or_else(|| "child run ended with an error".to_owned()),
            ),
        ),
        Some((StopReason::Aborted, _)) => (ChildExit::Interrupted, Some(INTERRUPTED.to_owned())),
        Some((StopReason::ToolUse, _)) if session.cancelled() => {
            (ChildExit::Interrupted, Some("cancelled".to_owned()))
        }
        Some((StopReason::ToolUse, _)) if out_of_clock => (
            ChildExit::Failed {
                class: FailClass::Deadline,
            },
            Some("the deadline ended the run before its task did".to_owned()),
        ),
        _ => (ChildExit::Completed, None),
    }
}

impl SubagentHost {
    pub fn new(options: SubagentHostOptions) -> Self {
        Self {
            capacity: crate::plan::capacity::Capacity::for_slots(options.lane_slots),
            options,
            deadline: Mutex::new(None),
            children: Mutex::new(Children::default()),
            reaped: Mutex::new(HashMap::new()),
            mail: Mutex::default(),
            grant: Mutex::default(),
        }
    }

    /// The lane split, shared with the plan engine so verification and workers count one pool.
    pub fn capacity(&self) -> Arc<crate::plan::capacity::Capacity> {
        Arc::clone(&self.capacity)
    }

    /// The run's end, read by every lane settle this host runs (D177).
    pub fn set_deadline(&self, ends: Option<std::time::Instant>) {
        if let Ok(mut slot) = self.deadline.lock() {
            *slot = ends;
        }
    }

    pub fn deadline(&self) -> Option<std::time::Instant> {
        self.deadline.lock().ok().and_then(|slot| *slot)
    }

    pub fn children_view(&self) -> Vec<ChildView> {
        self.children
            .lock()
            .map(|children| {
                let mut view: Vec<ChildView> = children
                    .iter()
                    .map(|(id, record)| ChildView {
                        update: record.update(id),
                        session: ChildFeed::of(Arc::clone(&record.session)),
                    })
                    .collect();
                view.sort_by(|left, right| left.update.id.cmp(&right.update.id));
                view
            })
            .unwrap_or_default()
    }

    fn publish(&self, child_id: &str) {
        let update = self
            .children
            .lock()
            .ok()
            .and_then(|children| children.get(child_id).map(|record| record.update(child_id)));
        if let Some(update) = update {
            let _ = self.options.events.send(AgentEvent::ChildUpdate { update });
        }
    }

    fn watch(self: &Arc<Self>, child_id: String, session: &Arc<AgentSession>) {
        let host = Arc::clone(self);
        let mut events = session.subscribe();
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(event) => {
                        if matches!(event, AgentEvent::AgentEnd { .. }) {
                            host.turn_ended(&child_id);
                        }
                        let moved = host
                            .children
                            .lock()
                            .ok()
                            .and_then(|mut children| {
                                children
                                    .get_mut(&child_id)
                                    .map(|record| record.step(Step::Event(&event)))
                            })
                            .unwrap_or(false);
                        if moved {
                            host.publish(&child_id);
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        host.refold(&child_id);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    fn create_child_dir(&self, parent_dir: &Path) -> Result<(PathBuf, String), String> {
        std::fs::create_dir_all(parent_dir).map_err(|error| error.to_string())?;
        for _ in 0..100 {
            let suffix = random_suffix()?;
            let dir = parent_dir.join(format!("sub-{suffix}"));
            match std::fs::create_dir(&dir) {
                Ok(()) => return Ok((dir, format!("sub-{suffix}"))),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(format!("{}: {error}", dir.display())),
            }
        }
        Err("Unable to create unique RLM child session directory".to_owned())
    }

    /// Validates, admits, spawns detached, returns the handle at admission.
    pub fn spawn(
        self: &Arc<Self>,
        prompt: String,
        kwargs: Map<String, Value>,
    ) -> Result<Map<String, Value>, String> {
        self.spawn_seated(prompt, kwargs, None)
    }

    /// [`Self::spawn`], or a juror's seat when `seat` holds the verification reserve: a leaf
    /// above the worker cap and the depth limit, still under the family cap and the lease.
    pub(crate) fn spawn_seated(
        self: &Arc<Self>,
        prompt: String,
        kwargs: Map<String, Value>,
        seat: Option<&crate::plan::capacity::Permit>,
    ) -> Result<Map<String, Value>, String> {
        let juror =
            seat.is_some_and(|seat| seat.purpose() == crate::plan::capacity::Purpose::Verification);
        let standing = if juror {
            Standing::Juror
        } else {
            Standing::Worker
        };
        self.admit(prompt, kwargs, standing)
    }

    /// Only a juror is above the depth limit; a juror and a service are above the worker cap.
    fn admit(
        self: &Arc<Self>,
        prompt: String,
        kwargs: Map<String, Value>,
        standing: Standing,
    ) -> Result<Map<String, Value>, String> {
        let juror = matches!(standing, Standing::Juror);
        require_kwargs(&kwargs)?;
        let requested_name = optional_string(&kwargs, "name")?;
        let fork = parse_fork(&kwargs)?;
        let isolation = parse_isolation(&kwargs)?;
        let check = optional_string(&kwargs, "check")?;
        let context = crate::mailbox::context_block(&kwargs)?;
        let ask = crate::lease::Ask::from_kwargs(&kwargs)?;
        let overrides =
            optional_string(&kwargs, "model")?.or(optional_string(&kwargs, "thinking")?);
        if fork == Fork::All && overrides.is_some() {
            return Err(
                "fork=all inherits the parent's model and thinking; drop the override".to_owned(),
            );
        }
        if (self.options.family_live)() >= FAMILY_CAP {
            return Err(format!(
                "the family holds {FAMILY_CAP} live sessions; reap one with rlm.delete_subagent before spawning"
            ));
        }
        if !juror && self.options.depth >= self.options.max_depth {
            return Err(format!(
                "RLM recursion depth limit reached (RLM_DEPTH={}, RLM_MAX_DEPTH={})",
                self.options.depth, self.options.max_depth
            ));
        }
        let cast = self.cast(&kwargs)?;
        let model = cast.0.clone();
        let (session_dir, child_id) = self.create_child_dir(&self.options.parent_session_dir)?;
        let session_name =
            requested_name.unwrap_or_else(|| default_session_name(&prompt, &child_id));
        let capped = matches!(standing, Standing::Worker);
        let (reserved, lease) = self.reserve(&session_name, &session_dir, &ask, capped)?;
        let (worktree, lane_permit) = match isolation {
            Isolation::None => (None, None),
            Isolation::Worktree => {
                let (lane, permit) = self.claim_child_lane(&child_id)?;
                (Some(lane), Some(permit))
            }
        };
        let cwd = worktree.as_ref().map(|lane| lane.path());
        let child = self.build(cast, &session_name, &session_dir, cwd, &lease, None)?;
        if fork != Fork::None {
            let seed = seed_for_fork(
                &(self.options.parent_messages)(),
                fork,
                child.model().context_window,
            );
            child.seed_messages(seed);
        }
        let session = Arc::new(child);
        // Invariant: read before the record is listed, so every abort a client can ask for
        // from here on carries a later epoch and admission cannot clear it.
        let requested = session.abort_epoch();
        {
            let mut children = self
                .children
                .lock()
                .map_err(|_| "subagent state poisoned")?;
            children.insert(
                child_id.clone(),
                ChildRecord {
                    session_name: session_name.clone(),
                    session_dir: session_dir.clone(),
                    worktree,
                    lane_permit,
                    disposition: None,
                    managed: false,
                    standing,
                    exit: None,
                    phase: crate::family::Phase::Queued,
                    lease,
                    parent_close: ask.parent_close,
                    activity: ChildActivity::Waiting,
                    billed_from: 0,
                    tool_use_count: 0,
                    token_count: 0,
                    answer_preview: None,
                    error: None,
                    check,
                    replied: false,
                    changed_at_epoch: 0,
                    session: Arc::clone(&session),
                },
            );
            // The record now carries the lease: a build still listed would reserve it twice,
            // and a spawn racing this one would be refused tokens nobody holds.
            children.release_build(&session_name);
            children.touch(&child_id);
        }
        drop(reserved);
        self.watch(child_id.clone(), &session);
        let host = Arc::clone(self);
        let task_child_id = child_id.clone();
        let task_name = session_name.clone();
        // Invariant: the spawn reply resolves at admission, and blocking here
        // would abort the turn whose cell awaits it.
        tokio::spawn(async move {
            host.run_child(
                task_child_id,
                task_name,
                prompt,
                context,
                session,
                requested,
            )
            .await;
        });
        self.publish(&child_id);
        Ok(service::handle(
            &child_id,
            &session_name,
            &session_dir,
            &model,
        ))
    }

    async fn run_child(
        self: Arc<Self>,
        child_id: String,
        session_name: String,
        prompt: String,
        context: Option<String>,
        session: Arc<AgentSession>,
        requested: u64,
    ) {
        // Invariant: scope rides the first user message, never a trusted block.
        let content = match context {
            Some(block) => format!("[task from parent]\n\n{block}\n\n{prompt}"),
            None => format!("[task from parent]\n\n{prompt}"),
        };
        if let Ok(mut children) = self.children.lock()
            && let Some(record) = children.get_mut(&child_id)
            && record.step(Step::Started)
        {
            children.touch(&child_id);
        }
        let outcome =
            session.prompt_requested(crate::session::user_message(&content), Some(requested));
        let mut ended = outcome.err().map(|error| {
            let class = FailClass::RefusedSpawn;
            (ChildExit::Failed { class }, Some(error.to_string()))
        });
        if ended.is_none() {
            session.wait_idle().await;
            ended = Some(exit_of(&session));
        }
        let (exit, error) = ended.unwrap_or((ChildExit::Completed, None));
        self.conclude(&child_id, &session_name, &session, exit, error);
    }

    /// A run's ending, unless it is a service's crash and the record was respawned instead.
    fn conclude(
        self: &Arc<Self>,
        child_id: &str,
        session_name: &str,
        session: &AgentSession,
        exit: ChildExit,
        error: Option<String>,
    ) {
        let Some((exit, error)) = self.respawn(child_id, exit, error) else {
            return;
        };
        // A retired child has no record left: `retire` already published its terminal update,
        // so a second one, a notice or a bill would only echo a closed slot.
        let (mut replied, mut juror) = (None, false);
        if let Ok(mut children) = self.children.lock()
            && let Some(record) = children.get_mut(child_id)
            && record.step(Step::Exit(exit, error.clone()))
        {
            let service = matches!(record.standing, Standing::Service(_));
            replied = Some((record.replied, service));
            juror = matches!(record.standing, Standing::Juror);
            children.touch(child_id);
        }
        let Some((replied, service)) = replied else {
            return;
        };
        if exit == ChildExit::Completed {
            for message in session.messages() {
                if let AgentMessage::Assistant {
                    stop_reason, usage, ..
                } = &message
                    && !matches!(stop_reason, StopReason::Error | StopReason::Aborted)
                {
                    (self.options.attribute)(usage);
                }
            }
        }
        self.publish(child_id);
        if juror {
            return;
        }
        // Terminal notices reach the parent as user-role host status, never as
        // something that can read as user instructions from the child.
        // a child that ended on `ask_user` is asking its parent, not finishing (D165).
        let question = crate::family::pending_question(&session.messages());
        let verb = crate::family::read_exit(Some(exit)).verb;
        let notice = match (exit, question) {
            (ChildExit::Completed, Some(question)) => format!(
                "[subagent {session_name} ({child_id}) asks: {question}]\nanswer with rlm.send(\"{session_name}\", \"…\", followup=True)"
            ),
            // An idle service has not ended: it waits on its inbox and there is nothing to reap.
            (ChildExit::Completed, None) if service => return,
            (ChildExit::Completed, None) => {
                let answer = last_assistant_text(&session.messages())
                    .map(|text| preview(&text))
                    .unwrap_or_else(|| "(no final answer text)".to_owned());
                let silent = if replied {
                    ""
                } else {
                    "; it sent you no message"
                };
                format!(
                    "[subagent {session_name} ({child_id}) {verb}{silent}]\nLast answer: {answer}\n{}",
                    crate::affordance::next(
                        "rlm.run",
                        &[yi_types::graph::CHILD_FINISHED],
                        session_name
                    )
                )
            }
            (ChildExit::Failed { .. }, _) => format!(
                "[subagent {session_name} ({child_id}) {verb}]\n{}",
                error.unwrap_or_default()
            ),
            _ => format!("[subagent {session_name} ({child_id}) {verb}]"),
        };
        (self.options.notice)(&notice);
    }

    /// every child's state as its own records show it (D165).
    pub fn states(&self) -> Vec<crate::family::MemberView> {
        let now = yi_session::now_ms();
        self.children
            .lock()
            .map(|children| {
                let mut views: Vec<crate::family::MemberView> = children
                    .values()
                    .map(|record| {
                        let recent = record
                            .session
                            .store()
                            .map(|store| crate::family::recent_entries(&store))
                            .unwrap_or_default();
                        let (state, note, idle_s) = crate::family::state_from_records(
                            (record.exit, record.phase),
                            record.error.as_deref(),
                            &record.session.messages(),
                            &recent,
                            now,
                        );
                        crate::family::MemberView {
                            name: record.session_name.clone(),
                            state,
                            note,
                            tools: record.tool_use_count,
                            tokens: record.token_count,
                            idle_s,
                            worktree: record
                                .worktree
                                .as_ref()
                                .map(|lane| lane.path().to_string_lossy().into_owned()),
                        }
                    })
                    .collect();
                views.sort_by(|left, right| left.name.cmp(&right.name));
                views
            })
            .unwrap_or_default()
    }

    pub fn status(&self) -> Map<String, Value> {
        let members: Vec<Value> = self
            .states()
            .into_iter()
            .map(|view| {
                let incarnation = self.incarnation_of(&view.name);
                json!({
                    "service": incarnation.is_some(),
                    "incarnation": incarnation,
                    "name": view.name,
                    "state": view.state.as_str(),
                    "note": view.note,
                    "tools": view.tools,
                    "tokens": view.tokens,
                    "idle_s": view.idle_s,
                    "worktree": view.worktree,
                })
            })
            .collect();
        let mut reply = Map::new();
        reply.insert("members".to_owned(), Value::Array(members));
        reply
    }

    pub fn list(&self) -> Map<String, Value> {
        let entries: Vec<Value> = self
            .children
            .lock()
            .map(|children| {
                let mut entries: Vec<(String, Value)> = children
                    .iter()
                    .map(|(id, record)| (id.clone(), child_entry(id, record)))
                    .collect();
                entries.sort_by(|left, right| left.0.cmp(&right.0));
                entries.into_iter().map(|(_, entry)| entry).collect()
            })
            .unwrap_or_default();
        let mut reply = Map::new();
        reply.insert("subagents".to_owned(), Value::Array(entries));
        reply
    }

    pub(crate) fn dispose_child_kernel(session: &AgentSession) {
        session.dispose_kernel();
    }

    pub(crate) fn key_of(
        children: &HashMap<String, ChildRecord>,
        target: &str,
    ) -> Result<String, String> {
        children
            .iter()
            .find(|(id, record)| id.as_str() == target || record.session_name == target)
            .map(|(id, _)| id.clone())
            .ok_or_else(|| format!("No RLM child matches \"{target}\""))
    }

    pub fn transcript(&self, target: &str) -> Option<yi_session::SharedSession> {
        let children = self.children.lock().ok()?;
        let key = Self::key_of(&children, target).ok()?;
        children.get(&key)?.session.store()
    }

    /// a member's checkout for `tree://<agent>/<path>`: the parent's own cwd, or the (D164)
    /// worktree an isolated child holds.
    pub fn cwd_of(&self, target: &str) -> Option<PathBuf> {
        if target == "main" {
            return Some(self.options.cwd.clone());
        }
        let children = self.children.lock().ok()?;
        let key = Self::key_of(&children, target).ok()?;
        let child = children.get(&key)?;
        Some(child.worktree.as_ref().map_or_else(
            || self.options.cwd.clone(),
            |lane| lane.path().to_path_buf(),
        ))
    }

    pub fn kept_transcript(&self, target: &str) -> Option<yi_session::SharedSession> {
        self.reaped.lock().ok()?.get(target).cloned()
    }

    /// Invariant: a reap asks that a child no longer run, so a host holding no such child has
    /// answered it; the caller skips rather than refuses, keeping a cascade retryable.
    pub fn holds(&self, target: &str) -> bool {
        self.children
            .lock()
            .is_ok_and(|children| Self::key_of(&children, target).is_ok())
    }

    pub fn delete(&self, target: &str) -> Result<Map<String, Value>, String> {
        let (key, record, settled) = self.retire(target)?;
        let mut reply = Map::new();
        reply.insert("subagent".to_owned(), child_entry(&key, &record));
        if let Some((choice, candidate)) = settled {
            let mut disposition = candidate.as_reply();
            disposition.insert(
                "choice".to_owned(),
                serde_json::to_value(choice).map_err(|error| error.to_string())?,
            );
            reply.insert("disposition".to_owned(), Value::Object(disposition));
        }
        Ok(reply)
    }

    pub fn register(self: &Arc<Self>, registry: &mut crate::kernel::HostRegistry) {
        let host = Arc::clone(self);
        registry.register("agent_message.send", move |payload| {
            let reply = host.send(PARENT_NAME, &payload);
            Box::pin(async move { reply })
        });
        let host = Arc::clone(self);
        registry.register("agent_message.request", move |payload| {
            let host = Arc::clone(&host);
            Box::pin(async move {
                let (target, draft) = Draft::from_payload(&payload)?;
                let timeout = timeout_of(&payload);
                host.request(PARENT_NAME, &target, &draft.text, timeout)
                    .await
            })
        });
        let host = Arc::clone(self);
        registry.register("agent_message.list_agents", move |_payload| {
            let reply = host.roster();
            Box::pin(async move { Ok(reply) })
        });
        let host = Arc::clone(self);
        registry.register("rlm.wait", move |payload| {
            let timeout = timeout_of(&payload);
            let cursor = payload.get("cursor").and_then(Value::as_u64);
            let host = Arc::clone(&host);
            Box::pin(async move { Ok(host.wait(timeout, cursor).await) })
        });
        self.register_stops(registry);
        self.register_service(registry);
        let host = Arc::clone(self);
        registry.register("rlm.result", move |payload| {
            let target = payload
                .get("target")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let schema = payload
                .get("schema")
                .cloned()
                .filter(|value| !value.is_null());
            let host = Arc::clone(&host);
            Box::pin(async move {
                let target = target.ok_or("rlm.result requires a target")?;
                // A protocol child's result runs checks; keep them off the executor.
                tokio::task::spawn_blocking(move || host.result(&target, schema.as_ref()))
                    .await
                    .map_err(|error| format!("rlm.result task failed: {error}"))?
            })
        });
        let host = Arc::clone(self);
        registry.register("rlm.run", move |payload| {
            let prompt = payload
                .get("prompt")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let kwargs = payload
                .get("kwargs")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let host = Arc::clone(&host);
            Box::pin(async move {
                let prompt = prompt.ok_or("rlm.run requires a prompt")?;
                host.spawn(prompt, kwargs)
            })
        });
        let host = Arc::clone(self);
        registry.register("rlm.list_subagents", move |_payload| {
            let reply = host.list();
            Box::pin(async move { Ok(reply) })
        });
        let host = Arc::clone(self);
        registry.register("rlm.status", move |_payload| {
            let reply = host.status();
            Box::pin(async move { Ok(reply) })
        });
        let host = Arc::clone(self);
        registry.register("rlm.delete_subagent", move |payload| {
            let target = payload
                .get("target")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let host = Arc::clone(&host);
            Box::pin(async move {
                let target = target.ok_or("rlm.delete_subagent requires a target")?;
                host.delete(&target)
            })
        });
        for (method, merges) in [
            ("rlm.merge_worktree", true),
            ("rlm.discard_worktree", false),
        ] {
            let host = Arc::clone(self);
            registry.register(method, move |payload| {
                let target = payload
                    .get("target")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let host = Arc::clone(&host);
                Box::pin(async move {
                    let target = target.ok_or_else(|| format!("{method} requires a target"))?;
                    if merges {
                        host.merge_worktree(&target)
                    } else {
                        host.discard_worktree(&target)
                    }
                })
            });
        }
        registry.register("rlm.find_models", move |payload| {
            let query = payload
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let limit = payload
                .get("limit")
                .and_then(Value::as_u64)
                .map(|limit| usize::try_from(limit).unwrap_or(8))
                .unwrap_or(8);
            let reply = Self::find_models(&query, limit);
            Box::pin(async move { Ok(reply) })
        });
        let defaults = Arc::clone(&self.options.defaults);
        registry.register("model.info", move |_payload| {
            let (model, effort) = defaults();
            let reply = json!({
                "provider": model.provider,
                "id": model.id,
                "name": model.name,
                "selector": format!("{}/{}", model.provider, model.id),
                "input": model.input,
                "thinking": effort.to_string(),
            })
            .as_object()
            .cloned()
            .unwrap_or_default();
            Box::pin(async move { Ok(reply) })
        });
    }
}

impl crate::fetch::MemberTrees for SubagentHost {
    fn cwd_of(&self, agent: &str) -> Option<PathBuf> {
        SubagentHost::cwd_of(self, agent)
    }
}
