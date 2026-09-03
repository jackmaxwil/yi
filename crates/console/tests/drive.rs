//! Drive-mode tests: the real console loop against an in-process fixture
//! daemon speaking ordered, scripted ACP over a scratch unix socket.

use std::error::Error;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::thread::JoinHandle;

use serde_json::{Value, json};
use yi_console::{ConsoleOptions, DriveOptions, parse_script, run_headless};

type TestResult = Result<(), Box<dyn Error>>;
type Responder = fn(&Value) -> Vec<Value>;

enum Step {
    /// Wait for a request whose method matches; reply with the responder's
    /// frames. Non-matching requests get a generic empty reply.
    Expect(&'static str, Responder),
    /// Push notifications/requests without waiting for anything.
    Push(fn() -> Vec<Value>),
    /// Drop the connection.
    Close,
    /// Wait for the client to reconnect.
    Accept,
}

fn scratch_socket(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("yi-console-test-{}-{name}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    dir.join("fixture.sock")
}

fn write_frame(stream: &mut UnixStream, frame: &Value) -> Result<(), String> {
    let mut payload = frame.to_string().into_bytes();
    payload.push(b'\n');
    stream
        .write_all(&payload)
        .map_err(|error| format!("fixture write: {error}"))
}

fn id_of(frame: &Value) -> Value {
    frame.get("id").cloned().unwrap_or(Value::Null)
}

fn ok(frame: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id_of(frame), "result": result})
}

fn generic_reply(frame: &Value) -> Option<Value> {
    let id = frame.get("id")?;
    let method = frame.get("method").and_then(Value::as_str).unwrap_or("");
    let result = if method == "session/list" {
        json!({"sessions": []})
    } else {
        json!({})
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn update(session: &str, update: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {"sessionId": session, "update": update},
    })
}

fn read_request(reader: &mut BufReader<UnixStream>) -> Option<Value> {
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line).ok()?;
        if read == 0 {
            return None;
        }
        if line.trim().is_empty() {
            continue;
        }
        return serde_json::from_str(&line).ok();
    }
}

fn fixture_loop(listener: &UnixListener, script: Vec<Step>) -> Result<(), String> {
    let accept = |listener: &UnixListener| -> Result<(UnixStream, BufReader<UnixStream>), String> {
        let stream = listener
            .accept()
            .map_err(|error| format!("fixture accept: {error}"))?
            .0;
        let reader = BufReader::new(
            stream
                .try_clone()
                .map_err(|error| format!("fixture clone: {error}"))?,
        );
        Ok((stream, reader))
    };
    let (mut stream, mut reader) = accept(listener)?;
    for step in script {
        match step {
            Step::Expect(method, respond) => loop {
                let Some(frame) = read_request(&mut reader) else {
                    return Err(format!("connection ended while expecting {method}"));
                };
                let got = frame.get("method").and_then(Value::as_str).unwrap_or("");
                if got == method || (method == "<response>" && got.is_empty()) {
                    for reply in respond(&frame) {
                        write_frame(&mut stream, &reply)?;
                    }
                    break;
                }
                if let Some(reply) = generic_reply(&frame) {
                    write_frame(&mut stream, &reply)?;
                }
            },
            Step::Push(frames) => {
                for frame in frames() {
                    write_frame(&mut stream, &frame)?;
                }
            }
            Step::Close => {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
            Step::Accept => {
                (stream, reader) = accept(listener)?;
            }
        }
    }
    // Keep the socket open until the console detaches, answering stragglers
    // generically so a slow teardown never wedges.
    while let Some(frame) = read_request(&mut reader) {
        if let Some(reply) = generic_reply(&frame) {
            write_frame(&mut stream, &reply)?;
        }
    }
    Ok(())
}

/// Its Result surfaces through the join in `run`, so a protocol mismatch fails with evidence.
fn spawn_fixture(socket: PathBuf, script: Vec<Step>) -> JoinHandle<Result<(), String>> {
    std::thread::spawn(move || {
        let _ = std::fs::remove_file(&socket);
        let listener =
            UnixListener::bind(&socket).map_err(|error| format!("fixture bind: {error}"))?;
        fixture_loop(&listener, script)
    })
}

fn init_reply(frame: &Value) -> Vec<Value> {
    vec![ok(
        frame,
        json!({
            "protocolVersion": 2,
            "info": {"name": "yi", "version": "test"},
            "capabilities": {},
            "authMethods": [],
        }),
    )]
}

fn run(name: &str, fixture: Vec<Step>, script: &str) -> TestResult {
    run_with(name, fixture, script, false)
}

fn run_with(name: &str, fixture: Vec<Step>, script: &str, autostart: bool) -> TestResult {
    let socket = scratch_socket(name);
    let server = spawn_fixture(socket.clone(), fixture);
    let steps = parse_script(script)?;
    let code = run_headless(
        &ConsoleOptions {
            socket,
            root: "/tmp/demo-root".to_owned(),
            autostart,
            auto_side: true,
        },
        DriveOptions {
            script: steps,
            frames_dir: std::env::var("CONSOLE_TEST_FRAMES").ok().map(PathBuf::from),
            width: 100,
            height: 30,
        },
    );
    // The console shut the socket down; the fixture thread ends with it.
    match server.join() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(format!("fixture: {error}").into()),
        Err(_) => return Err("fixture thread panicked".into()),
    }
    if code != 0 {
        return Err(format!("console exited {code}").into());
    }
    Ok(())
}

