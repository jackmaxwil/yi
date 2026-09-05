#![forbid(unsafe_code)]
#![deny(clippy::string_slice)]

mod catalog;
mod lanes;
mod plan;
mod rpc;
mod sessions;
mod stats;
mod why;

use std::sync::Arc;

use lanes::{claim_lane, configured_lanes, release_lane, run_lanes};

use yi_runtime::{AgentSession, ProviderStream, SessionConfig, resolve_model};
use yi_types::event::AgentEvent;
use yi_types::event::AssistantMessageEvent;
use yi_types::message::{AgentMessage, StopReason};
use yi_types::model::{Effort, Model, ModelCost, UnknownEffort};

#[derive(Clone)]
struct Args {
    command: String,
    model: String,
    system: String,
    thinking: Option<Effort>,
    /// `--model` was named on the command line, so it outranks a resumed one.
    model_pinned: bool,
    json: bool,
    mode: yi_runtime::PermissionMode,
    session_dir: Option<String>,
    cwd: Option<String>,
    here: bool,
    socket: Option<String>,
    headless: bool,
    solo: bool,
    keys: Option<String>,
    frames: Option<String>,
    record: Option<String>,
    snap: Option<String>,
    deadline: Option<u64>,
    resume: Resume,
    schema: Option<String>,
    prompt: String,
}

/// Which session file `yi ask` writes to (X1 `--continue` / `--session`).
#[derive(Clone, PartialEq, Eq)]
enum Resume {
    Fresh,
    Leaf,
    Named(String),
}

fn parse_args() -> Result<Args, lexopt::Error> {
    use lexopt::prelude::*;
    let mut command = String::new();
    let mut model = None;
    let mut system = String::new();
    let mut thinking = None;
    let mut json = false;
    // Auto is the default: reads and known-safe commands run, destructive ones ask.
    // `--yolo` removes the gate, `--confirm` asks for everything.
    let mut mode = yi_runtime::PermissionMode::Auto;
    let mut session_dir = None;
    let mut cwd = None;
    let mut here = false;
    let mut socket = None;
    let mut headless = false;
    let mut solo = false;
    let mut keys = None;
    let mut frames = None;
    let mut record = None;
    let mut snap = None;
    let mut deadline = None;
    let mut continue_leaf = false;
    let mut session = None;
    let mut schema = None;
    let mut prompt_parts: Vec<String> = Vec::new();
    let mut parser = lexopt::Parser::from_env();
    while let Some(argument) = parser.next()? {
        match argument {
            Long("version") => {
                command = "version".to_owned();
            }
            Long("model") => model = Some(parser.value()?.string()?),
            Long("system") => system = parser.value()?.string()?,
            Long("thinking") => {
                let raw = parser.value()?.string()?;
                thinking = Some(
                    raw.parse()
                        .map_err(|error: UnknownEffort| lexopt::Error::Custom(Box::new(error)))?,
                );
            }
            Long("json") => json = true,
            Long("yolo") => mode = yi_runtime::PermissionMode::Yolo,
            Long("auto") => mode = yi_runtime::PermissionMode::Auto,
            Long("confirm") => mode = yi_runtime::PermissionMode::Ask,
            Long("session-dir") => session_dir = Some(parser.value()?.string()?),
            Long("cwd") => cwd = Some(parser.value()?.string()?),
            Long("here") => here = true,
            Long("socket") => socket = Some(parser.value()?.string()?),
            Long("headless") => headless = true,
            Long("solo") => solo = true,
            Long("keys") => keys = Some(parser.value()?.string()?),
            Long("frames") => frames = Some(parser.value()?.string()?),
            Long("record") => record = Some(parser.value()?.string()?),
            Long("snap") => snap = Some(parser.value()?.string()?),
            Long("deadline") => deadline = Some(parser.value()?.parse()?),
            Long("continue") => continue_leaf = true,
            Long("session") => session = Some(parser.value()?.string()?),
            Long("schema") => schema = Some(parser.value()?.string()?),
            Value(value) => {
                let value = value.string()?;
                if command.is_empty() {
                    command = value;
                } else {
                    prompt_parts.push(value);
                }
            }
            _ => return Err(argument.unexpected()),
        }
    }
    // Drive-only flags are silently inert outside the headless loop, which
    // reads downstream as a capture that produced nothing.
    if !headless
        && let Some(flag) = [
            ("--keys", keys.is_some()),
            ("--frames", frames.is_some()),
            ("--record", record.is_some()),
            ("--snap", snap.is_some()),
            ("--deadline", deadline.is_some()),
        ]
        .into_iter()
        .find_map(|(name, present)| present.then_some(name))
    {
        return Err(lexopt::Error::Custom(
            format!("{flag} needs --headless").into(),
        ));
    }
    Ok(Args {
        command,
        model_pinned: model.is_some(),
        model: model.or_else(configured_model).unwrap_or_default(),
        system,
        thinking: thinking.or_else(configured_thinking),
        json,
        mode,
        session_dir,
        cwd,
        here,
        socket,
        headless,
        solo,
        keys,
        frames,
        record,
        snap,
        deadline,
        resume: match (session, continue_leaf) {
            (Some(id), _) => Resume::Named(id),
            (None, true) => Resume::Leaf,
            (None, false) => Resume::Fresh,
        },
        schema,
        prompt: prompt_parts.join(" "),
    })
}

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

