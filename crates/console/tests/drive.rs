//! Drive-mode tests: the real console loop against an in-process fixture
//! daemon speaking ordered, scripted ACP over a scratch unix socket.

use std::error::Error;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

use serde_json::{Value, json};
use yi_console::{ConsoleOptions, DriveOptions, parse_script, run_headless};

type TestResult = Result<(), Box<dyn Error>>;
type Responder = fn(&Value) -> Vec<Value>;

enum Fx {
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

fn fixture_loop(listener: &UnixListener, script: Vec<Fx>) -> Result<(), String> {
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
            Fx::Expect(method, respond) => loop {
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
            Fx::Push(frames) => {
                for frame in frames() {
                    write_frame(&mut stream, &frame)?;
                }
            }
            Fx::Close => {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
            Fx::Accept => {
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

/// Run the fixture script on its own thread; its Result surfaces through the
/// join in `run` so a protocol mismatch fails the test with evidence.
fn spawn_fixture(socket: PathBuf, script: Vec<Fx>) -> std::thread::JoinHandle<Result<(), String>> {
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

fn run(name: &str, fixture: Vec<Fx>, script: &str) -> TestResult {
    let socket = scratch_socket(name);
    let server = spawn_fixture(socket.clone(), fixture);
    let steps = parse_script(script)?;
    let code = run_headless(
        &ConsoleOptions {
            socket,
            root: "/tmp/demo-root".to_owned(),
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
            Fx::Expect("initialize", init_reply),
            Fx::Expect("session/list", two_session_list),
            Fx::Expect("session/list", ledger_list),
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
            Fx::Expect("initialize", init_reply),
            Fx::Expect("session/list", two_session_list),
            Fx::Expect("session/list", empty_list),
            Fx::Expect("session/resume", resume_alpha),
            Fx::Expect("_yi/seen", seen_ok),
            Fx::Expect("session/prompt", prompt_stream),
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
            Fx::Expect("initialize", init_reply),
            Fx::Expect("session/list", two_session_list),
            Fx::Expect("session/list", empty_list),
            Fx::Expect("session/resume", resume_alpha),
            Fx::Expect("_yi/seen", seen_ok),
            Fx::Close,
            Fx::Accept,
            Fx::Expect("initialize", init_reply),
            // The second replay carries different text: seeing it WITHOUT the
            // first replay's text proves the wipe-and-replay contract.
            Fx::Expect("session/resume", resume_alpha_again),
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
            Fx::Expect("initialize", init_reply),
            Fx::Expect("session/list", two_session_list),
            Fx::Expect("session/list", empty_list),
            Fx::Expect("session/resume", resume_alpha),
            Fx::Expect("_yi/seen", seen_ok),
            Fx::Push(permission_request),
            Fx::Expect("<response>", permission_answer),
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
            Fx::Expect("initialize", init_reply),
            Fx::Expect("session/list", two_session_list),
            Fx::Expect("session/list", empty_list),
            Fx::Expect("session/resume", resume_alpha),
            Fx::Expect("_yi/seen", seen_ok),
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
            Fx::Expect("initialize", init_reply),
            Fx::Expect("session/list", two_session_list),
            Fx::Expect("session/list", empty_list),
            Fx::Expect("session/resume", |frame| {
                vec![
                    update(
                        "s-beta",
                        json!({"sessionUpdate": "agent_message", "messageId": "msg_1",
                        "content": [{"type": "text", "text": "beta transcript"}]}),
                    ),
                    ok(frame, json!({"sessionId": "s-beta", "configOptions": []})),
                ]
            }),
            Fx::Expect("_yi/seen", seen_ok),
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
            Fx::Expect("initialize", init_reply),
            Fx::Expect("session/list", two_session_list),
            Fx::Expect("session/list", empty_list),
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
            Fx::Expect("initialize", init_reply),
            Fx::Expect("session/list", two_session_list),
            Fx::Expect("session/list", empty_list),
            Fx::Expect("session/resume", resume_alpha),
            Fx::Expect("_yi/seen", seen_ok),
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
            Fx::Expect("initialize", init_reply),
            Fx::Expect("session/list", two_session_list),
            Fx::Expect("session/list", empty_list),
            Fx::Expect("session/resume", resume_alpha),
            Fx::Expect("_yi/seen", seen_ok),
            Fx::Expect("session/resume", |frame| {
                vec![ok(
                    frame,
                    json!({"sessionId": "s-beta", "configOptions": []}),
                )]
            }),
            Fx::Expect("_yi/seen", |frame| {
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
            Fx::Expect("initialize", init_reply),
            Fx::Expect("session/list", two_session_list),
            Fx::Expect("session/list", empty_list),
            Fx::Expect("session/resume", resume_alpha),
            Fx::Expect("_yi/seen", seen_ok),
            Fx::Expect("session/prompt", |frame| {
                let mut frames = vec![ok(frame, json!({}))];
                frames.extend(ipython_updates());
                frames
            }),
        ],
        // Open the session, swap the pane to the notebook view, then prompt:
        // the kernel cell renders code, streams and the image placeholder.
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         key alt-/\n\
         type nb\n\
         key enter\n\
         wait-frame 3000 no kernel cells yet\n\
         type chart it\n\
         key enter\n\
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
            Fx::Expect("initialize", init_reply),
            Fx::Expect("session/list", two_session_list),
            Fx::Expect("session/list", empty_list),
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
            Fx::Expect("initialize", init_reply),
            Fx::Expect("session/list", two_session_list),
            Fx::Expect("session/list", empty_list),
            Fx::Expect("session/resume", resume_with_offset),
            Fx::Expect("_yi/seen", seen_ok),
            Fx::Close,
            Fx::Accept,
            Fx::Expect("initialize", init_reply),
            Fx::Expect("session/resume", |frame| {
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
            Fx::Expect("initialize", init_reply),
            Fx::Expect("session/list", two_session_list),
            Fx::Expect("session/list", empty_list),
            Fx::Expect("session/resume", resume_with_offset),
            Fx::Expect("_yi/seen", |frame| {
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
            Fx::Close,
            Fx::Accept,
            Fx::Expect("initialize", init_reply),
            Fx::Expect("session/resume", |frame| {
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