fn empty_list(frame: &Value) -> Vec<Value> {
    vec![ok(frame, json!({"sessions": []}))]
}

fn seen_ok(frame: &Value) -> Vec<Value> {
    vec![ok(frame, json!({}))]
}

fn two_session_list(frame: &Value) -> Vec<Value> {
    vec![ok(
        frame,
        json!({"sessions": [
            {"sessionId": "s-alpha", "attached": false},
            {"sessionId": "s-beta", "attached": false},
        ]}),
    )]
}

fn ledger_list(frame: &Value) -> Vec<Value> {
    vec![ok(
        frame,
        json!({"sessions": [
            {"sessionId": "s-beta", "cwd": "/tmp/demo-root", "attached": false,
             "unseen": 2, "lastState": "idle", "lastEventMs": 1},
        ]}),
    )]
}

fn resume_alpha(frame: &Value) -> Vec<Value> {
    vec![
        update(
            "s-alpha",
            json!({"sessionUpdate": "user_message", "messageId": "msg_1",
            "content": [{"type": "text", "text": "hello agent"}]}),
        ),
        update(
            "s-alpha",
            json!({"sessionUpdate": "agent_message", "messageId": "msg_2",
            "content": [{"type": "text", "text": "replayed world"}]}),
        ),
        ok(frame, json!({"sessionId": "s-alpha", "configOptions": []})),
    ]
}

fn resume_alpha_again(frame: &Value) -> Vec<Value> {
    vec![
        update(
            "s-alpha",
            json!({"sessionUpdate": "agent_message", "messageId": "msg_1",
            "content": [{"type": "text", "text": "replayed again"}]}),
        ),
        ok(frame, json!({"sessionId": "s-alpha", "configOptions": []})),
    ]
}

fn prompt_stream(frame: &Value) -> Vec<Value> {
    vec![
        ok(frame, json!({})),
        update(
            "s-alpha",
            json!({"sessionUpdate": "state_update", "state": "running"}),
        ),
        update(
            "s-alpha",
            json!({"sessionUpdate": "agent_message_chunk", "messageId": "msg_3",
            "content": {"type": "text", "text": "streamed answer"}}),
        ),
        update(
            "s-alpha",
            json!({"sessionUpdate": "state_update", "state": "idle"}),
        ),
    ]
}

fn permission_request() -> Vec<Value> {
    vec![json!({
        "jsonrpc": "2.0",
        "id": "perm_s-alpha_1",
        "method": "session/request_permission",
        "params": {
            "sessionId": "s-alpha",
            "title": "run rm -rf target",
            "options": [
                {"optionId": "allow_once", "name": "Allow once", "kind": "allow_once"},
                {"optionId": "reject_once", "name": "Reject", "kind": "reject_once"},
            ],
        },
    })]
}

fn permission_answer(frame: &Value) -> Vec<Value> {
    let option = frame
        .pointer("/result/outcome/optionId")
        .and_then(Value::as_str)
        .unwrap_or("");
    if option != "allow_once" {
        // Surfaced by the console's next wait-frame failing: the approval
        // marker below is only pushed for the accepted option.
        return Vec::new();
    }
    vec![
        update(
            "s-alpha",
            json!({"sessionUpdate": "state_update", "state": "running"}),
        ),
        update(
            "s-alpha",
            json!({"sessionUpdate": "agent_message", "messageId": "msg_9",
            "content": [{"type": "text", "text": "approved, continuing"}]}),
        ),
    ]
}

#[test]
fn attach_lists_sessions_with_ledger_status() -> TestResult {
    run(
        "attach",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", ledger_list),
        ],
        "wait-frame 5000 ● connected\n\
         wait-frame 5000 s-alpha\n\
         wait-frame 5000 s-beta\n\
         wait-frame 5000 ●\n\
         quit\n",
    )
}

