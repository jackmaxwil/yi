#![forbid(unsafe_code)]

mod rpc;
mod schema;
mod sessions;

use std::sync::Arc;

use yi_runtime::{AgentSession, ProviderStream, SessionConfig, resolve_model};
use yi_types::event::AgentEvent;
use yi_types::event::AssistantMessageEvent;
use yi_types::message::{AgentMessage, StopReason};
use yi_types::model::{Model, ModelCost};

#[derive(Clone)]
struct Args {
    command: String,
    model: String,
    system: String,
    thinking: Option<String>,
    json: bool,
    yolo: bool,
    session_dir: Option<String>,
    cwd: Option<String>,
    socket: Option<String>,
    headless: bool,
    keys: Option<String>,
    frames: Option<String>,
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
    let mut yolo = false;
    let mut session_dir = None;
    let mut cwd = None;
    let mut socket = None;
    let mut headless = false;
    let mut keys = None;
    let mut frames = None;
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
            Long("thinking") => thinking = Some(parser.value()?.string()?),
            Long("json") => json = true,
            Long("yolo") => yolo = true,
            Long("session-dir") => session_dir = Some(parser.value()?.string()?),
            Long("cwd") => cwd = Some(parser.value()?.string()?),
            Long("socket") => socket = Some(parser.value()?.string()?),
            Long("headless") => headless = true,
            Long("keys") => keys = Some(parser.value()?.string()?),
            Long("frames") => frames = Some(parser.value()?.string()?),
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
    Ok(Args {
        command,
        model: model.or_else(configured_model).unwrap_or_default(),
        system,
        thinking,
        json,
        yolo,
        session_dir,
        cwd,
        socket,
        headless,
        keys,
        frames,
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
    config_value()?
        .get("model")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

fn config_value() -> Option<serde_json::Value> {
    let home = std::env::var_os("HOME")?;
    let path = std::path::Path::new(&home).join(".yi/config.json");
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn configured_roles() -> yi_types::config::ModelRoles {
    config_value()
        .and_then(|config| config.get("models").cloned())
        .and_then(|models| serde_json::from_value(models).ok())
        .unwrap_or_default()
}

/// §12: an unset role falls back to the primary model.
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

fn tty_ask(title: &str, description: &str) -> yi_runtime::AskOutcome {
    use std::io::Write;
    eprintln!("\n{title}\n{description}");
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
    let api_key = yi_ai_key(&model.provider);
    if api_key.is_none() && model.provider != "faux" {
        eprintln!(
            "error: no API key for provider {} (set the provider env var)",
            model.provider
        );
        return Err(4);
    }
    let provider = Arc::new(ProviderStream::new(api_key, None));
    if model.provider == "faux" {
        provider.queue_faux(vec![yi_ai_faux_reply(&args.prompt)]);
    }
    let system_prompt = session_system_prompt(args);
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt,
            model,
            thinking_level: args.thinking.clone(),
            tool_execution: yi_loop_default(),
        },
        provider,
    );
    let cwd = effective_cwd(args);
    let mode = if args.yolo {
        yi_runtime::PermissionMode::Yolo
    } else {
        yi_runtime::PermissionMode::Ask
    };
    let broker = std::sync::Arc::new(yi_runtime::PermissionBroker::new(
        mode,
        cwd.clone(),
        Vec::new(),
        asker,
        session.events_sender(),
    ));
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    let tools_home = home.clone();
    let system_prompt = session_system_prompt(args);
    let provider = std::sync::Arc::clone(session_provider(&session));
    let host = yi_runtime::attach_runtime(
        &mut session,
        yi_runtime::RuntimeWiring {
            provider,
            system_prompt,
            tool_execution: yi_loop_default(),
            cwd,
            home: home.clone(),
            broker: Some(broker),
            tools: std::sync::Arc::new(move || {
                let mut tools = yi_runtime::builtin_tools();
                for tool in yi_runtime::discover_exec_tools(&tools_home.join(".yi/tools")) {
                    tools.push(std::sync::Arc::new(tool));
                }
                tools
            }),
            depth: 0,
            max_depth: 1,
            rlm_dir: default_session_dir(args).join(format!("rlm-{}", std::process::id())),
            summarizer: summarizer_model(args),
        },
    );
    Ok((session, host))
}

fn session_system_prompt(args: &Args) -> String {
    let mode_fragment = yi_runtime::mode_fragment(if args.yolo {
        yi_runtime::PermissionMode::Yolo
    } else {
        yi_runtime::PermissionMode::Ask
    });
    let mut prompt = if args.system.is_empty() {
        format!("{}\n{mode_fragment}", yi_runtime::identity_fragment())
    } else {
        format!(
            "{}\n{}\n\n{mode_fragment}",
            yi_runtime::identity_fragment(),
            args.system
        )
    };
    if let Some(spec) = &args.schema
        && let Ok(schema) = schema::Schema::load(spec)
    {
        prompt.push_str("\n\n");
        prompt.push_str(&schema.instruction());
    }
    prompt
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
    let schema = match args.schema.as_deref().map(schema::Schema::load) {
        Some(Ok(schema)) => Some(schema),
        Some(Err(error)) => {
            eprintln!("error: {error}");
            return 2;
        }
        None => None,
    };
    let mut answer = String::new();
    runtime.block_on(async move {
        let mut events = session.subscribe();
        if session.prompt(&prompt).is_err() {
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
    })
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
    Ok(id)
}

/// OMP's parting hint: the exact command that brings this session back.
/// Only for a session that recorded something — resuming an empty one
/// restores nothing and reads as a broken suggestion.
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
        let age = sessions::age_label(now.saturating_sub(entry.timestamp));
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
fn emit_structured(schema: &schema::Schema, answer: &str, json: bool) -> i32 {
    let value = match schema::extract(answer) {
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

/// D36: MCP is compiled in but runtime-gated; `mcp.enabled = true` in
/// ~/.yi/config.json is the only switch.
fn mcp_enabled() -> bool {
    let Some(home) = std::env::var_os("HOME") else {
        return false;
    };
    let path = std::path::Path::new(&home).join(".yi/config.json");
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(&content)
        .ok()
        .and_then(|config| config.pointer("/mcp/enabled").and_then(|v| v.as_bool()))
        .unwrap_or(false)
}

#[cfg(feature = "tui")]
fn run_tui_command(args: &Args, initial_prompt: Option<String>) -> i32 {
    use std::io::IsTerminal;
    if !args.headless && !std::io::stdin().is_terminal() {
        eprintln!("error: yi tui needs a terminal (use `yi ask` when piping)");
        return 2;
    }
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
    let (ask_tx, ask_rx) = std::sync::mpsc::channel::<yi_tui::AskRequest>();
    let asker: yi_runtime::Asker = std::sync::Arc::new(move |title, description| {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        let request = yi_tui::AskRequest {
            title: title.to_owned(),
            description: description.to_owned(),
            reply: reply_tx,
        };
        if ask_tx.send(request).is_err() {
            return yi_runtime::AskOutcome::Reject;
        }
        match reply_rx.recv() {
            Ok(yi_tui::AskChoice::AllowOnce) => yi_runtime::AskOutcome::AllowOnce,
            Ok(yi_tui::AskChoice::AllowAlways) => yi_runtime::AskOutcome::AllowAlways,
            _ => yi_runtime::AskOutcome::Reject,
        }
    });
    let (session, host) = {
        let _guard = runtime.enter();
        match build_session(args, Some(asker)) {
            Ok(built) => built,
            Err(code) => return code,
        }
    };
    let session_name = match attach_store(args, &session) {
        Ok(id) => id,
        Err(error) if args.resume == Resume::Fresh => {
            eprintln!("warning: session store unavailable: {error}");
            "yi".to_owned()
        }
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };
    let model = session.model();
    let options = yi_tui::TuiOptions {
        model_label: model.id.clone(),
        session_name: session_name.clone(),
        cwd: effective_cwd(args).display().to_string(),
        context_window: model.context_window,
        keys: configured_keys(),
        initial_prompt,
    };
    if args.headless {
        let script = match &args.keys {
            Some(path) => match std::fs::read_to_string(path) {
                Ok(source) => match yi_tui::parse_script(&source) {
                    Ok(script) => script,
                    Err(error) => {
                        eprintln!("error: --keys {path}: {error}");
                        return 2;
                    }
                },
                Err(error) => {
                    eprintln!("error: --keys {path}: {error}");
                    return 2;
                }
            },
            None => Vec::new(),
        };
        let drive = yi_tui::DriveOptions {
            script,
            frames_dir: args.frames.clone().map(std::path::PathBuf::from),
            width: 80,
            height: 24,
        };
        let session = std::sync::Arc::new(session);
        let code = yi_tui::run_headless(
            runtime,
            std::sync::Arc::clone(&session),
            host,
            ask_rx,
            options,
            drive,
        );
        print_resume_hint(&session, &session_name);
        return code;
    }
    let session = std::sync::Arc::new(session);
    let code = yi_tui::run_tui(
        runtime,
        std::sync::Arc::clone(&session),
        host,
        ask_rx,
        options,
    );
    print_resume_hint(&session, &session_name);
    code
}

#[cfg(feature = "tui")]
fn configured_keys() -> Vec<(String, String)> {
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    let path = std::path::Path::new(&home).join(".yi/config.json");
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    value
        .get("keys")
        .and_then(|keys| keys.as_object())
        .map(|keys| {
            keys.iter()
                .filter_map(|(key, action)| {
                    action
                        .as_str()
                        .map(|action| (key.clone(), action.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(not(feature = "tui"))]
fn run_tui_command(_args: &Args, _initial_prompt: Option<String>) -> i32 {
    eprintln!("error: this build has no TUI (rebuild with the `tui` feature); use `yi ask`");
    2
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
    let socket = args.socket.clone().map_or_else(
        || {
            std::env::var_os("HOME").map_or_else(
                || std::path::PathBuf::from(".yi/daemon.sock"),
                |home| std::path::Path::new(&home).join(".yi/daemon.sock"),
            )
        },
        std::path::PathBuf::from,
    );
    let mut worker_args = Vec::new();
    if !args.model.is_empty() {
        worker_args.push("--model".to_owned());
        worker_args.push(args.model.clone());
    }
    if let Some(dir) = &args.session_dir {
        worker_args.push("--session-dir".to_owned());
        worker_args.push(dir.clone());
    }
    if args.yolo {
        worker_args.push("--yolo".to_owned());
    }
    yi_acp::daemon::run_daemon(
        yi_acp::daemon::DaemonOptions {
            socket,
            worker_args,
            agent_version: version.to_owned(),
        },
        runtime,
    )
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("mcp") {
        if !mcp_enabled() {
            eprintln!(
                "error: mcp is disabled; set {{\"mcp\": {{\"enabled\": true}}}} in ~/.yi/config.json"
            );
            std::process::exit(2);
        }
        let raw: Vec<String> = std::env::args().skip(2).collect();
        std::process::exit(yi_mcp_cli::run(&raw));
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
            std::process::exit(rpc::run_rpc(session, &options, runtime));
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
                    build_session(&build_args, asker)
                        .map(|(session, _host)| session)
                        .map_err(|code| format!("exit code {code}"))
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
        "sessions" => {
            let options = sessions::Options {
                session_dir: default_session_dir(&args),
                cwd: effective_cwd(&args).display().to_string(),
                json: args.json,
            };
            std::process::exit(sessions::run(&args.prompt, &options));
        }
        "serve" => std::process::exit(run_serve_command(&args, version)),
        "tui" => {
            let prompt = (!args.prompt.is_empty()).then(|| args.prompt.clone());
            std::process::exit(run_tui_command(&args, prompt));
        }
        "" => {
            use std::io::IsTerminal;
            if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
                std::process::exit(run_tui_command(&args, None));
            }
            println!(
                "yi {version} (yi [prompt], yi ask, yi sessions, yi rpc, yi acp, yi serve; more surfaces land in later phases)"
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
