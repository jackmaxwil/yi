use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Map, Value};
use yi_types::message::HostSource;
use yi_types::model::Model;

use crate::args::Args;
use crate::mailbox::{ParentLink, register_child_messaging};
use crate::session::AgentSession;
use crate::subagent::{ChildBuild, ChildFactory, SubagentHost, SubagentHostOptions};

/// A child is a fresh session with its own extensions and cwd, on its family's stream (D314).
fn child_factory(wiring: RuntimeWiring) -> Arc<ChildFactory> {
    Arc::new(move |build: ChildBuild<'_>| {
        let provider = Arc::new(wiring.provider.for_child());
        if let Some(reader) = build.reader.clone() {
            let cwd = build
                .cwd
                .map_or_else(|| wiring.cwd.clone(), Path::to_path_buf);
            let broker = child_broker(&wiring, &build.wall, &cwd);
            let tools = (wiring.tools)();
            let rules = crate::rules::discover_armed(&cwd, &wiring.home).rules;
            let rules = Some(Arc::new(crate::rules::RuleEngine::new(rules)));
            return Ok(crate::subagent::reader::session(
                provider,
                build,
                &reader,
                tools,
                (cwd, &wiring.home),
                broker,
                rules,
            ));
        }
        let mut child = AgentSession::new(
            crate::session::SessionConfig {
                system_prompt: wiring.system_prompt.clone(),
                model: build.model,
                thinking_level: build.thinking,
                tool_execution: wiring.tool_execution,
            },
            Arc::clone(&provider),
        );
        let child_cwd = build
            .cwd
            .map_or_else(|| wiring.cwd.clone(), Path::to_path_buf);
        child.install_extensions(crate::ext::install(crate::ext::ExtOptions {
            cwd: child_cwd.clone(),
            home: wiring.home.clone(),
            mode: wiring
                .broker
                .as_ref()
                .map_or(yi_permission::PermissionMode::Auto, |broker| broker.mode()),
            user_system: String::new(),
            schema_instruction: None,
            context_window: child.model().context_window,
            global_skills: Vec::new(),
        }));
        let host = attach_runtime(
            &mut child,
            RuntimeWiring {
                broker: child_broker(&wiring, &build.wall, &child_cwd),
                depth: wiring.depth.saturating_add(1),
                rlm_dir: build.session_dir.to_path_buf(),
                family_dir: Some(wiring.family_dir()),
                cwd: child_cwd,
                parent_link: Some(build.link),
                wall: build.wall,
                deadline: build.deadline,
                kernel_prewarm: false,
                provider,
                ..wiring.clone()
            },
        );
        host.set_grant(child.wall(), build.tokens);
        Ok(child)
    })
}

/// Rules flow down only: a child's broker starts from a copy of its parent's (#600).
fn child_broker(
    wiring: &RuntimeWiring,
    wall: &crate::wall::Wall,
    cwd: &Path,
) -> Option<Arc<crate::permission::PermissionBroker>> {
    (wiring.broker.as_ref()).map(|broker| Arc::new(broker.for_child(wall, cwd)))
}

/// Carried again by every child one level deeper.
#[derive(Clone)]
pub struct RuntimeWiring {
    pub provider: Arc<crate::provider::ProviderStream>,
    pub system_prompt: String,
    pub tool_execution: yi_loop::ExecutionMode,
    pub cwd: PathBuf,
    pub home: PathBuf,
    /// `lanes.slots`: how many worktree slots the repo's pool holds.
    pub lane_slots: u8,
    pub broker: Option<Arc<crate::permission::PermissionBroker>>,
    pub tools: Arc<dyn Fn() -> Vec<Arc<dyn yi_tools::Tool>> + Send + Sync>,
    pub depth: u8,
    pub max_depth: u8,
    pub rlm_dir: PathBuf,
    /// The family board keyed by the root session (D242); `None`, as before the store is
    /// known, falls back to `rlm_dir`'s own `family/`. Children inherit it.
    pub family_dir: Option<PathBuf>,
    /// §5 roles resolved to models; `None` keeps the session's own model.
    pub summarizer: Option<Model>,
    /// Naming `models.advisor` in config enables the LLM reviewer (D28).
    pub advisor: Option<Model>,
    /// Model consulted on a Write or Exec permission ask; `None` keeps admission fully
    /// deterministic, so no permission decision costs a model call (§8, D81).
    pub auto_review: Option<Model>,
    /// `plan.staleReminderTurns` config; None keeps the default.
    pub plan_stale_turns: Option<u64>,
    /// `plans.dir` config; None reads `.yi/plans` under the cwd. Resolved once
    /// at the root so worktree children share the owner's store.
    pub plans_dir: Option<PathBuf>,
    /// Set for a child: its §12 route back into the family that spawned it.
    pub parent_link: Option<ParentLink>,
    /// The wall reduction: paths this session may not touch (plan §3.4 wall).
    pub wall: crate::wall::Wall,
    /// `bash.autoBackgroundMs`, for a call that passes no `wait`; None backgrounds only one that does.
    pub auto_background: Option<std::time::Duration>,
    /// `--deadline`: the run's wall clock, counted down in the environment block and enforced.
    pub deadline: Option<std::time::Duration>,
    /// `kernel.prewarm` (default true): boot the kernel in the background at
    /// session open. Children never prewarm — they spawn to run a cell now.
    pub kernel_prewarm: bool,
    /// Invariant: only yi-cli may see yi-mcp-cli, so `mcp://` reaches a server through a
    /// reader the root supplies; absent, the scheme refuses rather than opening a socket.
    pub mcp_read: Option<Arc<dyn crate::fetch::McpResourceRead>>,
    /// The session corpus root, so `history://<session-id>` reaches a run other
    /// than this one; absent, the corpus is this session and its live children.
    pub sessions_dir: Option<PathBuf>,
    /// Invariant: created once at the composition root and carried down every child, because
    /// `kernel://<child>/var` reads another session's namespace; a per-session map cannot.
    pub kernels: Arc<crate::fetch::KernelServiceMap>,
}

