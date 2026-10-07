//! `python/yi_client.py` against the real binary: a plain Python program fans out reader
//! children and gets their answers back with no model turn at the root.

use std::error::Error;
use std::process::Command;

use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use yi_runtime::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_types::message::StopReason;

#[path = "../../types/tests/support/repo.rs"]
mod repo;
#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use repo::init_repo;
use scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

#[expect(
    clippy::disallowed_methods,
    reason = "the client's contract is a separate process driving the spawned binary"
)]
fn run(program: &str, args: &[&str], dir: &std::path::Path) -> Result<String, Box<dyn Error>> {
    let out = Command::new(program)
        .args(args)
        .current_dir(dir)
        .env("HOME", dir.join("home"))
        .output()?;
    if !out.status.success() {
        return Err(format!(
            "{program} {args:?} failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(out.stdout)?)
}

const CLIENT_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../python");

const PROGRAM: &str = r#"
import json, sys
sys.path.insert(0, sys.argv[1])
from yi_client import Yi, YiError
yi_bin, repo, script, sessions = sys.argv[2:6]
with Yi(repo, model="faux/faux-1", yi=yi_bin, args=["--faux", script, "--session-dir", sessions]) as yi:
    answers = yi.ask_all(["first?", "second?", "third?"])
    big = yi.eval("'é' * 100000")
    try:
        yi.run("1/0")
        failed = None
    except YiError as error:
        failed = str(error)
print(json.dumps({"answers": answers, "big": len(big), "failed": failed}))
"#;

const HARNESS_TEMPLATE: &str = r#"
import json, sys, threading, traceback
sys.path.insert(0, sys.argv[1])
from yi_client import Yi, YiError
out = {}
def work():
    try:
{body}
    except YiError as error:
        out["error"] = str(error)
    except Exception:
        out["error"] = traceback.format_exc()
thread = threading.Thread(target=work, daemon=True)
thread.start()
thread.join(timeout=10)
print(json.dumps({report}))
"#;

fn harness(body: &str, report: &str) -> String {
    HARNESS_TEMPLATE
        .replacen("{body}", body, 1)
        .replacen("{report}", report, 1)
}

fn cancelled_program() -> String {
    harness(
        r#"        with Yi(sys.argv[3], yi=sys.argv[2], mode="ask") as yi:
            out["result"] = yi.run("1")"#,
        r#"{"finished": not thread.is_alive(), "error": out.get("error", ""), "result": out.get("result", "")}"#,
    )
}

const CANCELLED_STUB: &str = r#"#!/usr/bin/env python3
import json, os, sys
log = open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "calls.log"), "a")
log.write("argv " + " ".join(sys.argv[1:]) + "\n")
log.flush()
for line in sys.stdin:
    frame = json.loads(line)
    if frame.get("method") == "session/new":
        print(json.dumps({"jsonrpc": "2.0", "id": frame["id"], "result": {"sessionId": "s1"}}), flush=True)
    elif frame.get("method") == "_yi/kernel_execute":
        log.write(frame.get("params", {}).get("code", "") + "\n")
        log.flush()
        print(json.dumps({"jsonrpc": "2.0", "id": frame["id"], "result": {"callId": "c1"}}), flush=True)
        print(json.dumps({"jsonrpc": "2.0", "method": "session/update", "params": {"update": {
            "toolCallId": "c1", "status": "cancelled",
            "content": [{"type": "text", "content": {"type": "text", "text": "cell killed at the 600 s ceiling"}}]}}}), flush=True)
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": frame.get("id"), "result": {}}), flush=True)
"#;

#[test]
fn a_cancelled_cell_update_raises_instead_of_hanging_the_client() -> TestResult {
    let dir = Scratch::new("yi-python-client-cancelled")?;
    dir.home()?;
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo)?;
    let stub = dir.join("yi-stub.py");
    std::fs::write(&stub, CANCELLED_STUB)?;
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755))?;
    let program = dir.join("program.py");
    std::fs::write(&program, cancelled_program())?;
    let out = run(
        "python3",
        &[
            &program.display().to_string(),
            CLIENT_DIR,
            &stub.display().to_string(),
            &repo.display().to_string(),
        ],
        &dir,
    )?;
    let report: Value = serde_json::from_str(out.trim())?;
    assert_eq!(
        report["finished"], true,
        "run() returned instead of waiting forever on a terminal status it does not list: {report}"
    );
    assert!(
        report["error"]
            .as_str()
            .is_some_and(|text| text.contains("600 s ceiling")),
        "the cancelled update's own text reached YiError: {report}"
    );
    let calls = std::fs::read_to_string(dir.join("calls.log"))?;
    assert!(
        calls.lines().any(|line| line.contains("--confirm")),
        "mode='ask' maps to the CLI's --confirm flag: {calls}"
    );
    assert!(
        calls.lines().any(|code| code.contains("rlm.wait")),
        "close() drained the family with an rlm.wait cell before closing stdin: {calls}"
    );
    Ok(())
}