#[test]
fn resume_replays_and_prompt_streams() -> TestResult {
    run(
        "resume",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_alpha),
            Step::Expect("_yi/seen", seen_ok),
            Step::Expect("session/prompt", prompt_stream),
        ],
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         type run the tests\n\
         key enter\n\
         wait-frame 5000 streamed answer\n\
         wait-idle 5000\n\
         quit\n",
    )
}

#[test]
fn prompt_rejected_while_daemon_unreachable() -> TestResult {
    // No listener at all: the client keeps retrying and the composer submit
    // is refused with a visible status note, never queued.
    let socket = scratch_socket("unreachable");
    let _ = std::fs::remove_file(&socket);
    let steps = parse_script(
        "wait-frame 5000 disconnected\n\
         key tab\n\
         type lost words\n\
         key enter\n\
         wait-frame 5000 not connected\n\
         quit\n",
    )?;
    let code = run_headless(
        &ConsoleOptions {
            socket,
            root: "/tmp/demo-root".to_owned(),
            autostart: false,
            auto_side: true,
        },
        DriveOptions {
            script: steps,
            frames_dir: None,
            width: 100,
            height: 30,
        },
    );
    if code != 0 {
        return Err(format!("console exited {code}").into());
    }
    Ok(())
}

#[test]
fn reconnect_wipes_and_replays() -> TestResult {
    run(
        "reconnect",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_alpha),
            Step::Expect("_yi/seen", seen_ok),
            Step::Close,
            Step::Accept,
            Step::Expect("initialize", init_reply),
            // The second replay carries different text: seeing it WITHOUT the
            // first replay's text proves the wipe-and-replay contract.
            Step::Expect("session/resume", resume_alpha_again),
        ],
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         wait-frame 8000 replayed again\n\
         wait-frame 1000 !replayed world\n\
         quit\n",
    )
}

#[test]
fn permission_request_blocks_then_answers() -> TestResult {
    run(
        "permission",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_alpha),
            Step::Expect("_yi/seen", seen_ok),
            Step::Push(permission_request),
            Step::Expect("<response>", permission_answer),
        ],
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         wait-frame 5000 rm -rf target\n\
         key a\n\
         wait-frame 5000 approved, continuing\n\
         wait-frame 5000 !rm -rf target\n\
         quit\n",
    )
}

#[test]
fn splits_zoom_tabs_and_prefix() -> TestResult {
    run(
        "splits",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_alpha),
            Step::Expect("_yi/seen", seen_ok),
        ],
        // Open a session, split right, split down, walk focus, zoom in and
        // out, close a pane, then a second tab via the ctrl+b prefix.
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         key alt-v\n\
         wait-frame 3000 no session\n\
         wait-frame 3000 ┬\n\
         key alt-s\n\
         key alt-left\n\
         key alt-z\n\
         wait-frame 3000 !no session\n\
         key alt-z\n\
         wait-frame 3000 no session\n\
         key alt-right\n\
         key alt-x\n\
         key ctrl-b\n\
         wait-frame 3000 PREFIX\n\
         key c\n\
         wait-frame 3000  1 \n\
         key alt-1\n\
         wait-frame 3000 replayed world\n\
         quit\n",
    )
}

#[test]
fn navigator_filters_and_opens() -> TestResult {
    run(
        "navigator",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", |frame| {
                vec![
                    update(
                        "s-beta",
                        json!({"sessionUpdate": "agent_message", "messageId": "msg_1",
                        "content": [{"type": "text", "text": "beta transcript"}]}),
                    ),
                    ok(frame, json!({"sessionId": "s-beta", "configOptions": []})),
                ]
            }),
            Step::Expect("_yi/seen", seen_ok),
        ],
        "wait-frame 5000 s-alpha\n\
         key alt-/\n\
         wait-frame 3000 find\n\
         type beta\n\
         wait-frame 3000 find beta\n\
         key enter\n\
         wait-frame 5000 beta transcript\n\
         quit\n",
    )
}

#[test]
fn tiny_terminal_survives_splits() -> TestResult {
    let socket = scratch_socket("tiny");
    let server = spawn_fixture(
        socket.clone(),
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
        ],
    );
    let steps = parse_script(
        "wait-frame 5000 ●\n\
         key alt-v\n\
         key alt-s\n\
         key alt-v\n\
         key alt-left\n\
         key alt-up\n\
         key alt-x\n\
         key alt-z\n\
         wait 50\n\
         quit\n",
    )?;
    let code = run_headless(
        &ConsoleOptions {
            socket,
            root: "/tmp/demo-root".to_owned(),
            autostart: false,
            auto_side: true,
        },
        DriveOptions {
            script: steps,
            frames_dir: None,
            width: 20,
            height: 8,
        },
    );
    match server.join() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(format!("fixture: {error}").into()),
        Err(_) => return Err("fixture thread panicked".into()),
    }
    if code != 0 {
        return Err(format!("console exited {code}").into());
    }
    Ok(())
}