pub(crate) fn family_dir_of(rlm_dir: &std::path::Path) -> PathBuf {
    let mut dir = rlm_dir;
    while dir
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with("sub-"))
        && let Some(parent) = dir.parent()
    {
        dir = parent;
    }
    dir.join("family")
}

/// Invariant: every sandboxed family member can plant a link on the board (D240), so host
/// code reads and writes only regular files there and never follows one.
pub(crate) fn is_board_file(path: &Path) -> bool {
    refuse_linked_board(path).is_ok()
        && std::fs::symlink_metadata(path).is_ok_and(|seen| seen.file_type().is_file())
}

/// A board that is itself a link, planted before the host made it (#757), would carry every
/// read and write below it elsewhere, past the kernel's read denials.
fn refuse_linked_board(path: &Path) -> std::io::Result<()> {
    match path.parent() {
        Some(dir) if std::fs::symlink_metadata(dir).is_ok_and(|seen| seen.is_symlink()) => {
            Err(std::io::Error::other(format!(
                "the family board {} is a link, which is never followed",
                dir.display()
            )))
        }
        _ => Ok(()),
    }
}

/// The host makes the board before a kernel boots under a grant of it: the grant covers the
/// board but not its parent, and whatever a member planted in its place is removed unfollowed.
pub(crate) fn make_board(dir: &Path) -> PathBuf {
    let unplanted = match std::fs::symlink_metadata(dir) {
        Ok(seen) if !seen.is_dir() => std::fs::remove_file(dir),
        _ => Ok(()),
    };
    if let Err(error) = unplanted.and_then(|()| std::fs::create_dir_all(dir)) {
        eprintln!(
            "kernel: the family board {} could not be made: {error}",
            dir.display()
        );
    }
    dir.to_path_buf()
}

/// The file opened must be the one `lstat` saw, so a link swapped in between is refused too.
pub(crate) fn read_board(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    use std::os::unix::fs::MetadataExt;
    refuse_linked_board(path)?;
    let seen = std::fs::symlink_metadata(path)?;
    let mut file = std::fs::File::open(path)?;
    let opened = file.metadata()?;
    if !seen.file_type().is_file() || (seen.dev(), seen.ino()) != (opened.dev(), opened.ino()) {
        return Err(std::io::Error::other(format!(
            "{} is a link on the family board, which is never followed",
            path.display()
        )));
    }
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    Ok(text)
}

/// A fresh file created exclusively, then renamed over `path`: rename replaces a link.
pub(crate) fn write_board(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    refuse_linked_board(path)?;
    yi_session::replace_file(path, bytes)
}

impl RuntimeWiring {
    /// The shared board: the root session's `family/<id>` once its store is known (D242), else
    /// the root `rlm_dir`'s `family/`, the first non-`sub-*` ancestor of a child's (D164).
    pub fn family_dir(&self) -> PathBuf {
        self.family_dir
            .clone()
            .unwrap_or_else(|| family_dir_of(&self.rlm_dir))
    }

    /// The kernel's own profile, which its `bash()` jobs run under too (D241).
    fn exec_sandbox(&self) -> Option<yi_tools::Sandbox> {
        self.session_sandbox()
            .map(|sandbox| crate::kernel::kernel_profile(&sandbox, Some(&self.family_dir())))
    }

    /// The kernel's own writable dir, unless it is the root's corpus (#580).
    fn writable_kernel_dir(&self) -> Option<PathBuf> {
        (self.depth > 0 || self.sessions_dir.is_none()).then(|| self.kernel_dir())
    }

    /// The kernel's and its `bash()` jobs' profile holds the wall and a contained call's walled
    /// roots (#889), since a cell opens files past the tool seam; the board reads again.
    fn session_sandbox(&self) -> Option<yi_tools::Sandbox> {
        let private = self.writable_kernel_dir();
        let mut sandbox = crate::workspace_sandbox(&self.cwd, &self.home, private.as_deref())?;
        sandbox.host_owned.extend(self.sessions_dir.clone());
        sandbox.deny_write.extend_from_slice(&self.wall.deny_write);
        sandbox.deny_read.extend_from_slice(&self.wall.deny_read);
        if !self.wall.is_empty() {
            let roots = crate::tools::walled_roots(&self.wall, self.broker.as_deref());
            sandbox.deny_read.extend(roots);
            let board =
                Some(self.family_dir()).filter(|dir| crate::tools::unwalled(&self.wall, dir));
            sandbox.spared.extend(board);
        }
        Some(sandbox)
    }

    /// A walled session's own spill dir, transcript and kernel state, named as a kernel boots or a
    /// job spawns (a child's store attaches after its wiring); a root's state is not its corpus.
    fn own_paths(&self, session: &AgentSession) -> Option<OwnPathsFn> {
        let (key, store, wall) = (
            session.store_id_hook(),
            session.store_handle(),
            self.wall.clone(),
        );
        let own = move |state: Option<&Path>| {
            let root = crate::tools::default_spill_root();
            let spills = crate::tools::own_spill_dir(root.as_deref(), key());
            let file =
                store().and_then(|store| yi_session::lock_session(&store).file_path().cloned());
            (spills
                .into_iter()
                .chain(file)
                .chain(state.map(Path::to_path_buf)))
            .filter(|dir| crate::tools::unwalled(&wall, dir))
            .collect()
        };
        (!self.wall.is_empty()).then(|| Arc::new(own) as OwnPathsFn)
    }

