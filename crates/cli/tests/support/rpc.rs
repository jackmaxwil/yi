use std::error::Error;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc::{Receiver, channel};

use serde_json::Value;

use crate::yi_bin::yi_command;

/// A `yi rpc` on `dir/home` over faux, its stdout read back as one JSON frame a line.
pub fn spawn_rpc(
    dir: &Path,
    cassette: Option<&Path>,
) -> Result<(Child, ChildStdin, Receiver<Value>), Box<dyn Error>> {
    let mut command = yi_command(&dir.join("home"));
    command.args([
        "rpc",
        "--model",
        "faux/faux-1",
        "--session-dir",
        &dir.join("sessions").display().to_string(),
        "--cwd",
        &dir.display().to_string(),
    ]);
    if let Some(cassette) = cassette {
        command.args(["--faux", &cassette.display().to_string()]);
    }
    let mut child = command
        .env_remove("HTTPS_PROXY")
        .env_remove("HTTP_PROXY")
        .env_remove("NO_PROXY")
        .env_remove("no_proxy")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let (sender, frames) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let Ok(frame) = serde_json::from_str::<Value>(&line) else {
                break;
            };
            if sender.send(frame).is_err() {
                break;
            }
        }
    });
    Ok((child, stdin, frames))
}

/// Every frame `yi rpc` writes for `commands`, sent one a line before stdin closes.
pub fn run_rpc(dir: &Path, commands: &[Value]) -> Result<Vec<Value>, Box<dyn Error>> {
    let (mut child, mut stdin, frames) = spawn_rpc(dir, None)?;
    for command in commands {
        serde_json::to_writer(&mut stdin, command)?;
        stdin.write_all(b"\n")?;
    }
    drop(stdin);
    let mut collected = Vec::new();
    while let Ok(frame) = frames.recv() {
        collected.push(frame);
    }
    child.wait()?;
    Ok(collected)
}