#[test]
fn mouse_focuses_opens_and_drags() -> TestResult {
    run(
        "mouse",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_alpha),
            Step::Expect("_yi/seen", seen_ok),
        ],
        // Click the first sidebar row to open it, split, click the right
        // pane, then prove focus moved there by submitting into it.
        "wait-frame 5000 s-alpha\n\
         mouse down 2 0\n\
         wait-frame 5000 replayed world\n\
         key alt-v\n\
         wait-frame 3000 no session\n\
         mouse down 90 5\n\
         type hi\n\
         key enter\n\
         wait-frame 3000 no session in this pane\n\
         mouse down 63 5\n\
         mouse drag 70 5\n\
         mouse up 70 5\n\
         mouse scrollup 40 5\n\
         wait 30\n\
         quit\n",
    )
}

#[test]
fn background_session_notifies_after_delay() -> TestResult {
    run(
        "notify",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_alpha),
            Step::Expect("_yi/seen", seen_ok),
            Step::Expect("session/resume", |frame| {
                vec![ok(
                    frame,
                    json!({"sessionId": "s-beta", "configOptions": []}),
                )]
            }),
            Step::Expect("_yi/seen", |frame| {
                vec![
                    ok(frame, json!({})),
                    // s-alpha finishes while s-beta holds focus: this must
                    // surface as a delayed, re-validated note.
                    update(
                        "s-alpha",
                        json!({"sessionUpdate": "state_update", "state": "idle"}),
                    ),
                ]
            }),
        ],
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         key alt-v\n\
         key tab\n\
         key down\n\
         key enter\n\
         wait-frame 5000 finished while you were away\n\
         quit\n",
    )
}

fn ipython_updates() -> Vec<Value> {
    vec![
        update(
            "s-alpha",
            json!({"sessionUpdate": "tool_call_update",
            "toolCallId": "call_1", "title": "ipython", "kind": "execute",
            "status": "in_progress",
            "rawInput": {"code": "plot_drift()"}}),
        ),
        update(
            "s-alpha",
            json!({"sessionUpdate": "tool_call_update",
            "toolCallId": "call_1", "status": "completed",
            "rawOutput": {
                "stdout": "computing drift…",
                "result": "+4.1 mm",
                "attachments": 1, "attachmentMedia": [{"mime_type": "image/png", "data": "aWJvcnk="}],
            }}),
        ),
    ]
}

#[test]
fn notebook_pane_shows_cells_and_image_placeholder() -> TestResult {
    run(
        "notebook",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_alpha),
            Step::Expect("_yi/seen", seen_ok),
            Step::Expect("session/prompt", |frame| {
                let mut frames = vec![ok(frame, json!({}))];
                frames.extend(ipython_updates());
                frames
            }),
        ],
        // Open the session and prompt: the first kernel cell opens the notebook pane
        // beside it, which renders code, streams and the image placeholder.
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         type chart it\n\
         key enter\n\
         wait-frame 5000 nb:s-alpha\n\
         wait-frame 5000 In[1]\n\
         wait-frame 3000 plot_drift\n\
         wait-frame 3000 computing drift\n\
         wait-frame 3000 image 0 KB png\n\
         quit\n",
    )
}

#[test]
fn markdown_viewer_pane_renders_file() -> TestResult {
    let dir = std::env::temp_dir().join(format!("yi-console-md-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("NOTES.md");
    std::fs::write(&path, "# Drift Survey\n\nthe datum moved\n")?;
    run(
        "mdview",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
        ],
        &format!(
            "wait-frame 5000 s-alpha\n\
             key tab\n\
             key alt-/\n\
             type md {}\n\
             key enter\n\
             wait-frame 5000 Drift Survey\n\
             wait-frame 3000 the datum moved\n\
             wait-frame 3000 NOTES.md\n\
             quit\n",
            path.display()
        ),
    )
}

fn resume_with_offset(frame: &Value) -> Vec<Value> {
    let mut frames = resume_alpha(frame);
    if let Some(Value::Object(map)) = frames.last_mut().map(|reply| &mut reply["result"]) {
        map.insert("replayedTo".to_owned(), json!(2));
    }
    frames
}

#[test]
fn reconnect_reuses_offset_and_skips_replay() -> TestResult {
    run(
        "offset",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_with_offset),
            Step::Expect("_yi/seen", seen_ok),
            Step::Close,
            Step::Accept,
            Step::Expect("initialize", init_reply),
            Step::Expect("session/resume", |frame| {
                // Nothing streamed since replayedTo=2 landed, so the client
                // must skip ahead instead of wiping; the marker only appears
                // on the offset path.
                let from = frame.pointer("/params/replayFrom").and_then(Value::as_u64);
                let mut frames = Vec::new();
                if from == Some(2) {
                    frames.push(update(
                        "s-alpha",
                        json!({"sessionUpdate": "agent_message", "messageId": "msg_9",
                        "content": [{"type": "text", "text": "offset honored"}]}),
                    ));
                }
                frames.push(ok(
                    frame,
                    json!({"sessionId": "s-alpha", "configOptions": [], "replayedTo": 3}),
                ));
                frames
            }),
        ],
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         wait-frame 8000 offset honored\n\
         wait-frame 1000 replayed world\n\
         quit\n",
    )
}

