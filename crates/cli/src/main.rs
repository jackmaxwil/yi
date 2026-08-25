#![forbid(unsafe_code)]

mod rpc;

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
    prompt: String,
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

/// Yi has no built-in default model; the user's config carries it.
fn configured_model() -> Option<String> {
    let home = std::env::var_os("HOME")?;
    let path = std::path::Path::new(&home).join(".yi/config.json");
    let content = std::fs::read_to_string(path).ok()?;
    let config: serde_json::Value = serde_json::from_str(&content).ok()?;
    config
        .get("model")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
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

fn build_session(args: &Args, asker: Option<yi_runtime::Asker>) -> Result<AgentSession, i32> {
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
    let mode_fragment = yi_runtime::mode_fragment(if args.yolo {
        yi_runtime::PermissionMode::Yolo
    } else {
        yi_runtime::PermissionMode::Ask
    });
    let system_prompt = if args.system.is_empty() {
        mode_fragment.to_owned()
    } else {
        format!("{}\n\n{mode_fragment}", args.system)
    };
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
    yi_runtime::attach_runtime(
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
        },
    );
    Ok(session)
}

fn session_system_prompt(args: &Args) -> String {
    let mode_fragment = yi_runtime::mode_fragment(if args.yolo {
        yi_runtime::PermissionMode::Yolo
    } else {
        yi_runtime::PermissionMode::Ask
    });
    if args.system.is_empty() {
        mode_fragment.to_owned()
    } else {
        format!("{}\n\n{mode_fragment}", args.system)
    }
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
            Ok(session) => session,
            Err(code) => return code,
        }
    };
    let json = args.json;
    let prompt = args.prompt.clone();
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
            if json {
                if let Ok(line) = serde_json::to_string(&event) {
                    println!("{line}");
                }
            } else if let Some(chunk) = render_text(&event) {
                print!("{chunk}");
                use std::io::Write;
                let _ = std::io::stdout().flush();
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
                    if !json {
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
                    Ok(session) => session,
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
        "serve" => {
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
            std::process::exit(yi_acp::daemon::run_daemon(
                yi_acp::daemon::DaemonOptions {
                    socket,
                    worker_args,
                    agent_version: version.to_owned(),
                },
                runtime,
            ));
        }
        "" => println!(
            "yi {version} (yi ask, yi rpc, yi acp, yi serve; more surfaces land in later phases)"
        ),
        other => {
            eprintln!("error: unknown command {other}");
            std::process::exit(2);
        }
    }
}