/// Yi has no built-in default model; the user's config carries it, either as
/// `"model"` or as the §12 `"models"` role table.
fn configured_model() -> Option<String> {
    let roles = configured_roles();
    if let Some(primary) = roles.primary {
        return Some(primary);
    }
    config().model.clone()
}

static CONFIG: std::sync::OnceLock<yi_types::config::UserConfig> = std::sync::OnceLock::new();

/// X7: the one config load, strict, before dispatch — a typo that reads as an unset default
/// is the failure nobody sees. It lives here because no fs or `$HOME` may reach yi-types.
fn load_config() -> Result<(), String> {
    let Some(home) = std::env::var_os("HOME") else {
        return set_config(yi_types::config::UserConfig::default());
    };
    yi_runtime::set_catalog_cache_dir(std::path::Path::new(&home).join(".yi/catalog"));
    set_config(read_config(std::path::Path::new(&home))?)
}

/// A relative home would read `<cwd>/.yi/config.json`, which is X7's project
/// layer, not this one; [`load_config`] is the only caller for that reason.
fn read_config(home: &std::path::Path) -> Result<yi_types::config::UserConfig, String> {
    let path = home.join(".yi/config.json");
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(yi_types::config::UserConfig::default());
        }
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    serde_json::from_str(&raw).map_err(|error| format!("{}: {error}", path.display()))
}

/// Invariant: nothing may read the config before this lands it, or the strict
/// load would be a no-op behind a default somebody else already installed.
fn set_config(loaded: yi_types::config::UserConfig) -> Result<(), String> {
    match CONFIG.set(loaded) {
        Ok(()) => Ok(()),
        Err(_) => Err("config was already read before it was loaded".to_owned()),
    }
}

fn config() -> &'static yi_types::config::UserConfig {
    CONFIG.get_or_init(yi_types::config::UserConfig::default)
}

/// X7: `thinking` in the user config, overridden by `--thinking`.
fn configured_thinking() -> Option<Effort> {
    config().thinking
}

fn configured_roles() -> yi_types::config::ModelRoles {
    config().models.clone().unwrap_or_default()
}

fn configured_auto_background() -> Option<std::time::Duration> {
    let millis = config().bash.as_ref()?.auto_background_ms?;
    (millis > 0).then(|| std::time::Duration::from_millis(millis))
}

/// §12: an unset role falls back to the primary model. Naming `models.advisor` is what turns
/// the LLM reviewer on (D28/D50); an unknown selector warns and leaves the advisor silent.
fn advisor_model() -> Option<Model> {
    let spec = configured_roles().advisor?;
    let resolved = resolve(&spec);
    if resolved.is_none() {
        eprintln!("warning: unknown advisor model {spec}; the advisor stays silent");
    }
    resolved
}

/// Naming `models.autoReview` is the switch for the M7 permission reviewer
/// (D81); an unknown selector warns and auto mode stays deterministic.
fn auto_review_model() -> Option<Model> {
    let spec = configured_roles().auto_review?;
    let resolved = resolve(&spec);
    if resolved.is_none() {
        eprintln!("warning: unknown autoReview model {spec}; the auto reviewer stays off");
    }
    resolved
}