#[test]
fn live_update_invalidates_offset() -> TestResult {
    run(
        "offset-invalid",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_with_offset),
            Step::Expect("_yi/seen", |frame| {
                vec![
                    ok(frame, json!({})),
                    // A live update past the offset makes it stale.
                    update(
                        "s-alpha",
                        json!({"sessionUpdate": "agent_message", "messageId": "msg_5",
                        "content": [{"type": "text", "text": "streamed since"}]}),
                    ),
                ]
            }),
            Step::Close,
            Step::Accept,
            Step::Expect("initialize", init_reply),
            Step::Expect("session/resume", |frame| {
                let from = frame.pointer("/params/replayFrom").and_then(Value::as_u64);
                let mut frames = Vec::new();
                if from == Some(0) {
                    frames.push(update(
                        "s-alpha",
                        json!({"sessionUpdate": "agent_message", "messageId": "msg_1",
                        "content": [{"type": "text", "text": "full replay again"}]}),
                    ));
                }
                frames.push(ok(
                    frame,
                    json!({"sessionId": "s-alpha", "configOptions": [], "replayedTo": 4}),
                ));
                frames
            }),
        ],
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         wait-frame 5000 streamed since\n\
         wait-frame 8000 full replay again\n\
         wait-frame 1000 !streamed since\n\
         quit\n",
    )
}

/// ⌘ chords reach the same actions as the ⌥ table once the terminal reports the super
/// modifier, and the first one flips the hint bar to ⌘ glyphs.
#[test]
fn cmd_chords_split_close_and_hide_the_sidebar() -> TestResult {
    run(
        "cmd-chords",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_alpha),
            Step::Expect("_yi/seen", seen_ok),
        ],
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         wait-frame 3000 ⌥v/⌥s split\n\
         cmd-d\n\
         wait-frame 3000 no session\n\
         wait-frame 3000 ⌘⇧M zoom\n\
         cmd-x\n\
         wait-frame 3000 !no session\n\
         wait-frame 3000 s-beta\n\
         cmd-b\n\
         wait-frame 3000 !s-beta\n\
         cmd-b\n\
         wait-frame 3000 s-beta\n\
         quit\n",
    )
}

/// ⌘J beside a session opens that session's notebook to the right; ⌘J on it closes it.
#[test]
fn cmd_j_toggles_the_notebook_pane() -> TestResult {
    run(
        "cmd-notebook",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_alpha),
            Step::Expect("_yi/seen", seen_ok),
        ],
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         cmd-j\n\
         wait-frame 3000 nb:s-alpha\n\
         wait-frame 3000 no kernel cells yet\n\
         cmd-j\n\
         wait-frame 3000 !nb:s-alpha\n\
         wait-frame 3000 replayed world\n\
         quit\n",
    )
}

fn new_session_reply(frame: &Value) -> Vec<Value> {
    vec![ok(
        frame,
        json!({"sessionId": "s-new", "configOptions": []}),
    )]
}

/// Bare `yi` opens a working pane: an empty root gets a fresh session without a keypress.
#[test]
fn workspace_autostarts_new_session_when_root_is_empty() -> TestResult {
    run_with(
        "autostart-new",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/new", new_session_reply),
        ],
        "wait-frame 5000 s-new\n\
         wait-frame 3000 workspace ·\n\
         quit\n",
        true,
    )
}

/// A root with sessions resumes its first one instead of minting another.
#[test]
fn workspace_resumes_first_session_when_root_has_one() -> TestResult {
    run_with(
        "autostart-resume",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_alpha),
            Step::Expect("_yi/seen", seen_ok),
        ],
        "wait-frame 5000 replayed world\n\
         quit\n",
        true,
    )
}

