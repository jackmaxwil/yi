//! `yi worker replace`: swap one root's running `yi acp` worker for a freshly built binary.
//! The supervisor and the console stay up; the sessions resume from the ledger.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use crate::Args;

/// The daemon's own budget is a prepare plus a restore timeout; this waits past both.
const ANSWER_MAX: Duration = Duration::from_secs(180);

pub(crate) fn run(args: &Args) -> i32 {
    let Some(binary) = args.binary.as_deref() else {
        eprintln!("error: yi worker replace needs --binary <path>");
        return 2;
    };
    let socket = crate::shells::daemon_socket(args);
    let root = crate::effective_cwd(args);
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "_yi/worker_replace",
        "params": {"cwd": root.to_string_lossy(), "binary": binary},
    });
    match ask(&socket, &request.to_string()) {
        Ok(answer) => report(args, &answer),
        Err(why) => {
            if args.if_running && std::os::unix::net::UnixStream::connect(&socket).is_err() {
                println!(
                    "no daemon is running on {}; nothing to replace",
                    socket.display()
                );
                return 0;
            }
            eprintln!("error: {why}");
            1
        }
    }
}

fn ask(socket: &Path, line: &str) -> Result<Value, String> {
    let unreachable = |error: std::io::Error| {
        format!(
            "cannot reach the daemon at {}: {error}; run `yi serve` first",
            socket.display()
        )
    };
    let mut stream = std::os::unix::net::UnixStream::connect(socket).map_err(unreachable)?;
    let _timeout = stream.set_read_timeout(Some(ANSWER_MAX));
    let mut payload = line.as_bytes().to_vec();
    payload.push(b'\n');
    stream
        .write_all(&payload)
        .map_err(|error| format!("{error}"))?;
    let mut reader = BufReader::new(stream);
    loop {
        let mut answer = String::new();
        let read = reader
            .read_line(&mut answer)
            .map_err(|error| format!("the daemon did not answer: {error}"))?;
        if read == 0 {
            return Err("the daemon closed before it answered".to_owned());
        }
        let Ok(frame) = serde_json::from_str::<Value>(&answer) else {
            continue;
        };
        if frame.get("id").and_then(Value::as_u64) != Some(1) {
            continue;
        }
        if let Some(error) = frame.pointer("/error/message").and_then(Value::as_str) {
            return Err(error.to_owned());
        }
        return Ok(frame.get("result").cloned().unwrap_or(Value::Null));
    }
}

fn report(args: &Args, answer: &Value) -> i32 {
    if args.json {
        println!("{answer}");
        return 0;
    }
    let field = |key: &str| answer.get(key).and_then(Value::as_u64).unwrap_or(0);
    let binary = answer.get("binary").and_then(Value::as_str).unwrap_or("");
    let sessions = answer
        .get("sessions")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    println!(
        "worker generation {} -> {} serving {binary}; {sessions} session(s) resumed from the ledger",
        field("from"),
        field("to")
    );
    0
}