    fn kernel_options(
        &self,
        host: Arc<dyn yi_kernel::client::HostHandlers>,
        on_restore: Arc<crate::kernel::RestoreNoticeFn>,
        snapshot_key: Arc<dyn Fn() -> Option<String> + Send + Sync>,
        on_boot: Arc<crate::kernel::BootFn>,
    ) -> crate::kernel::KernelServiceOptions {
        crate::kernel::KernelServiceOptions {
            cwd: self.cwd.clone(),
            home: self.home.clone(),
            session_dir: Some(self.kernel_dir()),
            family_dir: Some(self.family_dir()),
            host,
            on_restore: Some(on_restore),
            on_boot: Some(on_boot),
            sandbox: self.session_sandbox(),
            snapshot_key: Some(snapshot_key),
            per_session_state: self.depth == 0 && self.sessions_dir.is_some(),
            cell_ceiling: None,
        }
    }

    /// The kernel's snapshot, `RLM_SESSION_DIR` and writable root. Incident: the root's was
    /// `rlm-<pid>`, so `--continue` never found its snapshot; a child keeps its `sub-*`.
    fn kernel_dir(&self) -> PathBuf {
        self.sessions_dir
            .as_ref()
            .filter(|_| self.depth == 0)
            .unwrap_or(&self.rlm_dir)
            .clone()
    }
}

/// A walled session's own paths as of the call, given its kernel's state dir: read again under
/// its profile's denials.
pub(crate) type OwnPathsFn = Arc<dyn Fn(Option<&Path>) -> Vec<PathBuf> + Send + Sync>;

/// A kernel `bash()` job's profile, built as it spawns: the kernel's own plus every write
/// grant kept since, so a grant reaches a kernel that was running before it (#600).
fn job_profile(
    wiring: &RuntimeWiring,
    session: &AgentSession,
) -> impl Fn() -> Option<yi_tools::Sandbox> + Send + Sync + 'static {
    let (profile, broker) = (wiring.exec_sandbox(), wiring.broker.clone());
    let (own, state) = (wiring.own_paths(session), wiring.writable_kernel_dir());
    move || {
        let mut profile = profile.clone()?;
        let kept = broker.iter().flat_map(|broker| broker.kept_writes());
        profile.writable.extend(kept);
        profile
            .spared
            .extend(own.iter().flat_map(|own| own(state.as_deref())));
        Some(profile)
    }
}

/// Every spawned child wires itself the same way at depth+1; the depth check in
/// [`SubagentHost::spawn`] is what terminates the recursion.
fn wire_schedule(
    session: &AgentSession,
    wiring: &RuntimeWiring,
    registry: &mut crate::kernel::HostRegistry,
) {
    let shared =
        crate::schedule::shared::intern(wiring.rlm_dir.join("scheduled-jobs.json"), |_| {});
    let heartbeats_cwd = wiring.cwd.to_string_lossy().into_owned();
    let deliver = session.heartbeat_deliverer();
    let heartbeats =
        crate::schedule::HeartbeatService::new(Arc::clone(&shared.store), heartbeats_cwd)
            .with_lane(Arc::clone(&shared.hub), Arc::clone(&deliver))
            .with_stop(session.halt_hook())
            .with_words(session.store_handle())
            .with_channels(
                wiring
                    .sessions_dir
                    .as_ref()
                    .unwrap_or(&wiring.rlm_dir)
                    .join("channels"),
            )
            .with_gate(crate::tools::heartbeat_gate(
                wiring,
                session.rules_handle(),
                wiring.own_paths(session),
            ))
            .interned();
    crate::schedule::adapter::adapters_home(&wiring.home);
    let heartbeats = Arc::new(match (&wiring.sessions_dir, wiring.depth) {
        (Some(sessions), 0) => heartbeats.durable(sessions.join("schedules")),
        _ => heartbeats,
    });
    heartbeats.register(registry);
    session.set_schedule(Arc::clone(&heartbeats));
}

fn wire_goal(
    session: &AgentSession,
    registry: &mut crate::kernel::HostRegistry,
    wiring: &RuntimeWiring,
    plans_dir: &Path,
) {
    let plan_stale_turns = wiring.plan_stale_turns;
    let service = crate::goal::attach_goal(session, plans_dir.to_path_buf(), wiring.cwd.clone());
    service.register(registry);
    session.set_goal_service(service);
    let plan = crate::plan::attach_plan(session, plan_stale_turns, plans_dir.to_path_buf());
    plan.register(registry);
    session.set_plan_service(plan);
}