const WIRED_STUB: &str = r#"#!/usr/bin/env python3
import json, sys
for line in sys.stdin:
    frame = json.loads(line)
    if frame.get("method") == "session/new":
        print(json.dumps({"jsonrpc": "2.0", "id": frame["id"], "result": {"sessionId": "s1"}}), flush=True)
    elif frame.get("method") == "_yi/kernel_execute":
        print(json.dumps({"jsonrpc": "2.0", "id": frame["id"], "result": {"callId": "c1"}}), flush=True)
        print(json.dumps({"jsonrpc": "2.0", "method": "session/update", "params": {"update": {
            "status": "completed", "content": []}}}), flush=True)
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": frame.get("id"), "result": {}}), flush=True)
"#;

fn wired_program() -> String {
    harness(
        r#"        with Yi(sys.argv[3], yi=sys.argv[2]) as yi:
            out["result"] = yi.run("1")"#,
        r#"{"finished": not thread.is_alive(), "error": out.get("error", "")}"#,
    )
}

#[test]
fn a_terminal_update_without_a_tool_call_id_raises_instead_of_hanging() -> TestResult {
    let dir = Scratch::new("yi-python-client-wired")?;
    dir.home()?;
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo)?;
    let stub = dir.join("yi-stub.py");
    std::fs::write(&stub, WIRED_STUB)?;
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755))?;
    let program = dir.join("program.py");
    std::fs::write(&program, wired_program())?;
    let out = run(
        "python3",
        &[
            &program.display().to_string(),
            CLIENT_DIR,
            &stub.display().to_string(),
            &repo.display().to_string(),
        ],
        &dir,
    )?;
    let report: Value = serde_json::from_str(out.trim())?;
    assert_eq!(
        report["finished"], true,
        "run() hung waiting for a toolCallId the wire stopped carrying: {report}"
    );
    assert!(
        report["error"]
            .as_str()
            .is_some_and(|text| text.contains("toolCallId")),
        "the wire-shape change reached YiError: {report}"
    );
    Ok(())
}

const LEAK_STUB: &str = r#"#!/usr/bin/env python3
import json, os, sys
open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "stub.pid"), "w").write(str(os.getpid()))
for line in sys.stdin:
    frame = json.loads(line)
    if frame.get("method") == "session/new":
        print(json.dumps({"jsonrpc": "2.0", "id": frame["id"], "error": {"message": "refused"}}), flush=True)
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": frame.get("id"), "result": {}}), flush=True)
"#;

fn leak_program() -> String {
    harness(
        r#"        import os, time
        try:
            Yi(sys.argv[3], yi=sys.argv[2])
        except YiError:
            pass
        pid = int(open(sys.argv[4]).read())
        out["dead"] = False
        for _ in range(100):
            try:
                os.kill(pid, 0)
                time.sleep(0.05)
            except ProcessLookupError:
                out["dead"] = True
                break"#,
        r#"{"dead": out.get("dead", False)}"#,
    )
}

#[test]
fn a_refused_handshake_kills_the_spawned_process_instead_of_leaking_it() -> TestResult {
    let dir = Scratch::new("yi-python-client-leak")?;
    dir.home()?;
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo)?;
    let stub = dir.join("yi-stub.py");
    std::fs::write(&stub, LEAK_STUB)?;
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755))?;
    let program = dir.join("program.py");
    std::fs::write(&program, leak_program())?;
    let out = run(
        "python3",
        &[
            &program.display().to_string(),
            CLIENT_DIR,
            &stub.display().to_string(),
            &repo.display().to_string(),
            &dir.join("stub.pid").display().to_string(),
        ],
        &dir,
    )?;
    let report: Value = serde_json::from_str(out.trim())?;
    assert_eq!(
        report["dead"], true,
        "the refused session/new left the spawned yi process alive: {report}"
    );
    Ok(())
}