fn summarizer_model(args: &Args) -> Option<Model> {
    let spec = configured_roles().summarizer?;
    match resolve(&spec) {
        Some(model) => Some(model),
        None => {
            eprintln!(
                "warning: unknown summarizer model {spec}; using {}",
                args.model
            );
            None
        }
    }
}

fn effective_cwd(args: &Args) -> std::path::PathBuf {
    args.cwd.as_ref().map_or_else(
        || std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        std::path::PathBuf::from,
    )
}

fn tty_ask(ask: &yi_runtime::PermissionAsk<'_>) -> yi_runtime::AskOutcome {
    use std::io::Write;
    eprintln!("\n{}\n{}", ask.title, ask.text());
    eprint!("Allow? [y]es once / [a]lways / [N]o: ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return yi_runtime::AskOutcome::Reject;
    }
    match line.trim().to_lowercase().as_str() {
        "y" | "yes" => yi_runtime::AskOutcome::AllowOnce,
        "a" | "always" => yi_runtime::AskOutcome::AllowAlways,
        _ => yi_runtime::AskOutcome::Reject,
    }
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

fn build_session(
    args: &Args,
    asker: Option<yi_runtime::Asker>,
) -> Result<(AgentSession, std::sync::Arc<yi_runtime::SubagentHost>), i32> {
    if args.model.is_empty() {
        eprintln!(
            "error: no model configured (pass --model provider/id or set \"model\" in ~/.yi/config.json)"
        );
        return Err(2);
    }
    let Some(model) = resolve(&args.model) else {
        eprintln!("error: unknown model {} (use provider/id)", args.model);
        return Err(2);
    };
    let faux = model.provider == "faux";
    let api_key = yi_ai_key(&model.provider);
    if api_key.is_none() && !faux {
        eprintln!(
            "error: no API key for provider {} (set the provider env var)",
            model.provider
        );
        return Err(4);
    }
    let interactive = {
        use std::io::IsTerminal;
        std::io::stdin().is_terminal()
    };
    // Incident: an inherited proxy value ureq cannot dial refused every faux run too, so an
    // operator's shell broke `just check`. E2 guards egress; faux never leaves the process.
    let proxy = if faux {
        None
    } else {
        match proxy_from_env() {
            Ok(proxy) => proxy,
            Err(message) => {
                eprintln!("error: {message}");
                return Err(2);
            }
        }
    };
    if !faux {
        catalog::spawn_refresh(&model.provider, api_key.as_ref(), proxy.as_ref());
    }
    let provider = Arc::new(
        ProviderStream::new(api_key, None)
            .with_long_cache(interactive)
            .with_proxy(proxy),
    );
    if faux {
        provider.queue_faux(vec![yi_ai_faux_reply(&args.prompt)]);
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
    let cwd = effective_cwd(args);
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    let session_dir = default_session_dir(args);
    let lane = match claim_lane(args, &home) {
        Ok(lane) => lane,
        Err(message) => {
            eprintln!("error: lane: {message}");
            return Err(1);
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
        .with_sandbox(yi_runtime::workspace_sandbox(&work, &home, &session_dir)),
    );
    let tools_home = home.clone();
    session.install_extensions(session_extensions(args, &work));
    let provider = std::sync::Arc::clone(session_provider(&session));
    let freeform_grammar = config()
        .edit
        .as_ref()
        .and_then(|edit| edit.freeform_grammar)
        .unwrap_or(false);
    let host = yi_runtime::attach_runtime(
        &mut session,
        yi_runtime::RuntimeWiring {
            provider,
            system_prompt: String::new(),
            tool_execution: yi_loop_default(),
            cwd: work.clone(),
            home: home.clone(),
            lane_slots: configured_lanes()
                .slots
                .unwrap_or(yi_runtime::lane::DEFAULT_SLOTS),
            mcp_read: Some(std::sync::Arc::new(McpOneShot)),
            broker: Some(broker),
            tools: std::sync::Arc::new(move || {
                let mut tools = yi_runtime::builtin_tools_with(freeform_grammar);
                for tool in yi_runtime::discover_exec_tools(&tools_home.join(".yi/tools")) {
                    tools.push(std::sync::Arc::new(tool));
                }
                tools
            }),
            depth: 0,
            max_depth: 1,
            rlm_dir: default_session_dir(args).join(format!("rlm-{}", std::process::id())),
            sessions_dir: Some(default_session_dir(args)),
            summarizer: summarizer_model(args),
            advisor: advisor_model(),
            auto_review: auto_review_model(),
            parent_link: None,
            wall: yi_runtime::Wall::default(),
            plan_stale_turns: config()
                .plan
                .as_ref()
                .and_then(|plan| plan.stale_reminder_turns),
            plans_dir: configured_plans_dir(&work),
            auto_background: configured_auto_background(),
            kernel_prewarm: config()
                .kernel
                .as_ref()
                .and_then(|kernel| kernel.prewarm)
                .unwrap_or(true),
            kernels: yi_runtime::fetch::KernelServiceMap::new(),
        },
    );
    session.set_lane(yi_runtime::lane::land::LaneHandle::new(
        lane,
        configured_lanes().land,
        session.events_sender(),
        session.heartbeat_hook(),
    ));
    Ok((session, host))
}

fn session_extensions(args: &Args, work: &std::path::Path) -> yi_runtime::ExtensionHost {
    yi_runtime::ext::install(yi_runtime::ExtOptions {
        cwd: work.to_path_buf(),
        home: std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_default(),
        mode: args.mode,
        user_system: args.system.clone(),
        schema_instruction: args
            .schema
            .as_deref()
            .and_then(|spec| yi_runtime::schema::Schema::load(spec).ok())
            .map(|schema| schema.instruction()),
    })
}

/// `plans.dir`, relative to the workspace root unless absolute. Unset leaves
/// the resolver's own `.yi/plans` default in place.
fn configured_plans_dir(workspace: &std::path::Path) -> Option<std::path::PathBuf> {
    let dir = config().plans.as_ref()?.dir.as_deref()?;
    let path = std::path::PathBuf::from(dir);
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
        let exe = std::env::current_exe().map_err(|error| error.to_string())?;
        #[expect(
            clippy::disallowed_methods,
            reason = "every MCP call goes out through the one-shot CLI, never a held socket"
        )]
        let output = std::process::Command::new(exe)
            .args(["mcp", "--json"])
            .arg(format!("@{server}"))
            .args(["resources-read", resource])
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
}

fn run_why(args: &Args) -> i32 {
    let cwd = effective_cwd(args);
    let options = why::Options {
        plans_dir: configured_plans_dir(&cwd),
        cwd,
        json: args.json,
    };
    why::run(&args.prompt, &options)
}

fn run_plan(args: &Args) -> i32 {
    let cwd = effective_cwd(args);
    let options = plan::Options {
        plans_dir: configured_plans_dir(&cwd),
        session_dir: default_session_dir(args),
        cwd,
        json: args.json,
    };
    plan::run(&args.prompt, &options)
}

fn run_fetch(args: &Args) -> i32 {
    let target = args.prompt.trim();
    if target.is_empty() {
        eprintln!("usage: yi fetch <url>");
        return 2;
    }
    let url: yi_types::url::Url = match target.parse() {
        Ok(url) => url,
        Err(error) => {
            eprintln!("error: {error}");
            return 2;
        }
    };
    let workspace = effective_cwd(args);
    let mut resolver =
        yi_runtime::fetch::Resolver::new(workspace.clone(), yi_runtime::Wall::default());
    if let Some(dir) = configured_plans_dir(&workspace) {
        resolver = resolver.with_plans_dir(dir);
    }
    resolver = resolver.with_mcp_read(Arc::new(McpOneShot));
    match resolver.fetch(&url) {
        Ok(fetched) => {
            print!("{}", fetched.text);
            0
        }
        Err(error) => {
            eprintln!("error: {error}");
            1
        }
    }
}

fn run_trust(args: &Args) -> i32 {
    let cwd = effective_cwd(args);
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    let root = yi_runtime::ext::git_root(&cwd).unwrap_or(cwd.clone());
    let gate = yi_runtime::TrustGate::new(&home);
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
            let sources = yi_runtime::ext::contributions(&root, &home);
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
        if let Ok(line) = serde_json::to_string(&report.to_json()) {
            println!("{line}");
        }
    } else {
        print!("{}", yi_runtime::gate::render(&report));
    }
    i32::from(!report.allowed())
}