fn wire_fetch(
    session: &AgentSession,
    wiring: &RuntimeWiring,
    plans_dir: &Path,
    host: &Arc<SubagentHost>,
    registry: &mut crate::kernel::HostRegistry,
    log: Arc<crate::fetch::FetchLog>,
    kernels: Arc<crate::fetch::KernelServiceMap>,
) -> Arc<crate::fetch::Resolver> {
    let transcripts = Arc::new(crate::fetch::SessionTranscripts::new(
        Arc::clone(host),
        wiring.sessions_dir.clone(),
        &wiring.cwd,
    ));
    let agent = wiring
        .parent_link
        .as_ref()
        .map_or_else(|| "main".to_owned(), |link| link.child_name.clone());
    let mut resolver = crate::fetch::Resolver::for_child(
        wiring.cwd.clone(),
        wiring.wall.clone(),
        agent,
        session.store_handle(),
        wiring.broker.as_deref(),
    )
    .with_plans_dir(plans_dir.to_path_buf())
    .with_log(log)
    .with_kernel_variables(kernels)
    .with_transcripts(transcripts)
    .with_family_dir(wiring.family_dir())
    .with_member_trees(Arc::clone(host) as Arc<dyn crate::fetch::MemberTrees>)
    .with_home(wiring.home.clone());
    if let Ok(show) = crate::fetch::open_checkpoint_show(&wiring.home, &wiring.cwd) {
        resolver = resolver.with_checkpoint_show(show);
    }
    if let Some(read) = wiring.mcp_read.clone() {
        resolver = resolver.with_mcp_read(read);
    }
    let resolver = Arc::new(resolver);
    host.set_resolver(Arc::clone(&resolver));
    let handler = Arc::clone(&resolver);
    registry.register("fetch", move |payload| {
        let resolver = Arc::clone(&handler);
        Box::pin(async move {
            let raw = payload
                .str_of("url")
                .ok_or_else(|| "fetch requires a \"url\" argument".to_owned())?
                .to_owned();
            let url: yi_types::url::Url = raw
                .parse()
                .map_err(|error: yi_types::url::UrlError| format!("{raw}: {error}"))?;
            let page = crate::fetch::Page::from_payload(&payload, "fetch")?;
            // a family member asks for the object; the owner dills it to the family dir (D164).
            if payload.bool_of("object") == Some(true) {
                if page.is_some() {
                    return Err(
                        "fetch pages text; drop \"object\" to page a kernel:// read".to_owned()
                    );
                }
                let dump = Arc::clone(&resolver);
                let (path, bytes) = tokio::task::spawn_blocking(move || dump.dump_kernel(&url))
                    .await
                    .map_err(|error| format!("fetch task failed: {error}"))?
                    .map_err(|error| error.to_string())?;
                let mut reply = Map::new();
                reply.insert("url".to_owned(), Value::String(raw));
                reply.insert(
                    "path".to_owned(),
                    Value::String(path.to_string_lossy().into_owned()),
                );
                reply.insert("bytes".to_owned(), Value::from(bytes));
                return Ok(reply);
            }
            let fetched = tokio::task::spawn_blocking(move || resolver.fetch_page(&url, page))
                .await
                .map_err(|error| format!("fetch task failed: {error}"))?
                .map_err(|error| error.to_string())?;
            Ok(fetched.into_reply(page.is_some()))
        })
    });
    register_history_grep(registry, session.store_handle());
    if let Some(dir) = wiring.sessions_dir.clone().filter(|_| wiring.depth == 0) {
        crate::history::register(registry, dir, wiring.cwd.clone());
    }
    crate::memory::attach(
        Some(session),
        registry,
        wiring.home.clone(),
        wiring.cwd.clone(),
        wiring.depth == 0,
    );
    resolver
}

pub fn register_history_grep(
    registry: &mut crate::kernel::HostRegistry,
    handle: crate::goal::StoreHandle,
) {
    registry.register("history.grep", move |payload| {
        let handle = Arc::clone(&handle);
        Box::pin(async move {
            let pattern = payload
                .str_of("pattern")
                .map(str::trim)
                .filter(|pattern| !pattern.is_empty())
                .ok_or_else(|| "history.grep requires a \"pattern\" argument".to_owned())?
                .to_owned();
            let number = |key: &str, default: usize| {
                payload
                    .get(key)
                    .and_then(Value::as_u64)
                    .map_or(default, |n| usize::try_from(n).unwrap_or(default))
            };
            let (offset, limit) = (number("offset", 0), number("limit", 8));
            let page = tokio::task::spawn_blocking(move || {
                let shared = handle()
                    .ok_or_else(|| "history.grep: missing an attached session store".to_owned())?;
                let store = yi_session::lock_session(&shared);
                Ok::<_, String>((store.grep_page(&pattern, offset, limit), pattern))
            })
            .await
            .map_err(|error| format!("history.grep task failed: {error}"))??;
            let ((hits, total), pattern) = page;
            Ok(grep_reply(&hits, total, &pattern, offset, limit))
        })
    });
}

fn grep_reply(
    hits: &[yi_session::HistoryHit],
    total: usize,
    pattern: &str,
    offset: usize,
    limit: usize,
) -> Map<String, Value> {
    let mut reply = Map::new();
    let items = hits
        .iter()
        .map(|hit| {
            let mut item = Map::new();
            item.insert("entryId".to_owned(), Value::String(hit.entry_id.clone()));
            item.insert("type".to_owned(), Value::String(hit.entry_type.clone()));
            item.insert("snippet".to_owned(), Value::String(hit.snippet.clone()));
            Value::Object(item)
        })
        .collect();
    reply.insert("hits".to_owned(), Value::Array(items));
    reply.insert("total".to_owned(), Value::from(total));
    let next = offset.saturating_add(hits.len());
    if next < total {
        let cap = limit.clamp(1, yi_session::GREP_PAGE_MAX);
        let quoted = Value::from(pattern);
        reply.insert(
            "notice".to_owned(),
            Value::from(format!(
                "[{} of {total} hits · limit {cap} (at most {}) · compact.recall({quoted}, limit={cap}, offset={next}) for the next]",
                hits.len(),
                yi_session::GREP_PAGE_MAX,
            )),
        );
    }
    reply
}