fn edit_push() -> Vec<Value> {
    vec![update(
        "s-alpha",
        json!({"sessionUpdate": "tool_call_update",
        "toolCallId": "e1", "title": "edit", "kind": "edit", "status": "completed",
        "rawOutput": {
            "patch": "--- a//tmp/demo-root/src/lib.rs\n+++ b//tmp/demo-root/src/lib.rs\n@@ -1,1 +1,2 @@\n old\n+brand new line\n",
            "added": 1, "removed": 0}}),
    )]
}

fn ipython_push() -> Vec<Value> {
    vec![update(
        "s-alpha",
        json!({"sessionUpdate": "tool_call_update",
        "toolCallId": "k1", "title": "ipython", "kind": "execute", "status": "in_progress",
        "rawInput": {"code": "print(1)"}}),
    )]
}

fn tracked_yes(frame: &Value) -> Vec<Value> {
    vec![ok(frame, json!({"tracked": [true]}))]
}

fn tracked_no(frame: &Value) -> Vec<Value> {
    vec![ok(frame, json!({"tracked": [false]}))]
}

/// The first edit to a tracked file opens the session's diff pane beside it, the transcript
/// shows the patch inline, and focus stays on the chat pane.
#[test]
fn edit_to_a_tracked_file_opens_the_diff_pane() -> TestResult {
    run(
        "diff-auto",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_alpha),
            Step::Expect("_yi/seen", seen_ok),
            Step::Push(edit_push),
            Step::Expect("_yi/tracked", tracked_yes),
        ],
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         wait-frame 5000 Δ s-alpha · 1 file · +1 −0\n\
         wait-frame 3000 brand new line\n\
         type still typing here\n\
         wait-frame 3000 still typing here\n\
         quit\n",
    )
}

/// An edit outside git accumulates but opens nothing; ⌘G shows it on demand.
#[test]
fn untracked_edit_accumulates_without_opening() -> TestResult {
    run(
        "diff-untracked",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_alpha),
            Step::Expect("_yi/seen", seen_ok),
            Step::Push(edit_push),
            Step::Expect("_yi/tracked", tracked_no),
        ],
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         wait-frame 3000 brand new line\n\
         wait-frame 2000 !Δ s-alpha\n\
         cmd-g\n\
         wait-frame 3000 Δ s-alpha · 1 file · +1 −0\n\
         cmd-g\n\
         wait-frame 3000 !Δ s-alpha\n\
         quit\n",
    )
}

/// The first kernel cell opens the notebook pane, and that spends the session's one
/// automatic side pane: a later tracked edit no longer opens the diff.
#[test]
fn first_kernel_cell_opens_the_notebook_and_spends_the_auto_side() -> TestResult {
    run(
        "side-once",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_alpha),
            Step::Expect("_yi/seen", seen_ok),
            Step::Push(ipython_push),
            Step::Push(edit_push),
            Step::Expect("_yi/tracked", tracked_yes),
        ],
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         wait-frame 5000 nb:s-alpha\n\
         wait-frame 5000 brand new line\n\
         wait-frame 2000 !Δ s-alpha\n\
         quit\n",
    )
}

fn other_root_ledger(frame: &Value) -> Vec<Value> {
    vec![ok(
        frame,
        json!({"sessions": [
            {"sessionId": "s-gamma", "cwd": "/tmp/other-root", "attached": false,
             "unseen": 0, "lastState": "idle", "lastEventMs": 1},
        ]}),
    )]
}

fn subagent_push() -> Vec<Value> {
    vec![update(
        "s-alpha",
        json!({"sessionUpdate": "_yi/subagent_update", "id": "c1",
        "name": "grep-bot-sub-1a2b3c4d", "status": "running", "activity": "executing",
        "toolUseCount": 2, "tokenCount": 100}),
    )]
}

/// The workspaces block lists every root; ←/→ in the sidebar narrows the session list
/// to one root and back.
#[test]
fn workspace_rows_filter_sessions_by_root() -> TestResult {
    run(
        "roots",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", other_root_ledger),
        ],
        "wait-frame 5000 s-gamma\n\
         wait-frame 3000 workspaces\n\
         wait-frame 3000 other-root 1\n\
         key right\n\
         wait-frame 3000 !s-gamma\n\
         wait-frame 3000 s-alpha\n\
         key right\n\
         wait-frame 3000 s-gamma\n\
         wait-frame 3000 !s-alpha\n\
         key left\n\
         key left\n\
         wait-frame 3000 s-alpha\n\
         quit\n",
    )
}