fn session_provider(session: &AgentSession) -> &std::sync::Arc<ProviderStream> {
    session.provider_arc()
}

fn default_session_dir(args: &Args) -> std::path::PathBuf {
    args.session_dir.as_ref().map_or_else(
        || {
            std::env::var_os("HOME").map_or_else(
                || std::path::PathBuf::from(".yi/sessions"),
                |home| std::path::Path::new(&home).join(".yi/sessions"),
            )
        },
        std::path::PathBuf::from,
    )
}

fn run(args: &Args) -> i32 {
    use std::io::IsTerminal;
    let interactive = std::io::stdin().is_terminal();
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };
    // Session wiring spawns runtime tasks (the H4 scheduler timer), so the
    // runtime context must exist before build_session.
    let session = {
        let _guard = runtime.enter();
        let asker: Option<yi_runtime::Asker> =
            interactive.then(|| std::sync::Arc::new(tty_ask) as yi_runtime::Asker);
        match build_session(args, asker) {
            Ok((session, _host)) => session,
            Err(code) => return code,
        }
    };
    match attach_store(args, &session) {
        Ok(_id) => {}
        // A requested resume that cannot be honoured is an error; an
        // unavailable store for a fresh turn only costs the recording.
        Err(error) if args.resume == Resume::Fresh => {
            eprintln!("warning: session store unavailable: {error}");
        }
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    }
    let json = args.json;
    let prompt = args.prompt.clone();
    let schema = match args.schema.as_deref().map(yi_runtime::schema::Schema::load) {
        Some(Ok(schema)) => Some(schema),
        Some(Err(error)) => {
            eprintln!("error: {error}");
            return 2;
        }
        None => None,
    };
    let mut answer = String::new();
    let lane = session.lane();
    let code = runtime.block_on(async move {
        let mut events = session.subscribe();
        if session
            .prompt_message(yi_runtime::session::user_input(&prompt))
            .is_err()
        {
            eprintln!("error: session busy");
            return 1;
        }
        let mut exit = 0;
        loop {
            let Ok(event) = events.recv().await else {
                break;
            };
            if json && let Ok(line) = serde_json::to_string(&event) {
                println!("{line}");
            }
            if let Some(chunk) = render_text(&event) {
                if schema.is_some() {
                    answer.push_str(&chunk);
                } else if !json {
                    print!("{chunk}");
                    use std::io::Write;
                    let _ = std::io::stdout().flush();
                }
            }
            match &event {
                AgentEvent::MessageEnd {
                    message:
                        AgentMessage::Assistant {
                            stop_reason: StopReason::Error,
                            error_message,
                            ..
                        },
                } => {
                    if !json {
                        eprintln!(
                            "error: {}",
                            error_message.as_deref().unwrap_or("provider error")
                        );
                        exit = 1;
                    }
                }
                AgentEvent::AgentEnd { .. } => {
                    if let Some(schema) = &schema {
                        exit = emit_structured(schema, &answer, json);
                    } else if !json {
                        println!();
                    }
                    break;
                }
                _ => {}
            }
        }
        exit
    });
    release_lane(lane.as_deref());
    code
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