/// The engine and `plan.op` with this session's principal; `None` (no plan surface at all)
/// when the store is unavailable or a child's name is not a valid agent id.
fn wire_plan_request(
    session: &AgentSession,
    wiring: &RuntimeWiring,
    plans_dir: &Path,
    host: &Arc<SubagentHost>,
    registry: &mut crate::kernel::HostRegistry,
    log: Arc<crate::fetch::FetchLog>,
    resolver: Arc<crate::fetch::Resolver>,
) -> Option<PlanWiring> {
    // Invariant: a child's engine stands on its parent's host and checkout, since its `submit`
    // settles the lane the parent holds and integrates onto the parent's generation (6.6).
    let (actor, host, cwd) = if wiring.depth == 0 {
        (
            crate::plan::ops::Actor::Owner,
            Arc::clone(host),
            wiring.cwd.clone(),
        )
    } else {
        let link = wiring.parent_link.as_ref()?;
        let name = yi_types::plan::doc::AgentId::new(&link.child_name).ok()?;
        let parent = link.host.upgrade()?;
        let cwd = parent.options.cwd.clone();
        (crate::plan::ops::Actor::Child(name), parent, cwd)
    };
    let host = &host;
    let store = match crate::plan::store::PlanStore::open(plans_dir.to_path_buf()) {
        Ok(store) => store,
        Err(error) => {
            (session.notice_hook(HostSource::Notice))(&format!("plan store unavailable: {error}"));
            return None;
        }
    };
    let deliver: crate::goal::DeliverFn = {
        let hook = session.heartbeat_hook();
        Arc::new(move |message, mode| hook(message, mode))
    };
    let delegate = Arc::new(crate::plan::dispatch::SessionDelegate::new(
        Arc::clone(host),
        deliver,
        log,
    ));
    let todo_actor = wiring
        .parent_link
        .as_ref()
        .map_or_else(|| "main".to_owned(), |link| link.child_name.clone());
    let todos = crate::todo::TodoStore::new(session.store_handle(), todo_actor);
    let mut ops: Arc<dyn crate::plan::ops::OpSink> =
        Arc::new(crate::plan::ledger::SessionOpSink(session.store_handle()));
    if wiring.depth == 0 {
        todos.set_resync(crate::todo::mirror::Mirror::resync(store.clone()));
        todos.resync();
        ops = Arc::new(crate::todo::mirror::Mirror {
            inner: ops,
            todos: Arc::clone(&todos),
            store: store.clone(),
        });
    }
    let liveness: Arc<dyn crate::plan::recovery::Liveness> = delegate.clone();
    let mut engine = crate::plan::ops::PlanEngine::new(store, delegate)
        .with_output_resolve(resolver)
        .with_op_sink(ops)
        .with_liveness(liveness)
        .with_cwd(cwd.clone())
        .with_owned(owned_roots(session, host))
        .with_owner_words(session.store_handle());
    // This session's host seats the juries (plan section 6.4). A verification never outlives
    // the run (D177): the verifier and every lane settle read the session's deadline too.
    let mut verifier = crate::plan::verify::Verifier::new(crate::goal::DEFAULT_CHECK_TIMEOUT_MS)
        .with_judge(Arc::new(crate::plan::judge::Jury::new(Arc::clone(host))));
    if let Some(deadline) = session.deadline()
        && let Some(ends) = deadline.started.checked_add(deadline.total)
    {
        verifier = verifier.with_deadline(ends);
        host.set_deadline(Some(ends));
    }
    engine = engine.with_verifier(verifier);
    // The staging and verification checkouts come from the lane pool, split with the host's
    // workers over one object (sections 6.6 and 7.6), opened at the first checkout needed.
    engine = engine
        .with_lane_home(wiring.home.clone(), wiring.lane_slots)
        .with_capacity(host.capacity());
    // The verification snapshot is the shadow gitdir tree the turn checkpoints capture (plan
    // section 6.3); without git the engine hashes the workspace itself.
    if let Some(snapshotter) = crate::plan::snapshot::shadow_tree(&wiring.home, &cwd, plans_dir) {
        engine = engine.with_snapshotter(snapshotter);
    }
    let engine = Arc::new(engine);
    if wiring.depth == 0 {
        todos.set_carry(crate::todo::mirror::carry(Arc::downgrade(&engine)));
    }
    crate::plan::request::register(Arc::clone(&engine), actor.clone(), registry);
    Some((engine, actor, todos))
}

/// A child works its owner's plan, so its scope is its own session's and the host's.
/// ponytail: one level up; a grandchild naming no plan sees its parent's roots, not the root's.
fn owned_roots(
    session: &AgentSession,
    host: &Arc<SubagentHost>,
) -> Arc<crate::plan::ledger::OwnedFn> {
    let (own, owner) = (session.store_handle(), Arc::clone(&host.options.store));
    Arc::new(move || {
        let stores: Vec<_> = [own(), owner()].into_iter().flatten().collect();
        crate::plan::ledger::Owned::of(&stores)
    })
}

type PlanWiring = (
    Arc<crate::plan::ops::PlanEngine>,
    crate::plan::ops::Actor,
    Arc<crate::todo::TodoStore>,
);

fn wire_plan_engine(
    session: &AgentSession,
    wiring: &RuntimeWiring,
    plans_dir: &Path,
    host: &Arc<SubagentHost>,
    tools: &mut Vec<Arc<dyn yi_tools::Tool>>,
    plan: Option<PlanWiring>,
) {
    let Some((engine, actor, todos)) = plan else {
        return;
    };
    if let Some(service) = session.plan_service() {
        service.set_engine(Arc::clone(&engine), actor.clone());
    }
    let store = session.store_handle();
    let mut tool = crate::plan::tool::PlanTool::new(Arc::clone(&engine), actor.clone());
    if let (crate::plan::ops::Actor::Owner, Some(broker)) = (&actor, wiring.broker.clone()) {
        tool = tool.confirming(crate::plan::authority::Confirming { broker, store });
    }
    tools.push(Arc::new(tool));
    let todo = crate::todo::tool::TodoTool::new(Arc::clone(&todos));
    tools.push(Arc::new(todo));
    session.set_todos(Arc::clone(&todos));
    if let Some(clock) = session.heartbeat_service() {
        let clock = Arc::downgrade(&clock);
        todos.on_change(Arc::new(move |list| {
            if let Some(clock) = clock.upgrade() {
                clock.watch(list);
            }
        }));
    }
    let inner = (wiring.depth == 0).then(|| {
        crate::plan::finish::install(host, &engine, {
            let hook = session.heartbeat_hook();
            Arc::new(move |message, mode| hook(message, mode))
        });
        let children = Arc::clone(host);
        let leased = Arc::clone(host);
        let mut timer = crate::plan::timer::PlanTimer::new(engine)
            .with_children(Arc::new(move || children.states()), {
                let (notice, stalled) = (lifecycle_notice(session), Arc::clone(host));
                Arc::new(move |text: &str, news| {
                    notice(text, news);
                    stalled.publish_all();
                })
            })
            .with_leases(Arc::new(move || {
                let host = Arc::clone(&leased);
                tokio::spawn(async move { host.expire().await });
            }))
            .with_owned({
                let store = session.store_handle();
                Arc::new(move || {
                    store()
                        .map(|session| crate::plan::ledger::owned_roots(&session))
                        .unwrap_or_default()
                })
            });
        if let Some(clock) = session.heartbeat_service() {
            let clock = Arc::downgrade(&clock);
            timer = timer.with_arm(Arc::new(move |waits| {
                if let Some(clock) = clock.upgrade() {
                    clock.arm(waits);
                }
            }));
        }
        let timer = Arc::new(timer);
        // A grace rides the loop's own due-time set, so an earlier one interrupts its sleep.
        let due = Arc::clone(&timer);
        host.set_lease_clock(
            None,
            Some(Arc::new(move |grace| {
                due.wake_at(due.now().checked_add(grace).unwrap_or_else(|| due.now()));
            })),
        );
        crate::plan::timer::spawn(timer);
        crate::plan::loop_coupling::coupling(
            session,
            crate::plan::loop_coupling::CouplingOptions {
                plans_dir: plans_dir.to_path_buf(),
            },
        )
    });
    let children = Arc::clone(host);
    crate::todo::coupling::install(
        session,
        todos,
        crate::todo::coupling::Options {
            eager: crate::todo::coupling::Eager::Prelude,
            children_running: Arc::new(move || children.busy()),
            inner,
        },
    );
}

