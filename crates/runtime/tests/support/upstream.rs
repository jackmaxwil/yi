//! A root session wired the way `yi` wires one, its requests proxied to a local stand-in that
//! hands back every body it is sent: what a test reads to see the bytes a provider would.
#![allow(dead_code, reason = "the shared harness serves several test binaries")]

use crate::scratch::Scratch;

use std::error::Error;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::{Arc, mpsc};

use serde_json::Value;
use yi_runtime::{AgentSession, ProviderStream, SessionConfig, SubagentHost};
use yi_types::model::Model;

pub(crate) const ROOT_ID: &str = "root-7";

pub(crate) type Tools = Arc<dyn Fn() -> Vec<Arc<dyn yi_tools::Tool>> + Send + Sync>;

pub(crate) fn key() -> yi_runtime::auth::Resolved {
    yi_runtime::auth::Resolved {
        secret: yi_runtime::auth::Secret::new("sk-test".to_owned()),
        kind: yi_runtime::auth::AuthKind::ApiKey,
        org: None,
        expires: None,
        headers: Vec::new(),
    }
}

/// `ext` installs the extensions `yi ask` gives a root (unattended slot included), in the broker's
/// mode, before the runtime attaches, as the CLI orders them: attach registers the memory slot.
pub(crate) fn root_with(
    scratch: &Scratch,
    model: Model,
    port: u16,
    long_cache: bool,
    broker: Option<Arc<yi_runtime::PermissionBroker>>,
    ext: bool,
    tools: Tools,
) -> Result<(AgentSession, Arc<SubagentHost>), Box<dyn Error>> {
    let provider = stream(&model, port, long_cache)?;
    root_at(scratch, model, provider, broker, ext, tools, 1)
}

pub(crate) fn stream(
    model: &Model,
    port: u16,
    long_cache: bool,
) -> Result<Arc<ProviderStream>, Box<dyn Error>> {
    Ok(Arc::new(
        ProviderStream::new(Some(ROOT_ID.to_owned()))
            .with_auth(&model.provider, key())
            .with_long_cache(long_cache)
            .with_proxy(yi_ai::request::ProxyConfig::from_values(
                Some(&format!("http://127.0.0.1:{port}")),
                None,
                None,
            )?),
    ))
}

/// [`root_with`] on a built provider at an `rlm.maxDepth` other than the default 1.
pub(crate) fn root_at(
    scratch: &Scratch,
    model: Model,
    provider: Arc<ProviderStream>,
    broker: Option<Arc<yi_runtime::PermissionBroker>>,
    ext: bool,
    tools: Tools,
    max_depth: u8,
) -> Result<(AgentSession, Arc<SubagentHost>), Box<dyn Error>> {
    let (cwd, home) = (scratch.join("ws"), scratch.join("home"));
    std::fs::create_dir_all(&cwd)?;
    std::fs::create_dir_all(&home)?;
    std::fs::write(cwd.join("notes.txt"), "red sun\nblue sky\ngreen sea\n")?;
    let context_window = model.context_window;
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: String::new(),
            model,
            thinking_level: None,
            tool_execution: yi_loop::ExecutionMode::Sequential,
        },
        Arc::clone(&provider),
    );
    if ext {
        let mut host = yi_runtime::ext::install(yi_runtime::ExtOptions {
            cwd: cwd.clone(),
            home: home.clone(),
            mode: broker
                .as_ref()
                .map_or(yi_runtime::PermissionMode::Auto, |broker| broker.mode()),
            user_system: String::new(),
            schema_instruction: None,
            context_window,
            global_skills: Vec::new(),
            can_delegate: true,
        });
        let slot = yi_runtime::ext::Slot::new(yi_runtime::ext::Rank::Mode, "unattended");
        host.attach(slot, yi_runtime::unattended_fragment().to_owned());
        session.install_extensions(host);
    }
    let host = yi_runtime::attach_runtime(
        &mut session,
        yi_runtime::RuntimeWiring {
            provider,
            system_prompt: String::new(),
            tool_execution: yi_loop::ExecutionMode::Sequential,
            cwd,
            home: home.clone(),
            lane_slots: 1,
            broker,
            tools,
            depth: 0,
            max_depth,
            rlm_dir: scratch.join("rlm"),
            family_dir: None,
            summarizer: None,
            advisor: None,
            auto_review: None,
            plan_stale_turns: None,
            plans_dir: Some(scratch.join("plans")),
            parent_link: None,
            wall: yi_runtime::Wall::default(),
            auto_background: None,
            deadline: None,
            kernel_prewarm: false,
            mcp_read: None,
            sessions_dir: None,
            kernels: yi_runtime::fetch::KernelServiceMap::new(),
        },
    );
    Ok((session, host))
}

/// Every request body, sent on as it arrives, each answered with `reply`. A reader's finish
/// wakes the root, whose own requests come here too, so bodies are found by what they ask.
pub(crate) fn stand_in(reply: String) -> std::io::Result<(u16, mpsc::Receiver<Value>)> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let (sender, bodies) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream);
            let mut length = 0usize;
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 && !line.trim().is_empty() {
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().unwrap_or(0);
                }
                line.clear();
            }
            let mut body = vec![0u8; length];
            let _ = reader.read_exact(&mut body);
            let _ = sender.send(serde_json::from_slice(&body).unwrap_or(Value::Null));
            let _ = write!(
                reader.into_inner(),
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                reply.len()
            );
        }
    });
    Ok((port, bodies))
}

/// The first body that asks `question`, waiting up to ten seconds for it.
pub(crate) async fn body_asking(
    bodies: &mut Vec<Value>,
    from: &mpsc::Receiver<Value>,
    question: &str,
) -> Result<Value, Box<dyn Error>> {
    for _ in 0..200 {
        bodies.extend(from.try_iter());
        if let Some(body) = bodies
            .iter()
            .find(|body| body.to_string().contains(question))
        {
            return Ok(body.clone());
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    Err(format!("no request asked {question}: {bodies:?}").into())
}

pub(crate) fn git(dir: &std::path::Path, args: &[&str]) -> Result<(), Box<dyn Error>> {
    let status = yi_tools::command("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(["-c", "commit.gpgsign=false", "-C"])
        .arg(dir)
        .args(args)
        .output()?
        .status;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("git {args:?}").into())
}