/// X1: every `yi ask` turn is recorded, so `--continue` has a leaf to resume.
fn attach_store(args: &Args, session: &AgentSession) -> Result<String, String> {
    use yi_runtime::session_store::{CreateOptions, JsonlRepo, SessionRepo, lock_session};
    let mut repo = JsonlRepo::new(
        default_session_dir(args),
        effective_cwd(args).display().to_string(),
    );
    let existing = match &args.resume {
        Resume::Fresh => None,
        Resume::Leaf => sessions::latest_id(&mut repo),
        Resume::Named(id) => Some(id.clone()),
    };
    let store = match existing {
        Some(id) => repo.open(&id).map_err(|error| error.to_string())?,
        None => repo
            .create(CreateOptions::default())
            .map_err(|error| error.to_string())?,
    };
    let id = lock_session(&store).metadata().id.clone();
    session
        .attach_store(store)
        .map_err(|error| error.to_string())?;
    if let Some(lane) = session.lane()
        && let Err(error) = lane.bind_session(&id)
    {
        eprintln!("warning: lane: {error}");
    }
    repin(args, session);
    Ok(id)
}

/// The exact command that brings this session back, printed only for a session
/// that recorded something: resuming an empty one reads as a broken suggestion.
fn print_resume_hint(session: &AgentSession, id: &str) {
    if session.messages().is_empty() {
        return;
    }
    eprintln!("\n\x1b[2mResume this session with `yi --session {id}`\x1b[0m");
}

