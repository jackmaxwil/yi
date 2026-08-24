#![forbid(unsafe_code)]

mod rpc;

use std::sync::Arc;

use yi_runtime::{AgentSession, ProviderStream, SessionConfig, resolve_model};
use yi_types::event::AgentEvent;
use yi_types::event::AssistantMessageEvent;
use yi_types::message::{AgentMessage, StopReason};
use yi_types::model::{Model, ModelCost};

struct Args {
    command: String,
    model: String,
    system: String,
    thinking: Option<String>,
    json: bool,
    yolo: bool,
    session_dir: Option<String>,
    cwd: Option<String>,
    prompt: String,
}

fn parse_args() -> Result<Args, lexopt::Error> {
    use lexopt::prelude::*;
    let mut command = String::new();
    let mut model = "anthropic/claude-opus-4-5".to_owned();
    let mut system = String::new();
    let mut thinking = None;
    let mut json = false;
    let mut yolo = false;
    let mut session_dir = None;
    let mut cwd = None;
    let mut prompt_parts: Vec<String> = Vec::new();
    let mut parser = lexopt::Parser::from_env();
    while let Some(argument) = parser.next()? {
        match argument {
            Long("version") => {
                command = "version".to_owned();
            }
            Long("model") => model = parser.value()?.string()?,
            Long("system") => system = parser.value()?.string()?,
            Long("thinking") => thinking = Some(parser.value()?.string()?),
            Long("json") => json = true,
            Long("yolo") => yolo = true,
            Long("session-dir") => session_dir = Some(parser.value()?.string()?),
            Long("cwd") => cwd = Some(parser.value()?.string()?),
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
        model,
        system,
        thinking,
        json,
        yolo,
        session_dir,
        cwd,
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

fn effective_cwd(args: &Args) -> std::path::PathBuf {
    args.cwd.as_ref().map_or_else(
        || std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        std::path::PathBuf::from,
    )
}

fn build_session(args: &Args) -> Result<AgentSession, i32> {
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
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: args.system.clone(),
            model,
            thinking_level: args.thinking.clone(),
            tool_execution: yi_loop_default(),
        },
        provider,
    );
    if args.yolo {
        let cwd = effective_cwd(args);
        let mut tools = yi_runtime::builtin_tools();
        if let Some(home) = std::env::var_os("HOME") {
            for tool in
                yi_runtime::discover_exec_tools(&std::path::Path::new(&home).join(".yi/tools"))
            {
                tools.push(std::sync::Arc::new(tool));
            }
        }
        session.use_tools(tools, cwd);
    }
    Ok(session)
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
    let session = match build_session(args) {
        Ok(session) => session,
        Err(code) => return code,
    };
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

fn main() {
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
            let session = match build_session(&args) {
                Ok(session) => session,
                Err(code) => std::process::exit(code),
            };
            let options = rpc::RpcOptions {
                session_dir: default_session_dir(&args),
                cwd: effective_cwd(&args),
            };
            std::process::exit(rpc::run_rpc(session, &options));
        }
        "" => println!("yi {version} (yi ask, yi rpc; more surfaces land in later phases)"),
        other => {
            eprintln!("error: unknown command {other}");
            std::process::exit(2);
        }
    }
}