fn wire_advisor(session: &AgentSession, wiring: &RuntimeWiring) {
    let hold_sink: Option<crate::advisor::HoldSink> = wiring.broker.as_ref().map(|broker| {
        let broker = Arc::clone(broker);
        Arc::new(move |advice: &yi_types::advisor::Advice| {
            let pattern = advice
                .target
                .as_deref()
                .and_then(yi_permission::HoldPattern::new);
            let Some(pattern) = pattern.filter(|_| broker.can_ask()) else {
                return false;
            };
            broker.insert_hold(yi_permission::Hold {
                pattern,
                reason: advice.text.clone(),
                source: yi_permission::HoldSource::Advisor,
                expires_at_ms: Some(yi_session::now_ms().saturating_add(3_600_000)),
            });
            true
        }) as crate::advisor::HoldSink
    });
    // §16: ADVISOR.md attention text, project-local, best-effort.
    let attention = std::fs::read_to_string(wiring.cwd.join("ADVISOR.md")).ok();
    let llm = wiring.advisor.clone().map(|model| {
        // Its own conversation: the root's ledger estimate and hour prefix are not its (D315).
        Arc::new(crate::advisor::review::LlmReviewer::new(
            Arc::new(wiring.provider.for_child()),
            model,
            attention.clone(),
        ))
    });
    let advisor = crate::advisor::attach_advisor(
        session,
        crate::advisor::AdvisorConfig {
            attention,
            reviewer: llm.is_some(),
            rules_dir: Some(wiring.cwd.join(".yi/rules")),
            ..crate::advisor::AdvisorConfig::default()
        },
        crate::advisor::AdvisorDeps { hold_sink, llm },
    );
    session.set_advisor(advisor);
}

/// A job this session started reports through its follow-up queue and wakes it when idle; the
/// loop ends with the session, so a retired child's job starts no paid turn in it.
pub(crate) fn wire_job_completions(session: &AgentSession) {
    let (report, owner) = (session.job_report_hook(), session.job_owner());
    tokio::spawn(crate::session::until(job_settled(), move || {
        let reports = yi_tools::jobs::registry().take_finished(owner).into_iter();
        let texts = reports.map(|report| {
            let (job, headline, body) = (report.id, report.headline(), &report.output);
            let text = format!("<async_result job=\"{job}\">{headline}\n{body}</async_result>");
            let unread: crate::session::StillNews =
                Arc::new(move || !yi_tools::jobs::registry().delivered(job));
            (text, unread)
        });
        if report(texts.collect()) {
            std::ops::ControlFlow::Continue(None)
        } else {
            std::ops::ControlFlow::Break(())
        }
    }));
}

pub(crate) fn journal_into<T: yi_types::entry::CustomRecord + 'static>(
    store: Arc<dyn Fn() -> Option<yi_session::SharedSession> + Send + Sync>,
) -> crate::permission::Journal<T> {
    Arc::new(move |record| {
        if let Some(store) = store() {
            let _journaled = yi_session::lock_session(&store).append_custom_record(&record);
        }
    })
}

/// One thread turns the job registry's settles into a wake the per-session loops can await.
fn job_settled() -> &'static tokio::sync::Notify {
    static SETTLED: std::sync::OnceLock<tokio::sync::Notify> = std::sync::OnceLock::new();
    static BRIDGE: std::sync::Once = std::sync::Once::new();
    BRIDGE.call_once(|| {
        let bridge = std::thread::Builder::new().name("yi-job-settles".to_owned());
        let _spawned = bridge.spawn(|| {
            let mut seen = 0;
            loop {
                seen = yi_tools::jobs::registry().wait_settle(seen);
                SETTLED
                    .get_or_init(tokio::sync::Notify::new)
                    .notify_waiters();
            }
        });
    });
    SETTLED.get_or_init(tokio::sync::Notify::new)
}

fn wire_kernel(
    session: &AgentSession,
    wiring: &RuntimeWiring,
    registry: crate::kernel::HostRegistry,
) -> Arc<crate::kernel::KernelService> {
    let restore_notice = session.notice_hook(HostSource::Restore);
    let waits = session.wait_hook();
    let options = wiring.kernel_options(
        Arc::new(registry),
        Arc::new(move |restore| restore_notice(&crate::kernel::restore_notice_text(restore))),
        session.store_id_hook(),
        Arc::new(move |step: Option<&str>| {
            waits(step.map(|step| yi_types::event::Wait::KernelBoot {
                step: step.to_owned(),
            }));
        }),
    );
    let service = crate::kernel::KernelService::new(options);
    Arc::new(service.with_own_paths(wiring.own_paths(session)))
}