/// T14: restore the files the last turn changed, from the cwd's leaf session.
fn run_undo(args: &Args) -> i32 {
    use yi_runtime::session_store::{JsonlRepo, SessionRepo};
    let cwd = effective_cwd(args);
    let mut repo = JsonlRepo::new(default_session_dir(args), cwd.display().to_string());
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
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    match yi_runtime::undo(&store, &cwd, &home) {
        yi_runtime::UndoOutcome::Restored(changes) => {
            report_undo(&changes, args.json);
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
        if let Ok(line) = serde_json::to_string(&serde_json::json!({ "checkpoints": rows })) {
            println!("{line}");
        }
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

fn report_undo(changes: &[yi_runtime::Change], json: bool) {
    let described: Vec<serde_json::Value> = changes
        .iter()
        .map(|change| {
            serde_json::json!({
                "path": change.path.display().to_string(),
                "action": match change.kind {
                    yi_runtime::ChangeKind::Restored => "restored",
                    yi_runtime::ChangeKind::Deleted => "deleted",
                },
            })
        })
        .collect();
    if json {
        if let Ok(line) = serde_json::to_string(&serde_json::json!({ "reverted": described })) {
            println!("{line}");
        }
        return;
    }
    if changes.is_empty() {
        println!("nothing to revert");
        return;
    }
    for change in changes {
        let action = match change.kind {
            yi_runtime::ChangeKind::Restored => "restored",
            yi_runtime::ChangeKind::Deleted => "deleted ",
        };
        println!("{action}  {}", change.path.display());
    }
}

/// D10: `--schema` answers are JSON or a non-zero exit, never prose.
fn emit_structured(schema: &yi_runtime::schema::Schema, answer: &str, json: bool) -> i32 {
    let value = match yi_runtime::schema::extract(answer) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("error: {error}");
            return 3;
        }
    };
    if let Err(error) = schema.validate(&value) {
        eprintln!("error: answer does not match --schema: {error}");
        return 3;
    }
    if !json && let Ok(line) = serde_json::to_string(&value) {
        println!("{line}");
    }
    0
}

fn yi_ai_key(provider: &str) -> Option<yi_runtime::auth::Secret> {
    yi_runtime::auth::api_key(provider)
}

fn yi_ai_faux_reply(prompt: &str) -> AgentMessage {
    yi_runtime::faux::faux_assistant_message(
        vec![yi_runtime::faux::faux_text(&format!("faux: {prompt}"))],
        StopReason::Stop,
    )
}

fn yi_loop_default() -> yi_runtime::ExecutionMode {
    yi_runtime::ExecutionMode::Sequential
}

fn mcp_enabled() -> bool {
    config().mcp.as_ref().and_then(|mcp| mcp.enabled) == Some(true)
}

fn run_serve_command(args: &Args, version: &str) -> i32 {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
    };
    let socket = daemon_socket(args);
    let worker_args = shells::serve_flags(args);
    yi_acp::daemon::run_daemon(
        yi_acp::daemon::DaemonOptions {
            socket,
            worker_args,
            agent_version: version.to_owned(),
        },
        runtime,
    )
}

mod shells;
use shells::{daemon_socket, run_console_command, run_tui_command};