/// A child reported over `_yi/subagent_update` shows under its parent in the sidebar.
#[test]
fn subagent_rows_render_under_parent() -> TestResult {
    run(
        "children",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_alpha),
            Step::Expect("_yi/seen", seen_ok),
            Step::Push(subagent_push),
        ],
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         wait-frame 5000 └ grep-bot-sub-1a2 ◐\n\
         quit\n",
    )
}

fn kernel_execute_reply(frame: &Value) -> Vec<Value> {
    vec![
        ok(frame, json!({"callId": "user-1"})),
        update(
            "s-alpha",
            json!({"sessionUpdate": "tool_call_update",
            "toolCallId": "user-1", "title": "ipython", "kind": "execute",
            "status": "in_progress", "rawInput": {"code": "print(1)"}}),
        ),
        update(
            "s-alpha",
            json!({"sessionUpdate": "tool_call_update",
            "toolCallId": "user-1", "status": "completed",
            "rawOutput": {"stdout": "1\n", "result": "", "error": null}}),
        ),
    ]
}

fn kernel_execute_running(frame: &Value) -> Vec<Value> {
    vec![
        ok(frame, json!({"callId": "user-1"})),
        update(
            "s-alpha",
            json!({"sessionUpdate": "tool_call_update",
            "toolCallId": "user-1", "title": "ipython", "kind": "execute",
            "status": "in_progress", "rawInput": {"code": "sleep()"}}),
        ),
    ]
}

/// ⇧↩ on the notebook pane runs the draft on the session's kernel, and the cell comes back
/// through the same tool-call stream the agent's cells use.
#[test]
fn shift_enter_runs_user_cell_on_session_kernel() -> TestResult {
    run(
        "user-cell",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_alpha),
            Step::Expect("_yi/seen", seen_ok),
            Step::Expect("_yi/kernel_execute", kernel_execute_reply),
        ],
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         cmd-j\n\
         wait-frame 3000 nb:s-alpha\n\
         wait-frame 3000 ⇧↩ runs\n\
         type print(1)\n\
         key shift-enter\n\
         wait-frame 5000 ● In[1]\n\
         wait-frame 3000 print(1)\n\
         quit\n",
    )
}

/// Esc on the notebook cancels the newest running user cell.
#[test]
fn esc_cancels_running_user_cell() -> TestResult {
    run(
        "user-cell-cancel",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_alpha),
            Step::Expect("_yi/seen", seen_ok),
            Step::Expect("_yi/kernel_execute", kernel_execute_running),
            Step::Expect("_yi/kernel_cancel", seen_ok),
        ],
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         cmd-j\n\
         wait-frame 3000 nb:s-alpha\n\
         type sleep()\n\
         key shift-enter\n\
         wait-frame 5000 ◐ In[1]\n\
         key esc\n\
         wait 300\n\
         quit\n",
    )
}

fn editor_file(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "yi-console-editor-{}-{name}.rs",
        std::process::id()
    ))
}

fn seed_editor_file(name: &str, body: &str) -> Result<PathBuf, Box<dyn Error>> {
    let path = editor_file(name);
    std::fs::write(&path, body)?;
    Ok(path)
}

fn open_editor_script(path: &std::path::Path, rest: &str) -> String {
    format!(
        "wait-frame 5000 s-alpha\nkey enter\nwait-frame 5000 replayed world\n\
         key alt-/\ntype e {}\nkey enter\nwait-frame 3000 ✎ \n{rest}",
        path.display()
    )
}

fn session_fixture() -> Vec<Step> {
    vec![
        Step::Expect("initialize", init_reply),
        Step::Expect("session/list", two_session_list),
        Step::Expect("session/list", empty_list),
        Step::Expect("session/resume", resume_alpha),
        Step::Expect("_yi/seen", seen_ok),
    ]
}

/// `e <path>` opens a file in the pane; typing marks it dirty and ⌘S writes it back.
#[test]
fn editor_opens_types_and_saves() -> TestResult {
    let path = seed_editor_file("save", "fn main() {}\n")?;
    run(
        "editor-save",
        session_fixture(),
        &open_editor_script(
            &path,
            "wait-frame 3000 fn main\nkey end\ntype  // done\nwait-frame 3000 .rs ●\n\
             cmd-s\nwait-frame 3000 saved\nwait-frame 3000 !.rs ●\nquit\n",
        ),
    )?;
    assert_eq!(std::fs::read_to_string(&path)?, "fn main() {} // done\n");
    let _ = std::fs::remove_file(&path);
    Ok(())
}

