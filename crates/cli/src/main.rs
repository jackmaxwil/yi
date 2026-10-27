#![forbid(unsafe_code)]
#![deny(clippy::string_slice)]

mod args;
mod ask;
mod catalog;
mod commands;
mod debug;
mod doctor;
mod fetch;
mod lanes;
mod memory;
mod plan;
mod plugin;
mod roles;
mod rpc;
mod sessions;
mod setup;
mod sidecar;
mod stats;
mod todo;
mod trace_sink;
mod tty;
mod update;
mod user_config;
mod why;

use std::sync::Arc;
use yi_types::config::{DEFAULT_MAX_DEPTH, RlmConfig};

use args::{Args, Resume, parse_args};
use lanes::{claim_lane, configured_lanes, release_lane};
use user_config::{
    config, configured_auto_background, configured_roles, home, load_config, refreshed,
    reload_config,
};

use yi_runtime::{AgentSession, ProviderStream, SessionConfig, resolve_model};
use yi_types::event::{AgentEvent, AssistantMessageEvent};
use yi_types::model::{Model, ModelCost};

fn faux_model() -> Model {
    let zero = || serde_json::Number::from(0u64);
    Model {
        id: "faux-1".to_owned(),
        name: "Faux".to_owned(),
        api: "faux".to_owned(),
        provider: "faux".to_owned(),
        base_url: "http://localhost:0".to_owned(),
        reasoning: false,
        input: vec!["text".to_owned()],
        cost: ModelCost {
            input: zero(),
            output: zero(),
            cache_read: zero(),
            cache_write: zero(),
            tiers: None,
        },
        context_window: 128_000,
        max_tokens: 16_384,
        compat: None,
        thinking_level_map: None,
        headers: None,
    }
}

fn resolve(spec: &str) -> Option<Model> {
    let (provider, id) = spec.split_once('/')?;
    if provider == "faux" {
        return Some(faux_model());
    }
    resolve_model(provider, id)
}

fn render_text(event: &AgentEvent) -> Option<String> {
    match event {
        AgentEvent::MessageUpdate {
            assistant_message_event: AssistantMessageEvent::TextDelta { delta, .. },
            ..
        } => Some(delta.clone()),
        _ => None,
    }
}

fn effective_cwd(args: &Args) -> std::path::PathBuf {
    args.cwd.as_ref().map_or_else(
        || std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        std::path::PathBuf::from,
    )
}

fn env_either(upper: &str, lower: &str) -> Option<String> {
    std::env::var(upper).or_else(|_| std::env::var(lower)).ok()
}

fn proxy_from_env() -> Result<Option<yi_runtime::ProxyConfig>, String> {
    yi_runtime::ProxyConfig::from_values(
        env_either("HTTPS_PROXY", "https_proxy").as_deref(),
        env_either("HTTP_PROXY", "http_proxy").as_deref(),
        env_either("NO_PROXY", "no_proxy").as_deref(),
    )
}

/// Incident: the ACP builder mapped a refusal to "exit code 2" and the TUI showed the user
/// exactly that; the reason travels with the code so every surface can say it.
struct Refused {
    code: i32,
    reason: String,
    class: yi_types::telemetry::ErrorClass,
}

fn exit_refused(refused: Refused) -> i32 {
    eprintln!("error: {} [{}]", refused.reason, refused.class);
    refused.code
}