pub fn attach_runtime(session: &mut AgentSession, mut wiring: RuntimeWiring) -> Arc<SubagentHost> {
    if let Some(total) = wiring.deadline {
        session.set_deadline(total);
    }
    let plans_dir = wiring
        .plans_dir
        .clone()
        .unwrap_or_else(|| wiring.cwd.join(crate::plan::PLANS_DIR));
    wiring.plans_dir = Some(plans_dir.clone());
    if session.compactor().is_none() {
        session.enable_compaction_with_summarizer(
            yi_context::Settings::default(),
            wiring.summarizer.clone(),
        );
    }
    crate::checkpoint::wire_turn_checkpoints(session, &wiring.home, &wiring.cwd);
    let mut registry = crate::kernel::HostRegistry::default();
    registry.register_mcp_stubs();
    registry.register_exec(wiring.cwd.clone(), job_profile(&wiring, session));
    crate::kernel_state::register_host_stores(&mut registry, &wiring);
    if let Some(compactor) = session.compactor() {
        // compact.run only schedules and returns — running inline would abort
        // the turn whose cell awaits the reply (design §9.2).
        registry.register("compact.run", move |payload| {
            let instructions = payload.str_of("instructions").map(str::to_owned);
            compactor.schedule_with_instructions(instructions);
            Box::pin(async {
                let mut reply = Map::new();
                reply.insert("scheduled".to_owned(), Value::Bool(true));
                Ok(reply)
            })
        });
    }
    if let Some(status) = session.compact_status_handle() {
        registry.register("compact.status", move |_payload| {
            let status = status();
            Box::pin(async move {
                let mut reply = Map::new();
                reply.insert("tokens".to_owned(), Value::from(status.tokens));
                reply.insert(
                    "context_window".to_owned(),
                    Value::from(status.context_window),
                );
                reply.insert("percent".to_owned(), Value::from(status.percent));
                reply.insert("scheduled".to_owned(), Value::Bool(status.scheduled));
                Ok(reply)
            })
        });
    }
    let host = subagent_host(session, &wiring, &plans_dir);
    host.register(&mut registry);
    crate::mailbox::register_receive(session, &host, &mut registry);
    session.set_environment(crate::environment::hook(
        session,
        &wiring,
        Arc::clone(&host),
    ));
    if let Some(link) = wiring.parent_link.clone() {
        register_child_messaging(link, &host, &mut registry);
    }
    wire_schedule(session, &wiring, &mut registry);
    wire_goal(session, &mut registry, &wiring, &plans_dir);
    let fetch_log = Arc::new(crate::fetch::FetchLog::new());
    fetch_log.attach_session_handle(session.store_handle());
    let kernels = Arc::clone(&wiring.kernels);
    let resolver = wire_fetch(
        session,
        &wiring,
        &plans_dir,
        &host,
        &mut registry,
        Arc::clone(&fetch_log),
        Arc::clone(&kernels),
    );
    wire_plan_compaction(session, &plans_dir);
    let plan = wire_plan_request(
        session,
        &wiring,
        &plans_dir,
        &host,
        &mut registry,
        Arc::clone(&fetch_log),
        Arc::clone(&resolver),
    );
    let service = wire_kernel(session, &wiring, registry);
    wire_advisor(session, &wiring);
    if wiring.kernel_prewarm {
        let warm = Arc::clone(&service);
        tokio::spawn(async move { warm.prewarm().await });
    }
    session.set_kernel_service(Arc::clone(&service));
    kernels.insert(
        wiring
            .parent_link
            .as_ref()
            .map_or_else(|| "main".to_owned(), |link| link.child_name.clone()),
        &service,
    );
    let mut tools = (wiring.tools)();
    crate::fetch::route_urls(&mut tools, &resolver);
    tools.push(crate::kernel::ipython_tool(Arc::clone(&service)));
    crate::auto_review::wire(session, &wiring, &mut tools);
    if let Some(broker) = &wiring.broker {
        broker.set_journal(journal_into(session.store_handle()));
    }
    let fetch_for_rules = Arc::clone(&fetch_log);
    wire_plan_engine(session, &wiring, &plans_dir, &host, &mut tools, plan);
    if let (Some(plan), Some(advisor)) = (session.plan_service(), session.advisor()) {
        plan.set_on_change(Arc::new(move |plan| {
            advisor.request_review(Some(crate::plan::summary_line(plan)));
        }));
    }
    let rule_set = crate::rules::discover_armed(&wiring.cwd, &wiring.home);
    if !rule_set.warnings.is_empty() {
        let notice = session.notice_hook(HostSource::Notice);
        for warning in &rule_set.warnings {
            notice(warning);
        }
    }
    // Attached even with zero rules: the adapters capture this Arc when tools
    // are installed, so a rule promoted mid-session (§16) arms immediately.
    let engine = Arc::new(crate::rules::RuleEngine::new(rule_set.rules));
    engine.set_fetch(fetch_for_rules);
    crate::rules::attach_rules(session, Arc::clone(&engine));
    session.set_rules_engine(Arc::clone(&engine));
    wire_compacted(session, &service, &plans_dir, engine);
    session.set_wall(wiring.wall.clone());
    session.use_tools_with_background(
        tools,
        wiring.cwd.clone(),
        wiring.broker.clone(),
        wiring.auto_background,
    );
    wire_job_completions(session);
    host
}

/// A child's lifecycle notice wakes its parent (§7.5) unless its news was read before a turn
/// would present it.
pub fn lifecycle_notice(session: &AgentSession) -> Arc<crate::subagent::NoticeFn> {
    let wake = session.wake_idle_hook();
    let host = |text: &str| crate::session::host_text(HostSource::Lifecycle, text);
    Arc::new(move |text: &str, news| wake(host(text), news))
}