/// A click places the cursor on that cell; a drag selects, and backspace removes the run.
#[test]
fn editor_click_places_cursor_and_drag_selects() -> TestResult {
    let path = seed_editor_file("mouse", "abcdef\nsecond\n")?;
    // Sidebar 26 wide, border at x=26, inner x=27, gutter "1 " puts text at x=29; row 0 at y=1.
    run(
        "editor-mouse",
        session_fixture(),
        &open_editor_script(
            &path,
            "wait-frame 3000 abcdef\nmouse down 31 1\nmouse up 31 1\ntype X\n\
             wait-frame 3000 abXcdef\nmouse down 29 1\nmouse drag 31 1\nmouse up 31 1\n\
             key backspace\nwait-frame 3000 Xcdef\nwait-frame 3000 !abXcdef\nquit\n",
        ),
    )?;
    let _ = std::fs::remove_file(&path);
    Ok(())
}

fn agent_rewrites_reload_file() -> Vec<Value> {
    let path = editor_file("reload");
    let _write = std::fs::write(&path, "rewritten by the agent\n");
    vec![update(
        "s-alpha",
        json!({"sessionUpdate": "tool_call_update",
        "toolCallId": "e9", "title": "edit", "kind": "edit", "status": "completed",
        "rawOutput": {"patch": format!("--- a/{0}\n+++ b/{0}\n@@ -1,1 +1,1 @@\n-old\n+rewritten by the agent\n", path.display()),
                      "added": 1, "removed": 1}}),
    )]
}

/// A clean editor follows the agent's edit to the same file without asking.
#[test]
fn agent_edit_reloads_clean_editor_silently() -> TestResult {
    let path = seed_editor_file("reload", "old\n")?;
    // The daemon list poll (every 5 s) is the sequencing point: the agent's rewrite lands
    // after the editor has read the original.
    let mut fixture = session_fixture();
    fixture.push(Step::Expect("session/list", empty_list));
    fixture.push(Step::Push(agent_rewrites_reload_file));
    fixture.push(Step::Expect("_yi/tracked", tracked_no));
    run(
        "editor-reload",
        fixture,
        &open_editor_script(
            &path,
            "wait-frame 3000 1 old\nwait-frame 9000 rewritten by the agent\n\
             wait-frame 2000 !file changed on disk\nquit\n",
        ),
    )?;
    let _ = std::fs::remove_file(&path);
    Ok(())
}

fn agent_rewrites_dirty_file() -> Vec<Value> {
    let path = editor_file("dirty");
    let _write = std::fs::write(&path, "rewritten underneath\n");
    vec![update(
        "s-alpha",
        json!({"sessionUpdate": "tool_call_update",
        "toolCallId": "e10", "title": "edit", "kind": "edit", "status": "completed",
        "rawOutput": {"patch": format!("--- a/{0}\n+++ b/{0}\n@@ -1,1 +1,1 @@\n-old\n+rewritten underneath\n", path.display()),
                      "added": 1, "removed": 1}}),
    )]
}

/// A dirty editor shows the stale bar instead of losing the draft; `r` takes the disk copy.
#[test]
fn dirty_editor_shows_reload_bar_and_r_reloads() -> TestResult {
    let path = seed_editor_file("dirty", "old\n")?;
    let mut fixture = session_fixture();
    fixture.push(Step::Expect("session/list", empty_list));
    fixture.push(Step::Push(agent_rewrites_dirty_file));
    fixture.push(Step::Expect("_yi/tracked", tracked_no));
    run(
        "editor-dirty",
        fixture,
        &open_editor_script(
            &path,
            "wait-frame 3000 1 old\nkey end\ntype er draft\nwait-frame 3000 older draft\n\
             wait-frame 9000 file changed on disk\nkey r\n\
             wait-frame 3000 rewritten underneath\nwait-frame 3000 !file changed on disk\nquit\n",
        ),
    )?;
    let _ = std::fs::remove_file(&path);
    Ok(())
}

/// ⌘F then `/needle` scrolls the editor to the match.
#[test]
fn cmd_f_scrolls_to_match() -> TestResult {
    let body: String = (1..=60)
        .map(|n| {
            if n == 55 {
                "let needle = 1;\n".to_owned()
            } else {
                format!("line {n}\n")
            }
        })
        .collect();
    let path = seed_editor_file("find", &body)?;
    run(
        "editor-find",
        session_fixture(),
        &open_editor_script(
            &path,
            "wait-frame 3000 line 1\nwait-frame 2000 !needle\ncmd-f\ntype needle\nkey enter\n\
             wait-frame 3000 let needle = 1;\nquit\n",
        ),
    )?;
    let _ = std::fs::remove_file(&path);
    Ok(())
}