const PERMISSION_PROGRAM: &str = r#"
import json, os, sys
sys.path.insert(0, sys.argv[1])
from yi_client import Yi
yi_bin, repo, script, sessions = sys.argv[2:6]
with Yi(repo, model="faux/faux-1", yi=yi_bin, args=["--faux", script, "--session-dir", sessions], mode="confirm") as yi:
    worker = yi.run("h = await rlm.run('write the note', role='worker')\n"
                    "r = await h.result(timeout=25)\nprint(r.get('text', ''))")
    after = yi.run("print('still here')")
print(json.dumps({"wrote": os.path.exists(os.path.join(repo, "note.txt")),
                  "worker": worker, "after": after}))
"#;

#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn a_permission_ask_is_refused_and_the_client_continues() -> TestResult {
    let dir = Scratch::new("yi-python-client-permission")?;
    let repo = dir.join("repo");
    init_repo(&repo)?;
    let script = dir.join("script.jsonl");
    let turns = [
        serde_json::to_string(&faux_assistant_message(
            vec![faux_tool_call(
                "c1",
                "write",
                serde_json::json!({"path": "note.txt", "content": "tidy\n"})
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
            )],
            StopReason::ToolUse,
        ))?,
        serde_json::to_string(&faux_assistant_message(
            vec![faux_text("denied")],
            StopReason::Stop,
        ))?,
    ];
    std::fs::write(&script, turns.join("\n"))?;
    let program = dir.join("program.py");
    std::fs::write(&program, PERMISSION_PROGRAM)?;
    let out = run(
        "python3",
        &[
            &program.display().to_string(),
            CLIENT_DIR,
            env!("CARGO_BIN_EXE_yi"),
            &repo.display().to_string(),
            &script.display().to_string(),
            &dir.join("sessions").display().to_string(),
        ],
        &dir,
    )?;
    let report: Value = serde_json::from_str(out.trim())?;
    assert_eq!(
        report["wrote"], false,
        "a permission ask the nobody approves is a refusal, not a write: {report}"
    );
    assert!(
        report["worker"]
            .as_str()
            .is_some_and(|text| text.contains("denied")),
        "the worker turn continued past the refused ask to its next reply: {report}"
    );
    assert!(
        report["after"]
            .as_str()
            .is_some_and(|text| text.contains("still here")),
        "the client ran another cell after the refusal instead of hanging: {report}"
    );
    Ok(())
}

#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn a_python_program_fans_out_readers_with_no_model_turn_at_the_root() -> TestResult {
    let dir = Scratch::new("yi-python-client")?;
    let repo = dir.join("repo");
    init_repo(&repo)?;
    let replies = ["alpha", "beta", "gamma"]
        .iter()
        .map(|text| {
            serde_json::to_string(&faux_assistant_message(
                vec![faux_text(text)],
                StopReason::Stop,
            ))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let script = dir.join("script.jsonl");
    std::fs::write(&script, replies.join("\n"))?;
    let program = dir.join("program.py");
    std::fs::write(&program, PROGRAM)?;
    let out = run(
        "python3",
        &[
            &program.display().to_string(),
            CLIENT_DIR,
            env!("CARGO_BIN_EXE_yi"),
            &repo.display().to_string(),
            &script.display().to_string(),
            &dir.join("sessions").display().to_string(),
        ],
        &dir,
    )?;
    let report: Value = serde_json::from_str(out.trim())?;
    let mut answers: Vec<&str> = report["answers"]
        .as_array()
        .ok_or("no answers")?
        .iter()
        .map(|answer| answer["text"].as_str().unwrap_or_default())
        .collect();
    answers.sort_unstable();
    assert_eq!(
        answers,
        ["alpha", "beta", "gamma"],
        "three scripted replies feed three children only if the root took no model turn: {report}"
    );
    assert_eq!(
        report["big"], 100_000,
        "eval returns a value past the 64K output cap whole"
    );
    let failed = report["failed"]
        .as_str()
        .ok_or("a failing cell did not raise")?;
    assert!(failed.contains("ZeroDivisionError"), "{failed}");
    Ok(())
}