fn subagent_host(
    session: &AgentSession,
    wiring: &RuntimeWiring,
    plans_dir: &Path,
) -> Arc<SubagentHost> {
    let factory = child_factory(wiring.clone());
    let host = Arc::new(SubagentHost::new(SubagentHostOptions {
        depth: wiring.depth,
        max_depth: wiring.max_depth,
        max_children: crate::levers::get().family_max_children,
        parent_session_dir: wiring.rlm_dir.clone(),
        defaults: session.settings_handle(),
        factory,
        provider: Arc::clone(&wiring.provider),
        notice: lifecycle_notice(session),
        events: session.events_sender(),
        parent_messages: session.history_handle(),
        cwd: wiring.cwd.clone(),
        home: wiring.home.clone(),
        lane_slots: wiring.lane_slots,
        report: session.deliver_hook(),
        attribute: session.attribution_handle(),
        store: session.store_handle(),
        plans_dir: plans_dir.to_path_buf(),
        family_live: Arc::clone(&wiring.kernels),
    }));
    host.set_grant(wiring.wall.clone(), None);
    host.family.get_or_init(|| wiring.family_dir());
    let counted = Arc::downgrade(&host);
    session.set_waits(Arc::new(move || {
        let host = counted.upgrade();
        host.map_or(0, |host| {
            host.waits.load(std::sync::atomic::Ordering::SeqCst)
        })
    }));
    host
}

/// §12: the ledger names what is load-bearing at every compaction and the summarizer
/// disposes. Read per compaction, never stored, so a directive cannot go stale.
fn wire_plan_compaction(session: &AgentSession, plans_dir: &Path) {
    let Some(compactor) = session.compactor() else {
        return;
    };
    let store = session.store_handle();
    let dir = plans_dir.to_path_buf();
    compactor.set_standing(Arc::new(move || {
        crate::plan::canonical_plan(&store, &dir)
            .ok()
            .and_then(|plan| crate::plan::compaction_directive(&plan))
    }));
}

/// The affordance notice, the advisor note, the kernel peek, and the plan's
/// windowed re-injection, all off the one post-compaction hook.
fn wire_compacted(
    session: &AgentSession,
    service: &Arc<crate::kernel::KernelService>,
    plans_dir: &Path,
    rules: Arc<crate::rules::RuleEngine>,
) {
    {
        let service = Arc::clone(service);
        let notice = session.notice_hook(HostSource::Notice);
        let store = session.store_handle();
        let advisor = session.advisor();
        let deliver = session.advisory_hook();
        let reinject_dir = plans_dir.to_path_buf();
        session.set_on_compacted(Arc::new(move || {
            rules.rearm();
            let service = Arc::clone(&service);
            let notice = Arc::clone(&notice);
            let handle = store();
            let file = handle
                .as_ref()
                .and_then(|store| yi_session::lock_session(store).file_path().cloned());
            let kept = match file.is_some() {
                true => yi_types::graph::SESSION_ON_DISK,
                false => yi_types::graph::SESSION_IN_MEMORY,
            };
            notice(&crate::affordance::next("compact.run", &[kept], ""));
            crate::advisor::note_last_compaction(advisor.as_deref(), handle.as_ref());
            tokio::spawn(async move {
                if let Some(text) = service.sync_after_compaction().await {
                    notice(&text);
                }
            });
            let deliver = Arc::clone(&deliver);
            let store = Arc::clone(&store);
            let dir = reinject_dir.clone();
            tokio::task::spawn_blocking(move || {
                if let Ok(plan) = crate::plan::canonical_plan(&store, &dir)
                    && plan.state == yi_types::plan::doc::PlanState::Active
                    && !plan.finished()
                {
                    deliver(crate::plan::loop_coupling::ledger_message(
                        crate::plan::loop_coupling::reinjection_text(&plan),
                        false,
                    ));
                }
            });
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cut_grep_page_names_the_call_for_the_next_page() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut store = yi_session::SessionStore::in_memory(yi_types::wire::SessionMetadata {
            id: "s".to_owned(),
            created_at: 0,
            parent_session_id: None,
            name: None,
        });
        for turn in 0..10 {
            store.append_compaction(
                "main",
                format!("the ö needle, turn {turn}"),
                Vec::new(),
                1,
                None,
            )?;
        }
        let (hits, total) = store.grep_page("Needle", 0, 8);
        let reply = grep_reply(&hits, total, "Needle", 0, 8);
        assert_eq!(reply["total"], 10);
        assert_eq!(
            reply["notice"],
            "[8 of 10 hits · limit 8 (at most 32) · compact.recall(\"Needle\", limit=8, offset=8) for the next]"
        );
        let (rest, total) = store.grep_page("Needle", 8, 8);
        let last = grep_reply(&rest, total, "Needle", 8, 8);
        assert_eq!(last["hits"].as_array().map(Vec::len), Some(2));
        assert!(last.get("notice").is_none(), "{last:?}");
        let (clamped, total) = store.grep_page("needle", 0, 500);
        let wide = grep_reply(&clamped, total, "needle", 0, 500);
        assert_eq!(wide["hits"].as_array().map(Vec::len), Some(10));
        assert!(wide.get("notice").is_none());
        Ok(())
    }

    /// #1001: with no Seatbelt (Linux) a walled kernel and its `bash()` jobs would run with no
    /// profile, so a cell would meet no wall at all; it is refused before it boots.
    #[tokio::test]
    async fn a_walled_kernel_with_no_profile_never_boots() {
        let dir = std::env::temp_dir();
        let options = crate::kernel::KernelServiceOptions {
            cwd: dir.clone(),
            home: dir.join("yi-no-home"),
            session_dir: None,
            family_dir: None,
            host: Arc::new(crate::kernel::HostRegistry::default()),
            on_restore: None,
            on_boot: None,
            sandbox: None,
            snapshot_key: None,
            per_session_state: false,
            cell_ceiling: None,
        };
        let walled: OwnPathsFn = Arc::new(|_| Vec::new());
        let service = crate::kernel::KernelService::new(options).with_own_paths(Some(walled));
        service.prewarm().await;
        let state = service.state();
        assert!(state.starts_with("unavailable: ipython"), "{state}");
    }
}