fn build_session(
    args: &Args,
    asker: Option<yi_runtime::Asker>,
    session_id: Option<&str>,
    prompts_close: bool,
    unattended: bool,
) -> Result<(AgentSession, std::sync::Arc<yi_runtime::SubagentHost>), Refused> {
    // Invariant: the first statement here, so no lever is read on both sides of the
    // override and a refused file stops the run before the session, and any model call.
    let _build = yi_types::trace::span("build_session");
    let levers = yi_types::trace::span("build_session.levers");
    yi_runtime::levers::init(args.eval).map_err(|reason| Refused {
        code: 1,
        reason,
        class: yi_types::telemetry::ErrorClass::RefusalConfig,
    })?;
    drop(levers);
    reload_config();
    let args = &refreshed(args);
    drop(yi_runtime::retention::sweep_home_hourly(home()));
    yi_runtime::node::configure(config().node.clone().unwrap_or_default());
    if args.model.is_empty() {
        return Err(Refused {
            code: 2,
            reason: "no model configured (pass --model provider/id or set \"model\" in ~/.yi/config.json)".to_owned(),
            class: yi_types::telemetry::ErrorClass::RefusalConfig,
        });
    }
    let Some(model) = resolve(&args.model) else {
        return Err(Refused {
            code: 2,
            reason: format!("unknown model {} (use provider/id)", args.model),
            class: yi_types::telemetry::ErrorClass::RefusalUnknownModel,
        });
    };
    // `--faux` scripts whatever model is named, which keeps its id and catalog facts on
    // screen; the host starts no request of its own below, while a scripted tool call still
    // runs (#943).
    let faux = args.faux.is_some() || model.provider == "faux";
    let interactive = {
        use std::io::IsTerminal;
        std::io::stdin().is_terminal()
    };
    // Incident: an inherited proxy value ureq cannot dial refused every faux run too, so an
    // operator's shell broke `just check`. A faux run keeps a proxy it can dial, so a request
    // that leaks rides it where a test's listener sees it (#943).
    let proxy = match proxy_from_env() {
        Ok(proxy) => proxy,
        Err(_) if faux => None,
        Err(reason) => {
            return Err(Refused {
                code: 2,
                reason,
                class: yi_types::telemetry::ErrorClass::RefusalConfig,
            });
        }
    };
    let telemetry = config()
        .telemetry
        .as_ref()
        .and_then(|telemetry| telemetry.enabled)
        .unwrap_or(false)
        .then(|| Arc::new(yi_runtime::Telemetry::default()));
    let provider = Arc::new(
        yi_runtime::ProviderStream::new(session_id.map(str::to_owned))
            .with_long_cache(interactive)
            .with_proxy(proxy.clone())
            .with_routing(config().routing.clone())
            .with_telemetry(telemetry.clone())
            .with_forced_faux(faux),
    );
    if !faux {
        // Resolved through the session's proxy so an OAuth refresh can reach the token
        // endpoint from behind the same wall the stream rides through.
        let auth = yi_types::trace::span("build_session.auth");
        let credential = provider
            .credential(&model.provider)
            .map_err(|_| login::no_credential(&model.provider))?;
        drop(auth);
        let _span = yi_types::trace::span("build_session.catalog_refresh");
        catalog::spawn_refresh(&model.provider, Some(&credential.secret), proxy.as_ref());
    }
    if faux {
        provider.queue_faux_timed(shells::faux_replies(args).map_err(|reason| Refused {
            code: 2,
            reason,
            class: yi_types::telemetry::ErrorClass::RefusalConfig,
        })?);
    }
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: String::new(),
            model,
            thinking_level: args.thinking,
            tool_execution: yi_loop_default(),
        },
        provider,
    );
    if let Some(telemetry) = telemetry {
        session.set_telemetry(telemetry);
    }
    let cwd = effective_cwd(args);
    let home = home().to_path_buf();
    if faux {
        yi_runtime::lane::forbid_fetch();
    }
    let claiming = yi_types::trace::span("build_session.claim_lane");
    let claimed = claim_lane(args, &home, session_id);
    drop(claiming);
    let (lane, pool) = match claimed {
        Ok(claimed) => claimed,
        Err(message) => {
            return Err(Refused {
                code: 1,
                reason: format!("lane: {message}"),
                class: yi_types::telemetry::ErrorClass::RefusalLane,
            });
        }
    };
    let work = lane
        .as_ref()
        .map_or_else(|| cwd.clone(), |lane| lane.path().to_path_buf());
    let broker = std::sync::Arc::new(
        yi_runtime::PermissionBroker::new(
            args.mode,
            work.clone(),
            Vec::new(),
            asker,
            session.events_sender(),
        )
        .with_sandbox(yi_runtime::workspace_sandbox(&work, &home, None))
        .with_session_store(&default_session_dir(args))
        .with_unattended(unattended),
    );
    if prompts_close {
        broker.prompts_close_on_settle();
    }
    yi_runtime::post::configure(config().mail.as_ref().and_then(|mail| mail.hop_limit));
    let tools_home = home.clone();
    let extensions = yi_types::trace::span("build_session.install_extensions");
    let mut host = shells::session_extensions(args, &work, session.model().context_window);
    if unattended {
        let slot = yi_runtime::ext::Slot::new(yi_runtime::ext::Rank::Mode, "unattended");
        host.attach(slot, yi_runtime::unattended_fragment().to_owned());
    }
    session.install_extensions(host);
    drop(extensions);
    let provider = std::sync::Arc::clone(session_provider(&session));
    let freeform_grammar = config()
        .edit
        .as_ref()
        .and_then(|edit| edit.freeform_grammar)
        .unwrap_or(false);
    let selector = roles::selector_model()
        .map(|model| yi_runtime::select::Selector::new(std::sync::Arc::clone(&provider), model));
    session.set_selector(selector);
    let wiring = yi_types::trace::span("build_session.attach_runtime");
    let host = yi_runtime::attach_runtime(
        &mut session,
        yi_runtime::RuntimeWiring {
            provider,
            system_prompt: String::new(),
            tool_execution: yi_loop_default(),
            cwd: work.clone(),
            home: home.clone(),
            lane_slots: lanes::lane_slots(),
            mcp_read: Some(std::sync::Arc::new(McpOneShot)),
            broker: Some(broker),
            tools: std::sync::Arc::new(move || {
                yi_runtime::session_tools(
                    freeform_grammar,
                    Some(yi_runtime::documents(&tools_home)),
                    Some(tools_home.join(".yi/tools")),
                )
            }),
            depth: 0,
            max_depth: config()
                .rlm
                .as_ref()
                .map_or(DEFAULT_MAX_DEPTH, RlmConfig::depth),
            rlm_dir: default_session_dir(args).join(format!("rlm-{}", std::process::id())),
            family_dir: session_id.map(|id| sessions::board_dir(&default_session_dir(args), id)),
            sessions_dir: Some(default_session_dir(args)),
            summarizer: roles::summarizer_model(args),
            advisor: roles::advisor_model(),
            auto_review: roles::auto_review_model(),
            parent_link: None,
            wall: yi_runtime::Wall::default(),
            plan_stale_turns: config()
                .plan
                .as_ref()
                .and_then(|plan| plan.stale_reminder_turns),
            plans_dir: configured_plans_dir(&work),
            auto_background: configured_auto_background(),
            deadline: args.deadline.map(std::time::Duration::from_secs),
            // A cold venv installs from PyPI; a scripted run boots it at its first cell.
            kernel_prewarm: !faux
                && config()
                    .kernel
                    .as_ref()
                    .and_then(|kernel| kernel.prewarm)
                    .unwrap_or(true),
            kernels: yi_runtime::fetch::KernelServiceMap::new(),
        },
    );
    drop(wiring);
    let at = config().compaction.as_ref().and_then(|at| at.at);
    if let Some(why) = session.compactor().and_then(|c| c.set_ceiling(at)) {
        eprintln!("warning: {why}");
    }
    if !faux {
        for why in yi_runtime::classifier::attach(&session, &work, &home, &config()) {
            eprintln!("warning: {why}");
        }
    }
    let sessions = default_session_dir(args);
    for why in yi_runtime::spend::attach_configured(&session, config().spend.as_ref(), &sessions) {
        eprintln!("warning: {why}");
    }
    yi_runtime::cache_miss::attach(&session);
    session.set_lane(yi_runtime::lane::land::LaneHandle::new(
        lane,
        pool,
        configured_lanes().land,
        session.events_sender(),
        session.heartbeat_hook(),
    ));
    Ok((session, host))
}

