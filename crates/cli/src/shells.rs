//! `yi tui`, `yi console` and `yi serve` launchers, their shared drive-script loader, and the
//! daemon socket they resolve. Lifted out of main.rs, which the console landing pushed past 1200.

use crate::{
    Args, Resume, attach_store, build_session, config, default_session_dir, effective_cwd,
    exit_refused, print_resume_hint, release_lane,
};

#[cfg(feature = "tui")]
pub fn run_tui_command(args: &Args, initial_prompt: Option<String>) -> i32 {
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
    let asker: yi_runtime::Asker =
        std::sync::Arc::new(move |ask: &yi_runtime::PermissionAsk<'_>| {
            let (reply_tx, reply_rx) = std::sync::mpsc::channel();
            let request = yi_tui::AskRequest {
                title: ask.title.to_owned(),
                description: ask.text(),
                grants: ask.grants.iter().map(|grant| grant.label.clone()).collect(),
                reply: reply_tx,
            };
            if ask_tx.send(request).is_err() {
                return yi_runtime::AskOutcome::Reject;
            }
            match reply_rx.recv() {
                Ok(yi_tui::AskChoice::AllowOnce) => yi_runtime::AskOutcome::AllowOnce,
                Ok(yi_tui::AskChoice::AllowAlways(index)) => {
                    yi_runtime::AskOutcome::AllowAlways(index)
                }
                _ => yi_runtime::AskOutcome::Reject,
            }
        });
    let (session, host) = {
        let _guard = runtime.enter();
        match build_session(args, Some(asker), None) {
            Ok(built) => built,
            Err(refused) => return exit_refused(refused),
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
    let work = session
        .lane()
        .and_then(|lane| lane.path())
        .unwrap_or_else(|| effective_cwd(args));
    let lane = session
        .lane()
        .and_then(|lane| lane.row())
        .map(|(_, row)| row);
    let options = yi_tui::TuiOptions {
        model: model.clone(),
        session_name: session_name.clone(),
        cwd: work.display().to_string(),
        lane,
        context_window: model.context_window,
        session_dir: default_session_dir(args).display().to_string(),
        keys: configured_keys(),
        initial_prompt,
        // A headless run asserts on frames, so it paints on arrival; a recording is for a
        // person and keeps the configured pace.
        pace: if args.headless && args.record.is_none() {
            0
        } else {
            config()
                .tui
                .as_ref()
                .and_then(|tui| tui.pace)
                .unwrap_or(100)
        },
    };
    if args.headless {
        let script = match load_drive_script(&args.keys, yi_tui::parse_script) {
            Ok(script) => script,
            Err(code) => return code,
        };
        let drive = yi_tui::DriveOptions {
            script,
            frames_dir: args.frames.clone().map(std::path::PathBuf::from),
            record: args.record.clone().map(std::path::PathBuf::from),
            snap: args.snap.clone().map(std::path::PathBuf::from),
            deadline_secs: args.deadline.unwrap_or(60),
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
        leave_lane(&session);
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
    leave_lane(&session);
    print_resume_hint(&session, &session_name);
    code
}

/// The quit line says where the work is before the slot is handed back.
#[cfg(feature = "tui")]
fn leave_lane(session: &yi_runtime::AgentSession) {
    let lane = session.lane();
    if let Some(line) = lane.as_ref().and_then(|lane| lane.describe()) {
        eprintln!("\x1b[2m{line}\x1b[0m");
    }
    release_lane(lane.as_deref());
}

#[cfg(feature = "tui")]
fn configured_keys() -> Vec<(String, String)> {
    config()
        .keys
        .clone()
        .unwrap_or_default()
        .into_iter()
        .collect()
}

#[cfg(not(feature = "tui"))]
pub fn run_tui_command(_args: &Args, _initial_prompt: Option<String>) -> i32 {
    eprintln!("error: this build has no TUI (rebuild with the `tui` feature); use `yi ask`");
    2
}

pub fn serve_flags(args: &Args) -> Vec<String> {
    let mut flags = Vec::new();
    if !args.model.is_empty() {
        flags.push("--model".to_owned());
        flags.push(args.model.clone());
    }
    if let Some(dir) = &args.session_dir {
        flags.push("--session-dir".to_owned());
        flags.push(dir.clone());
    }
    flags.push(
        match args.mode {
            yi_runtime::PermissionMode::Yolo => "--yolo",
            yi_runtime::PermissionMode::Auto => "--auto",
            yi_runtime::PermissionMode::Ask => "--confirm",
        }
        .to_owned(),
    );
    flags
}

fn ensure_daemon(args: &Args, socket: &std::path::Path) {
    if std::os::unix::net::UnixStream::connect(socket).is_ok() {
        return;
    }
    if let Err(error) = yi_acp::daemon::spawn_detached(socket, &serve_flags(args)) {
        eprintln!("yi: could not start the daemon: {error}");
        return;
    }
    for _ in 0..60 {
        if std::os::unix::net::UnixStream::connect(socket).is_ok() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    eprintln!("yi: the daemon did not answer within 3 s; the console will keep retrying");
}

pub fn daemon_socket(args: &Args) -> std::path::PathBuf {
    args.socket.clone().map_or_else(
        || {
            std::env::var_os("HOME").map_or_else(
                || std::path::PathBuf::from(".yi/daemon.sock"),
                |home| std::path::Path::new(&home).join(".yi/daemon.sock"),
            )
        },
        std::path::PathBuf::from,
    )
}

pub fn run_serve_command(args: &Args, version: &str) -> i32 {
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
    let worker_args = serve_flags(args);
    yi_acp::daemon::run_daemon(
        yi_acp::daemon::DaemonOptions {
            socket,
            worker_args,
            agent_version: version.to_owned(),
        },
        runtime,
    )
}

pub fn faux_replies(args: &Args) -> Result<Vec<yi_types::message::AgentMessage>, String> {
    let Some(path) = &args.faux else {
        return Ok(vec![yi_runtime::faux::faux_assistant_message(
            vec![yi_runtime::faux::faux_text(&format!(
                "faux: {}",
                args.prompt
            ))],
            yi_types::message::StopReason::Stop,
        )]);
    };
    let source =
        std::fs::read_to_string(path).map_err(|error| format!("--faux {path}: {error}"))?;
    source
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(|error| format!("--faux {path}: {error}")))
        .collect()
}

#[cfg(feature = "tui")]
fn load_drive_script<S>(
    keys: &Option<String>,
    parse: impl Fn(&str) -> Result<Vec<S>, String>,
) -> Result<Vec<S>, i32> {
    let Some(path) = keys else {
        return Ok(Vec::new());
    };
    let source = std::fs::read_to_string(path).map_err(|error| {
        eprintln!("error: --keys {path}: {error}");
        2
    })?;
    parse(&source).map_err(|error| {
        eprintln!("error: --keys {path}: {error}");
        2
    })
}

/// `yi console` — the workspace shell, an ACP client over the serve daemon.
#[cfg(feature = "tui")]
pub fn run_console_command(args: &Args) -> i32 {
    let options = yi_console::ConsoleOptions {
        socket: daemon_socket(args),
        root: effective_cwd(args).display().to_string(),
        autostart: !args.headless,
        auto_side: config()
            .console
            .as_ref()
            .and_then(|console| console.auto_side)
            .unwrap_or(true),
        sidebar: yi_console::model::SidebarMode::Rail,
    };
    if !args.headless {
        ensure_daemon(args, &options.socket);
    }
    if args.headless {
        let script = match load_drive_script(&args.keys, yi_console::parse_script) {
            Ok(script) => script,
            Err(code) => return code,
        };
        let drive = yi_console::DriveOptions {
            script,
            frames_dir: args.frames.clone().map(std::path::PathBuf::from),
            record: args.record.clone().map(std::path::PathBuf::from),
            width: 80,
            height: 24,
        };
        return yi_console::run_headless(&options, drive);
    }
    yi_console::run_console(&options)
}

#[cfg(not(feature = "tui"))]
pub fn run_console_command(_args: &Args) -> i32 {
    eprintln!("error: this build has no console (rebuild with the `tui` feature)");
    2
}

pub(crate) fn session_extensions(
    args: &crate::Args,
    work: &std::path::Path,
    context_window: u64,
) -> yi_runtime::ExtensionHost {
    yi_runtime::ext::install(yi_runtime::ExtOptions {
        context_window,
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