fn main() {
    if let Err(error) = load_config() {
        eprintln!("error: {error}");
        std::process::exit(2);
    }
    if std::env::args().nth(1).as_deref() == Some("mcp") {
        if !mcp_enabled() {
            eprintln!(
                "error: mcp is disabled; set {{\"mcp\": {{\"enabled\": true}}}} in ~/.yi/config.json"
            );
            std::process::exit(2);
        }
        let raw: Vec<String> = std::env::args().skip(2).collect();
        let token_store = config()
            .mcp
            .as_ref()
            .and_then(|mcp| mcp.token_store.as_deref());
        std::process::exit(yi_mcp_cli::run(&raw, token_store));
    }
    let args = match parse_args() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(2);
        }
    };
    let version = env!("CARGO_PKG_VERSION");
    match args.command.as_str() {
        "version" => println!("yi {version}"),
        "ask" => {
            if args.prompt.is_empty() {
                eprintln!("usage: yi ask [--model provider/id] [--json] <prompt>");
                std::process::exit(2);
            }
            std::process::exit(run(&args));
        }
        "rpc" => {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            };
            let session = {
                let _guard = runtime.enter();
                match build_session(&args, None) {
                    Ok((session, _host)) => session,
                    Err(code) => std::process::exit(code),
                }
            };
            let options = rpc::RpcOptions {
                session_dir: default_session_dir(&args),
                cwd: effective_cwd(&args),
            };
            let lane = session.lane();
            let code = rpc::run_rpc(session, &options, runtime);
            release_lane(lane.as_deref());
            std::process::exit(code);
        }
        "acp" => {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            };
            let build_args = args.clone();
            let build: yi_acp::SessionBuilder = {
                let runtime_handle = runtime.handle().clone();
                std::sync::Arc::new(move |asker| {
                    let _guard = runtime_handle.enter();
                    build_session(&build_args, asker).map_err(|code| format!("exit code {code}"))
                })
            };
            let options = yi_acp::AcpOptions {
                session_dir: default_session_dir(&args),
                cwd: effective_cwd(&args),
                build,
                agent_version: version.to_owned(),
            };
            std::process::exit(yi_acp::run_acp(options, runtime));
        }
        "undo" => std::process::exit(run_undo(&args)),
        "lanes" => std::process::exit(run_lanes(&args)),
        "trust" => std::process::exit(run_trust(&args)),
        "gate" => std::process::exit(run_gate(&args)),
        "fetch" => std::process::exit(run_fetch(&args)),
        "catalog" => std::process::exit(catalog::run(&args)),
        "why" => std::process::exit(run_why(&args)),
        "plan" => std::process::exit(run_plan(&args)),
        "sessions" => {
            let options = sessions::Options {
                session_dir: default_session_dir(&args),
                cwd: effective_cwd(&args).display().to_string(),
                json: args.json,
            };
            std::process::exit(sessions::run(&args.prompt, &options));
        }
        "stats" => {
            let options = stats::Options {
                session_dir: default_session_dir(&args),
                cwd: effective_cwd(&args).display().to_string(),
                json: args.json,
            };
            std::process::exit(stats::run(&args.prompt, &options));
        }
        "serve" => std::process::exit(run_serve_command(&args, version)),
        "console" => std::process::exit(run_console_command(&args)),
        "tui" => {
            let prompt = (!args.prompt.is_empty()).then(|| args.prompt.clone());
            std::process::exit(run_tui_command(&args, prompt));
        }
        "" => {
            use std::io::IsTerminal;
            if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
                if args.solo {
                    std::process::exit(run_tui_command(&args, None));
                }
                std::process::exit(run_console_command(&args));
            }
            println!(
                "yi {version} (yi [prompt], yi ask, yi sessions, yi stats, yi plan, yi why, yi trust, yi gate, yi fetch, yi rpc, yi acp, yi serve; more surfaces land in later phases)"
            );
        }
        other => {
            // X1: `yi <prompt words>` opens the TUI on a TTY, plain ask otherwise.
            use std::io::IsTerminal;
            let mut full = other.to_owned();
            if !args.prompt.is_empty() {
                full.push(' ');
                full.push_str(&args.prompt);
            }
            if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
                std::process::exit(run_tui_command(&args, Some(full)));
            }
            let mut ask_args = args.clone();
            ask_args.prompt = full;
            std::process::exit(run(&ask_args));
        }
    }
}