fn plans_dir(workspace: &std::path::Path) -> std::path::PathBuf {
    yi_runtime::plan::resolve_plans_dir(configured_plans_dir(workspace), workspace)
}

/// `plans.dir`, relative to the workspace root unless absolute. Unset leaves
/// the resolver's own `.yi/plans` default in place.
fn configured_plans_dir(workspace: &std::path::Path) -> Option<std::path::PathBuf> {
    let config = config();
    let path = std::path::PathBuf::from(config.plans.as_ref()?.dir.as_deref()?);
    Some(if path.is_absolute() {
        path
    } else {
        workspace.join(path)
    })
}

/// Invariant: an MCP read runs as a one-shot child of `yi mcp`, so no socket or token lives
/// in this process; the server segment of `mcp://<server>/<uri>` is a connected session name.
struct McpOneShot;

impl yi_runtime::fetch::McpResourceRead for McpOneShot {
    fn read(&self, server: &str, resource: &str) -> Result<String, String> {
        run_yi_mcp(&[&format!("@{server}"), "resources-read", resource])
    }

    /// The kernel's connect: the entry is read from `config` by the `<file>:<entry>` form, so
    /// no workspace file, and no other name, is consulted (D296).
    fn connect_server(
        &self,
        config: &std::path::Path,
        entry: &str,
        session: &str,
    ) -> Result<String, String> {
        let reference = format!("{}:{entry}", config.display());
        run_yi_mcp(&["connect", &reference, &format!("@{session}")])
    }
}

/// `yi mcp --json <args>` as a child of this binary; its `error:` line is the failure text.
fn run_yi_mcp(args: &[&str]) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    #[expect(
        clippy::disallowed_methods,
        reason = "every MCP call goes out through the one-shot CLI, never a held socket"
    )]
    let output = std::process::Command::new(exe)
        .args(["mcp", "--json"])
        .args(args)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        let raw = String::from_utf8_lossy(&output.stderr);
        let trimmed = raw.trim();
        let message = trimmed.strip_prefix("error: ").unwrap_or(trimmed);
        return Err(if message.is_empty() {
            format!("yi mcp exited {:?}", output.status.code())
        } else {
            message.to_owned()
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_owned())
}

/// A preview unless `--fix`, as `yi doctor` is: deleting is the act the user asks for.
fn run_gc(args: &Args) -> i32 {
    if !args.fix {
        println!("preview: `yi gc --fix` removes and trims what these lines count");
    }
    for report in yi_runtime::retention::sweep(home(), !args.fix) {
        println!("{report}");
    }
    0
}

fn run_why(args: &Args) -> i32 {
    let cwd = effective_cwd(args);
    let options = why::Options {
        plans_dir: plans_dir(&cwd),
        cwd,
        json: args.json,
    };
    why::run(&args.prompt, &options)
}

fn run_plan(args: &Args) -> i32 {
    let cwd = effective_cwd(args);
    let options = plan::Options {
        plans_dir: plans_dir(&cwd),
        session_dir: default_session_dir(args),
        cwd,
        json: args.json,
    };
    plan::run(&args.prompt, &options)
}

fn run_trust(args: &Args) -> i32 {
    let cwd = effective_cwd(args);
    let root = yi_runtime::ext::git_root(&cwd).unwrap_or(cwd.clone());
    let gate = yi_runtime::TrustGate::new(home());
    match args.prompt.trim() {
        "list" => {
            for (root, count) in gate.grants() {
                println!("{root}  ({count} sources)");
            }
            0
        }
        "revoke" => match gate.revoke(&root) {
            Ok(()) => {
                println!("revoked trust for {}", root.display());
                0
            }
            Err(error) => {
                eprintln!("error: {error}");
                1
            }
        },
        "" => {
            let sources = yi_runtime::ext::contributions(&root, home());
            if sources.is_empty() {
                println!("{} contributes no instructions or packs", root.display());
                return 0;
            }
            for (source, _) in &sources {
                println!("grant {source}");
            }
            match gate.grant(&root, &sources) {
                Ok(()) => {
                    println!(
                        "{} is trusted; its text reads as configuration until it changes",
                        root.display()
                    );
                    0
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    1
                }
            }
        }
        other => {
            eprintln!("usage: yi trust [list|revoke] (no argument grants the current repository)");
            eprintln!("error: unknown argument {other}");
            2
        }
    }
}

/// Would this run, and why. The same `decide()` the tool seam calls, so the
/// answer is the decision itself.
fn run_gate(args: &Args) -> i32 {
    let command = args.prompt.trim();
    if command.is_empty() {
        eprintln!("usage: yi gate [--auto|--confirm|--yolo] [--json] <command>");
        return 2;
    }
    let report = yi_runtime::gate::explain(command, args.mode, &effective_cwd(args));
    if args.json {
        print_json(&report.to_json());
    } else {
        print!("{}", yi_runtime::gate::render(&report));
    }
    i32::from(!report.allowed())
}

fn session_provider(session: &AgentSession) -> &std::sync::Arc<ProviderStream> {
    session.provider_arc()
}

fn default_session_dir(args: &Args) -> std::path::PathBuf {
    args.session_dir
        .as_ref()
        .map_or_else(|| home().join(".yi/sessions"), std::path::PathBuf::from)
}

fn session_repo(args: &Args) -> yi_runtime::session_store::JsonlRepo {
    yi_runtime::session_store::JsonlRepo::new(
        default_session_dir(args),
        effective_cwd(args).display().to_string(),
    )
}

fn current_thread() -> Result<tokio::runtime::Runtime, i32> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            eprintln!("error: {error}");
            1
        })
}

fn schema(args: &Args) -> Option<Result<yi_runtime::schema::Schema, String>> {
    args.schema.as_deref().map(yi_runtime::schema::Schema::load)
}

fn print_json(value: &serde_json::Value) {
    if let Ok(line) = serde_json::to_string(value) {
        println!("{line}");
    }
}

/// A flag names what the user wants now, a resumed session what they wanted last time.
/// Flags apply after `attach_store` or `--continue --model X` silently keeps the old model.
fn repin(args: &Args, session: &AgentSession) {
    if args.model_pinned
        && let Some(model) = resolve(&args.model)
    {
        session.set_model(model);
    }
    if let Some(effort) = args.thinking {
        session.set_effort(effort);
    }
}

/// The session a run records into, settled before the session is built so its wiring can key
/// the family board by the id (D242); the file itself is created only by [`attach_store`].
pub(crate) struct SessionTarget {
    pub(crate) id: String,
    exists: bool,
}

pub(crate) fn session_target(args: &Args) -> SessionTarget {
    let mut repo = session_repo(args);
    let existing = match &args.resume {
        Resume::Fresh => None,
        Resume::Leaf => sessions::latest_id(&mut repo),
        Resume::Named(id) => Some(id.clone()),
    };
    match existing {
        Some(id) => SessionTarget { id, exists: true },
        None => SessionTarget {
            id: yi_runtime::session_store::IdGenerator::new().next_id(),
            exists: false,
        },
    }
}

/// X1: every `yi ask` turn is recorded, so `--continue` has a leaf to resume.
fn attach_store(
    args: &Args,
    session: &AgentSession,
    target: &SessionTarget,
) -> Result<String, String> {
    use yi_runtime::session_store::{CreateOptions, SessionRepo, lock_session};
    let mut repo = session_repo(args);
    let store = if target.exists {
        repo.open(&target.id)
    } else {
        repo.create(CreateOptions {
            id: Some(target.id.clone()),
            ..CreateOptions::default()
        })
    }
    .map_err(|error| error.to_string())?;
    let id = lock_session(&store).metadata().id.clone();
    let attaching = yi_types::trace::span("attach_store.session");
    session
        .attach_store(store)
        .map_err(|error| error.to_string())?;
    drop(attaching);
    if let Some(lane) = session.lane() {
        let _span = yi_types::trace::span("attach_store.bind_lane");
        if let Err(error) = lane.bind_session(&id) {
            eprintln!("warning: lane: {error}");
        }
        lane.reattach();
    }
    let _span = yi_types::trace::span("attach_store.repin");
    repin(args, session);
    Ok(id)
}

/// Checkpoints (§7.7) restore the files the last turn changed, from the cwd's leaf session.
fn run_undo(args: &Args) -> i32 {
    use yi_runtime::session_store::SessionRepo;
    let cwd = effective_cwd(args);
    let mut repo = session_repo(args);
    let id = match &args.resume {
        Resume::Named(id) => Some(id.clone()),
        _ => sessions::latest_id(&mut repo),
    };
    let Some(id) = id else {
        eprintln!("error: no session for this directory");
        return 1;
    };
    let store = match repo.open(&id) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };
    if args.prompt.trim() == "list" {
        return list_checkpoints(&store, args.json);
    }
    match yi_runtime::undo(&store, &cwd, home()) {
        yi_runtime::UndoOutcome::Restored { changes, scoped } => {
            report_undo(&changes, scoped, args.json);
            0
        }
        yi_runtime::UndoOutcome::NoCheckpoint => {
            eprintln!("error: session {id} has no checkpoint to restore");
            1
        }
        yi_runtime::UndoOutcome::Failed(error) => {
            eprintln!("error: {error}");
            1
        }
    }
}

fn list_checkpoints(store: &yi_runtime::session_store::SharedSession, json: bool) -> i32 {
    use yi_types::checkpoint::CheckpointAt;
    let recorded = yi_runtime::recorded(store);
    if json {
        let rows: Vec<serde_json::Value> = recorded
            .iter()
            .map(|entry| {
                serde_json::json!({
                    "tree": entry.data.tree,
                    "at": entry.data.at,
                    "timestamp": entry.timestamp,
                })
            })
            .collect();
        print_json(&serde_json::json!({ "checkpoints": rows }));
        return 0;
    }
    if recorded.is_empty() {
        println!("no checkpoints recorded");
        return 0;
    }
    let now = yi_runtime::session_store::now_ms();
    for entry in &recorded {
        let at = match &entry.data.at {
            CheckpointAt::TurnStart => "turn start",
            CheckpointAt::TurnEnd => "turn end",
            CheckpointAt::Undo => "undo",
            CheckpointAt::Other(name) => name,
        };
        let age = yi_runtime::session_store::age_label(now.saturating_sub(entry.timestamp));
        let tree = entry.data.tree.get(..8).unwrap_or(&entry.data.tree);
        println!("{tree}  {at:<10}  {age:>8}");
    }
    0
}

fn report_undo(changes: &[yi_runtime::Change], scoped: bool, json: bool) {
    let described: Vec<serde_json::Value> = changes
        .iter()
        .map(|change| {
            serde_json::json!({
                "path": change.path.display().to_string(),
                "action": match change.kind {
                    yi_runtime::ChangeKind::Restored => "restored",
                    yi_runtime::ChangeKind::Deleted => "deleted",
                    yi_runtime::ChangeKind::Kept => "kept",
                },
            })
        })
        .collect();
    if json {
        print_json(&serde_json::json!({ "reverted": described, "scoped": scoped }));
        return;
    }
    let notes = yi_runtime::undo_notes(changes, scoped);
    if changes.is_empty() && notes.is_empty() {
        println!("nothing to revert");
        return;
    }
    for change in changes {
        let action = match change.kind {
            yi_runtime::ChangeKind::Restored => "restored",
            yi_runtime::ChangeKind::Deleted => "deleted ",
            yi_runtime::ChangeKind::Kept => continue,
        };
        println!("{action}  {}", change.path.display());
    }
    for note in notes {
        println!("{note}");
    }
}

fn yi_ai_key(provider: &str) -> Option<yi_runtime::auth::Secret> {
    yi_runtime::auth::api_key(provider)
}
fn yi_loop_default() -> yi_runtime::ExecutionMode {
    yi_runtime::ExecutionMode::Sequential
}
fn mcp_enabled() -> bool {
    config().mcp.as_ref().and_then(|mcp| mcp.enabled) == Some(true)
}

mod login;
mod mcp_hooks;
mod shells;
use shells::{run_console_command, run_serve_command, run_tui_command};

/// `yi mcp` answers before argument parsing: it takes the raw argv the one-shot CLI owns.
fn mcp_fast_path() {
    if std::env::args().nth(1).as_deref() == Some("mcp") {
        if !mcp_enabled() {
            eprintln!(
                "error: mcp is disabled; set {{\"mcp\": {{\"enabled\": true}}}} in ~/.yi/config.json"
            );
            std::process::exit(2);
        }
        let raw: Vec<String> = std::env::args().skip(2).collect();
        let config = config();
        let token_store = config
            .mcp
            .as_ref()
            .and_then(|mcp| mcp.token_store.as_deref());
        let hooks = mcp_hooks::McpHooks::load();
        let hooks = hooks
            .as_ref()
            .map(|hooks| hooks as &dyn yi_mcp_cli::CallHooks);
        let version = env!("ARCHITECTURE_VERSION");
        std::process::exit(yi_mcp_cli::run(&raw, token_store, hooks, version));
    }
}

fn main() {
    if std::env::args_os().nth(1).is_some_and(|arg| arg == "__fds") {
        let args: Vec<std::ffi::OsString> = std::env::args_os().skip(2).collect();
        eprintln!("error: __fds: {}", yi_browser::exec_on_pipe_fds(&args));
        std::process::exit(1);
    }
    #[cfg(feature = "tui")]
    if std::env::args_os().nth(1).is_some_and(|arg| arg == "__pty") {
        std::process::exit(yi_console::pty::shim(std::env::args_os().skip(2)));
    }
    doctor::early();
    update::early();
    plugin::early();
    if let Err(error) = load_config() {
        eprintln!("error: {error}");
        std::process::exit(2);
    }
    login::fast_path();
    mcp_fast_path();
    let args = match parse_args() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(2);
        }
    };
    std::process::exit(dispatch(args));
}

fn dispatch(args: Args) -> i32 {
    use std::io::IsTerminal;
    if args.help || (args.command == "help" && args.prompt.is_empty()) {
        print!("{}", commands::help());
        return 0;
    }
    let terminal = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let word = match args.command.as_str() {
        "" if !terminal => {
            println!(
                "yi {}: not a terminal; `yi ask <prompt>` answers once; `yi serve` / `yi rpc` / `yi acp` serve a program",
                env!("ARCHITECTURE_VERSION")
            );
            return 0;
        }
        "" if args.solo => "tui",
        "" => "console",
        word => word,
    };
    if let Some(verb) = commands::find(word) {
        return commands::run(verb, &args);
    }
    if args.prompt.is_empty()
        && let Some(verb) = commands::near(word)
    {
        eprintln!("error: did you mean \"{verb}\"? to ask the model: yi ask {word}");
        return 2;
    }
    // §17.1: `yi <prompt words>` opens the TUI on a TTY, plain ask otherwise.
    let mut full = args.command.clone();
    if !args.prompt.is_empty() {
        full.push(' ');
        full.push_str(&args.prompt);
    }
    if terminal {
        return run_tui_command(&args, Some(full));
    }
    let mut ask_args = args.clone();
    ask_args.prompt = full;
    ask::run(&ask_args)
}

fn run_version(_: &Args) -> i32 {
    println!("yi {}", env!("ARCHITECTURE_VERSION"));
    0
}

fn run_ask(args: &Args) -> i32 {
    if args.prompt.is_empty() {
        eprintln!("usage: yi ask [--model provider/id] [--json] [--deadline secs] <prompt | ->");
        return 2;
    }
    ask::run(args)
}

fn run_rpc(args: &Args) -> i32 {
    let runtime = match current_thread() {
        Ok(runtime) => runtime,
        Err(code) => return code,
    };
    let session = {
        let _guard = runtime.enter();
        match build_session(args, None, None, false, false) {
            Ok((session, _host)) => session,
            Err(refused) => return exit_refused(refused),
        }
    };
    let options = rpc::RpcOptions {
        session_dir: default_session_dir(args),
        cwd: effective_cwd(args),
    };
    let lane = session.lane();
    let code = rpc::run_rpc(session, &options, runtime);
    release_lane(lane.as_deref());
    code
}

fn run_acp(args: &Args) -> i32 {
    let runtime = match current_thread() {
        Ok(runtime) => runtime,
        Err(code) => return code,
    };
    let build_args = args.clone();
    let defaults_args = args.clone();
    let build: yi_acp::SessionBuilder = {
        let runtime_handle = runtime.handle().clone();
        std::sync::Arc::new(move |asker, session| {
            let _guard = runtime_handle.enter();
            build_session(&build_args, asker, session, false, false)
                .map_err(|refused| format!("{} [{}]", refused.reason, refused.class))
        })
    };
    let options = yi_acp::AcpOptions {
        session_dir: default_session_dir(args),
        cwd: effective_cwd(args),
        build,
        agent_version: env!("ARCHITECTURE_VERSION").to_owned(),
        defaults: std::sync::Arc::new(move || {
            reload_config();
            let args = refreshed(&defaults_args);
            resolve(&args.model).map(|model| yi_acp::SessionDefaults {
                model,
                effort: args.thinking.unwrap_or_default(),
                mode: args.mode,
            })
        }),
    };
    yi_acp::run_acp(options, runtime)
}

fn run_sessions(args: &Args) -> i32 {
    let options = sessions::Options {
        session_dir: default_session_dir(args),
        cwd: effective_cwd(args).display().to_string(),
        json: args.json,
    };
    sessions::run(&args.prompt, &options)
}

fn run_stats(args: &Args) -> i32 {
    let cwd = effective_cwd(args);
    let options = stats::Options {
        session_dir: default_session_dir(args),
        plans_dir: plans_dir(&cwd),
        cwd: cwd.display().to_string(),
        json: args.json,
        query: args.stats.clone(),
    };
    stats::run(&args.prompt, &options)
}

fn run_tui(args: &Args) -> i32 {
    run_tui_command(args, (!args.prompt.is_empty()).then(|| args.prompt.clone()))
}
