//! Drive-mode tests: the real console loop against an in-process fixture
//! daemon speaking ordered, scripted ACP over a scratch unix socket.

use std::error::Error;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::thread::JoinHandle;

use serde_json::{Value, json};
use yi_console::model::SidebarMode;
use yi_console::{ConsoleOptions, DriveOptions, parse_script, run_headless};
use yi_types::event::{AgentEvent, AssistantMessageEvent};
use yi_types::message::{AgentMessage, Content, StopReason, Usage, UserContent};

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

fn assistant(text: &str) -> AgentMessage {
    AgentMessage::Assistant {
        content: vec![Content::Text {
            text: text.to_owned(),
            text_signature: None,
        }],
        api: "faux".to_owned(),
        provider: "faux".to_owned(),
        model: "faux-1".to_owned(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::zero(),
        stop_reason: StopReason::Stop,
        raw_stop_reason: None,
        end_turn: None,
        deferred: None,
        error_message: None,
        timestamp: 0,
    }
}

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::host_user(UserContent::Text(text.to_owned()), 0)
}

fn entry(id: &str, parent: Option<&str>, seq: u64, message: &AgentMessage) -> Value {
    json!({"type": "message", "id": id, "message": message, "parentId": parent,
           "seq": seq, "timestamp": seq})
}

/// The branch verbatim, the way the worker sends it on a resume.
fn replay(session: &str, entries: &[Value], from: u64, name: Option<&str>) -> Value {
    let leaf = entries.last().and_then(|entry| entry["id"].as_str());
    update(
        session,
        json!({"sessionUpdate": "_yi/replay", "entries": entries, "from": from,
               "replayedTo": from + entries.len() as u64, "leafId": leaf, "name": name,
               "goal": null, "contextWindow": 128000}),
    )
}

fn event(session: &str, seq: u64, event: &AgentEvent) -> Value {
    update(
        session,
        json!({"sessionUpdate": "_yi/event", "event": event, "seq": seq}),
    )
}

/// An assistant message as the runtime streams it: start, one delta, end.
fn stream(session: &str, seq: u64, reply: &AgentMessage) -> Vec<Value> {
    let text = match reply {
        AgentMessage::Assistant { content, .. } => content
            .iter()
            .filter_map(|block| match block {
                Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>(),
        _ => String::new(),
    };
    vec![
        event(
            session,
            seq,
            &AgentEvent::MessageStart {
                message: assistant(""),
            },
        ),
        event(
            session,
            seq + 1,
            &AgentEvent::MessageUpdate {
                message: reply.clone(),
                assistant_message_event: AssistantMessageEvent::TextDelta {
                    content_index: 0,
                    delta: text,
                    partial: reply.clone(),
                },
            },
        ),
        event(
            session,
            seq + 2,
            &AgentEvent::MessageEnd {
                message: reply.clone(),
            },
        ),
    ]
}

/// One whole turn: running, the prompt, the reply, idle.
fn turn(session: &str, seq: u64, prompt: &str, reply: &AgentMessage) -> Vec<Value> {
    let mut frames = vec![
        update(
            session,
            json!({"sessionUpdate": "state_update", "state": "running"}),
        ),
        event(session, seq, &AgentEvent::AgentStart),
        event(
            session,
            seq + 1,
            &AgentEvent::MessageStart {
                message: user_message(prompt),
            },
        ),
    ];
    frames.extend(stream(session, seq + 2, reply));
    frames.push(event(
        session,
        seq + 5,
        &AgentEvent::AgentEnd {
            messages: Vec::new(),
        },
    ));
    frames.push(update(
        session,
        json!({"sessionUpdate": "state_update", "state": "idle"}),
    ));
    frames
}

fn session_result(frame: &Value, session: &str, name: Option<&str>, extra: Value) -> Value {
    let mut result = json!({
        "sessionId": session,
        "name": name,
        "configOptions": [
            {"configId": "model", "name": "Model",
             "kind": {"type": "select", "value": "faux/faux-1", "options": []}},
            {"configId": "thought_level", "name": "Thinking level",
             "kind": {"type": "select", "value": "medium", "options": []}},
        ],
    });
    if let (Some(map), Some(more)) = (result.as_object_mut(), extra.as_object()) {
        for (key, value) in more {
            map.insert(key.clone(), value.clone());
        }
    }
    ok(frame, result)
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
    run_sidebar(name, fixture, script, autostart, SidebarMode::Full)
}

/// Runs with a frame dump and hands back the last frame, for assertions a substring
/// cannot make.
fn run_frames(name: &str, fixture: Vec<Step>, script: &str) -> Result<String, Box<dyn Error>> {
    run_frames_with(name, fixture, script, SidebarMode::Full)
}

fn run_frames_with(
    name: &str,
    fixture: Vec<Step>,
    script: &str,
    sidebar: SidebarMode,
) -> Result<String, Box<dyn Error>> {
    let dir = std::env::temp_dir().join(format!("yi-console-frames-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    run_opts(name, fixture, script, false, sidebar, Some(dir.clone()))?;
    let mut names: Vec<PathBuf> = std::fs::read_dir(&dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    names.sort();
    let last = names.last().ok_or("no frames were dumped")?;
    Ok(std::fs::read_to_string(last)?)
}

/// The harness opens the sidebar in full so rows can be asserted by name; the
/// rail the CLI defaults to has its own test.
fn run_sidebar(
    name: &str,
    fixture: Vec<Step>,
    script: &str,
    autostart: bool,
    sidebar: SidebarMode,
) -> TestResult {
    run_opts(name, fixture, script, autostart, sidebar, None)
}

fn run_opts(
    name: &str,
    fixture: Vec<Step>,
    script: &str,
    autostart: bool,
    sidebar: SidebarMode,
    frames: Option<PathBuf>,
) -> TestResult {
    let socket = scratch_socket(name);
    let server = spawn_fixture(socket.clone(), fixture);
    let steps = parse_script(script)?;
    let code = run_headless(
        &ConsoleOptions {
            socket,
            root: "/tmp/demo-root".to_owned(),
            autostart,
            auto_side: true,
            sidebar,
        },
        DriveOptions {
            script: steps,
            frames_dir: frames
                .or_else(|| std::env::var("CONSOLE_TEST_FRAMES").ok().map(PathBuf::from)),
            record: None,
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
            {"sessionId": "s-alpha", "name": "s-alpha", "attached": false},
            {"sessionId": "s-beta", "name": "s-beta", "attached": false},
        ]}),
    )]
}

fn ledger_list(frame: &Value) -> Vec<Value> {
    vec![ok(
        frame,
        json!({"sessions": [
            {"sessionId": "s-beta", "name": "s-beta", "cwd": "/tmp/demo-root", "attached": false,
             "unseen": 2, "lastState": "idle", "lastEventMs": 1},
        ]}),
    )]
}

fn alpha_branch() -> Vec<Value> {
    vec![
        entry("e1", None, 1, &user_message("hello agent")),
        entry("e2", Some("e1"), 2, &assistant("replayed world")),
    ]
}

fn resume_alpha(frame: &Value) -> Vec<Value> {
    vec![
        replay("s-alpha", &alpha_branch(), 0, None),
        session_result(frame, "s-alpha", None, json!({"replayedTo": 2})),
    ]
}

fn resume_alpha_again(frame: &Value) -> Vec<Value> {
    vec![
        replay(
            "s-alpha",
            &[entry("e1", None, 1, &assistant("replayed again"))],
            0,
            None,
        ),
        session_result(frame, "s-alpha", None, json!({"replayedTo": 1})),
    ]
}

fn prompt_stream(frame: &Value) -> Vec<Value> {
    let prompt = frame
        .pointer("/params/prompt/0/text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let mut frames = vec![ok(frame, json!({}))];
    frames.extend(turn("s-alpha", 10, &prompt, &assistant("streamed answer")));
    frames
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
    let mut frames = vec![update(
        "s-alpha",
        json!({"sessionUpdate": "state_update", "state": "running"}),
    )];
    frames.extend(stream("s-alpha", 20, &assistant("approved, continuing")));
    frames.push(update(
        "s-alpha",
        json!({"sessionUpdate": "state_update", "state": "idle"}),
    ));
    frames
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
        "wait-frame 5000 !connecting…\n\
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
            sidebar: SidebarMode::Full,
        },
        DriveOptions {
            script: steps,
            frames_dir: None,
            record: None,
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
         key y\n\
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
         key c\n\
         wait-frame 3000  1 \n\
         key ctrl-b\n\
         key 1\n\
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
                    replay(
                        "s-beta",
                        &[entry("b1", None, 1, &assistant("beta transcript"))],
                        0,
                        None,
                    ),
                    session_result(frame, "s-beta", None, json!({"replayedTo": 1})),
                ]
            }),
            Step::Expect("_yi/seen", seen_ok),
        ],
        "wait-frame 5000 s-alpha\n\
         wait-frame 3000 !Command Palette\n\
         key alt-/\n\
         wait-frame 3000 ›\n\
         wait-frame 3000 Command Palette\n\
         type beta\n\
         wait-frame 3000 › beta\n\
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
        "wait-frame 5000 !connecting…\n\
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
            sidebar: SidebarMode::Full,
        },
        DriveOptions {
            script: steps,
            frames_dir: None,
            record: None,
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
         mouse down 2 1\n\
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
    run_sidebar(
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
                    json!({"sessionId": "s-beta", "name": "s-beta", "configOptions": []}),
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
        "wait-frame 5000 1 SA\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         key alt-v\n\
         key tab\n\
         key down\n\
         key enter\n\
         wait-frame 5000 done while you were away\n\
         quit\n",
        false,
        SidebarMode::Rail,
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
         wait-frame 5000 In [1]\n\
         wait-frame 3000 plot_drift\n\
         wait-frame 3000 computing drift\n\
         wait-frame 3000 image · 0 KB png\n\
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
                    frames.push(replay(
                        "s-alpha",
                        &[entry("e3", Some("e2"), 3, &assistant("offset honored"))],
                        2,
                        None,
                    ));
                }
                frames.push(session_result(
                    frame,
                    "s-alpha",
                    None,
                    json!({"replayedTo": 3}),
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
                // A live update past the offset makes it stale.
                let mut frames = vec![ok(frame, json!({}))];
                frames.extend(stream("s-alpha", 40, &assistant("streamed since")));
                frames
            }),
            Step::Close,
            Step::Accept,
            Step::Expect("initialize", init_reply),
            Step::Expect("session/resume", |frame| {
                let from = frame.pointer("/params/replayFrom").and_then(Value::as_u64);
                let mut frames = Vec::new();
                if from == Some(0) {
                    frames.push(replay(
                        "s-alpha",
                        &[entry("e1", None, 1, &assistant("full replay again"))],
                        0,
                        None,
                    ));
                }
                frames.push(session_result(
                    frame,
                    "s-alpha",
                    None,
                    json!({"replayedTo": 4}),
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
         key alt-/\n\
         wait-frame 3000 ⌥n new\n\
         key esc\n\
         cmd-d\n\
         wait-frame 3000 no session\n\
         cmd-p\n\
         wait-frame 3000 ⌘⇧N new\n\
         key esc\n\
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
        json!({"sessionId": "s-new", "name": "s-new", "configOptions": []}),
    )]
}

/// Opening yi is not resuming: with sessions listed and nothing bound, the first pane still
/// gets a fresh session; a resume is a sidebar pick. Seen with a ledger whose newest row was a
/// deleted worktree's session, which resumed into "unknown session".
#[test]
fn workspace_autostarts_a_new_session_even_when_sessions_are_listed() -> TestResult {
    run_with(
        "auto-new-rows",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", named_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/new", new_session_reply),
        ],
        "wait-frame 5000 s-new\n\
         quit\n",
        true,
    )
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
         quit\n",
        true,
    )
}

/// An edit as the worker reports it: the runtime event the chat renders, then the
/// standard tool update the diff pane reads.
fn edit_push() -> Vec<Value> {
    let patch = "--- a//tmp/demo-root/src/lib.rs\n+++ b//tmp/demo-root/src/lib.rs\n@@ -1,1 +1,2 @@\n old\n+brand new line\n";
    let result = yi_types::event::ToolResult {
        content: vec![Content::Text {
            text: "[lib.rs#1]\nupdated; first change at line 2".to_owned(),
            text_signature: None,
        }],
        details: json!({"path": "/tmp/demo-root/src/lib.rs", "patch": patch}),
        usage: None,
        added_tool_names: None,
        terminate: None,
    };
    vec![
        event(
            "s-alpha",
            20,
            &AgentEvent::ToolExecutionStart {
                tool_call_id: "e1".to_owned(),
                tool_name: "edit".to_owned(),
                args: json!({"path": "/tmp/demo-root/src/lib.rs"}),
            },
        ),
        event(
            "s-alpha",
            21,
            &AgentEvent::ToolExecutionEnd {
                tool_call_id: "e1".to_owned(),
                tool_name: "edit".to_owned(),
                result,
                is_error: false,
            },
        ),
        update(
            "s-alpha",
            json!({"sessionUpdate": "tool_call_update",
            "toolCallId": "e1", "title": "edit", "kind": "edit", "status": "completed",
            "rawOutput": {"patch": patch, "added": 1, "removed": 0}}),
        ),
    ]
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

/// The tracked round trip is the sequencing point for the editor-reload tests: the console
/// asks only once it has absorbed the agent's edit, so this answer cannot precede that.
fn tracked_no_then_answer(frame: &Value) -> Vec<Value> {
    let mut frames = tracked_no(frame);
    frames.extend(turn(
        "s-alpha",
        30,
        "check the reload",
        &assistant("the edit landed"),
    ));
    frames
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
            {"sessionId": "s-gamma", "name": "s-gamma", "cwd": "/tmp/other-root", "attached": false,
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
         wait-frame 5000 └ GR grep-bot-sub-1a2b ◐\n\
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
         wait-frame 5000 ● In [1]\n\
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
         wait-frame 5000 ◐ In [1]\n\
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
    editor_script(path, "", rest)
}

/// Splits first so the chat pane survives beside the editor and the transcript still shows
/// what the agent did.
fn split_editor_script(path: &std::path::Path, rest: &str) -> String {
    editor_script(path, "key alt-v\n", rest)
}

fn editor_script(path: &std::path::Path, split: &str, rest: &str) -> String {
    format!(
        "wait-frame 5000 s-alpha\nkey enter\nwait-frame 5000 replayed world\n\
         {split}key alt-/\ntype e {}\nkey enter\nwait-frame 3000 ✎ \n{rest}",
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
    // Sidebar 29 wide, border at x=29, inner x=30, gutter "1 " puts text at x=32; row 0 at y=1.
    run(
        "editor-mouse",
        session_fixture(),
        &open_editor_script(
            &path,
            "wait-frame 3000 abcdef\nmouse down 34 1\nmouse up 34 1\ntype X\n\
             wait-frame 3000 abXcdef\nmouse down 32 1\nmouse drag 34 1\nmouse up 34 1\n\
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
    // The daemon list poll (every 5 s) holds the rewrite back until the editor has read the
    // original; the answer pushed after `_yi/tracked` is what says the console absorbed it.
    let mut fixture = session_fixture();
    fixture.push(Step::Expect("session/list", empty_list));
    fixture.push(Step::Push(agent_rewrites_reload_file));
    fixture.push(Step::Expect("_yi/tracked", tracked_no_then_answer));
    run(
        "editor-reload",
        fixture,
        &split_editor_script(
            &path,
            "wait-frame 3000 1 old\nwait-frame 9000 1 rewritten by the agent\n\
             wait-frame 9000 the edit landed\n\
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
    fixture.push(Step::Expect("_yi/tracked", tracked_no_then_answer));
    run(
        "editor-dirty",
        fixture,
        &split_editor_script(
            &path,
            "wait-frame 3000 1 old\nkey end\ntype er draft\nwait-frame 3000 1 older draft\n\
             wait-frame 9000 the edit landed\n\
             wait-frame 3000 file changed on disk\nkey r\n\
             wait-frame 3000 1 rewritten underneath\n\
             wait-frame 3000 !file changed on disk\nquit\n",
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

/// Once a second pane exists the corners are rounded and the focused pane wears a reversed
/// chip; nothing sits between the transcript and the composer (M10).
#[test]
fn rounded_borders_and_context_strip_render() -> TestResult {
    run(
        "polish",
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
         key alt-v\n\
         key alt-left\n\
         wait-frame 3000 ╭\n\
         wait-frame 3000 ❯ s-alpha\n\
         wait-frame 3000 !touched\n\
         quit\n",
    )
}

fn long_rust_file(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    // A block comment opens on line 1 and closes on line 70, so every row a scrolled
    // viewport shows sits inside a construct that began above it.
    let mut body = String::from("/* the header comment this file opens with\n");
    for n in 2..=69 {
        body.push_str(&format!("   still inside the comment, line {n}\n"));
    }
    body.push_str("*/\nfn after() -> u32 { 7 }\n");
    let path = editor_file(name);
    std::fs::write(&path, body)?;
    Ok(path)
}

/// A file taller than its pane wears a scroll bar, and the wheel moves the viewport
/// without the cursor dragging it back.
#[test]
fn the_editor_shows_a_scroll_bar_and_the_wheel_moves_it() -> TestResult {
    let path = long_rust_file("scroll")?;
    let mut script = String::from("wait-frame 3000 the header comment\nwait-frame 3000 \u{2503}\n");
    for _ in 0..12 {
        script.push_str("mouse scrolldown 60 8\n");
    }
    script.push_str(
        "wait-frame 3000 line 40\nwait-frame 3000 !the header comment\n\
         wait-frame 3000 \u{2503}\nquit\n",
    );
    run(
        "editor-scroll",
        session_fixture(),
        &open_editor_script(&path, &script),
    )?;
    let _ = std::fs::remove_file(&path);
    Ok(())
}

/// A lone chat pane is the chat itself and wears no frame or title; the frame arrives with
/// the second pane, whose edge it separates, and leaves with it.
#[test]
fn a_lone_chat_pane_wears_no_frame() -> TestResult {
    run(
        "lone-pane",
        session_fixture(),
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         wait-frame 3000 !❯ s-alpha\n\
         wait-frame 3000 !┬\n\
         key alt-v\n\
         wait-frame 3000 ❯ s-alpha\n\
         wait-frame 3000 ┬\n\
         key alt-x\n\
         wait-frame 3000 !❯ s-alpha\n\
         wait-frame 3000 replayed world\n\
         quit\n",
    )
}

/// ctrl+c is the solo vocabulary: a drafted prompt clears, an idle composer warns, and a
/// second press inside the window asks the daemon to stop and quits the console.
#[test]
fn ctrl_c_clears_the_draft_then_warns_then_stops_the_daemon() -> TestResult {
    let mut fixture = session_fixture();
    fixture.push(Step::Expect("_yi/shutdown", seen_ok));
    run(
        "ctrl-c",
        fixture,
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         type draft text\n\
         wait-frame 3000 draft text\n\
         key ctrl-c\n\
         wait-frame 3000 !draft text\n\
         key ctrl-c\n\
         wait-frame 3000 stops the daemon\n\
         key ctrl-c\n\
         wait-frame 3000 the console quit before this frame\n",
    )
}

/// ⌥q leaves: the console exits and no `_yi/shutdown` reaches the daemon.
#[test]
fn alt_q_detaches_and_leaves_the_daemon_running() -> TestResult {
    run(
        "detach",
        session_fixture(),
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         key alt-q\n\
         wait-frame 3000 the console quit before this frame\n",
    )
}

#[expect(
    clippy::disallowed_methods,
    reason = "the fixture dates its sessions against the clock the ages are read from"
)]
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

fn named_list(frame: &Value) -> Vec<Value> {
    let now = now_ms();
    vec![ok(
        frame,
        json!({"sessions": [
            {"sessionId": "s-alpha", "attached": false, "name": "fix login bug",
             "createdAt": now - 3 * 3_600_000},
            {"sessionId": "s-beta", "attached": false, "name": "release notes",
             "createdAt": now - 5 * 60_000},
        ]}),
    )]
}

/// Echoes the resumed id into the transcript, so a frame says which row Enter opened.
fn resume_named(frame: &Value) -> Vec<Value> {
    let id = frame
        .pointer("/params/sessionId")
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_owned();
    vec![
        replay(
            &id,
            &[entry("e1", None, 1, &assistant(&format!("resumed {id}")))],
            0,
            None,
        ),
        session_result(frame, &id, None, json!({"replayedTo": 1})),
    ]
}

fn two_root_ledger(frame: &Value) -> Vec<Value> {
    vec![ok(
        frame,
        json!({"sessions": [
            {"sessionId": "s-gamma", "name": "s-gamma", "cwd": "/tmp/other-root", "attached": false,
             "unseen": 0, "lastState": "idle", "lastEventMs": now_ms()},
        ]}),
    )]
}

/// Rows sit under their workspace, the console's own first, newest first inside each:
/// the newest session of all is slot 3 because it belongs to the other root.
#[test]
fn the_sidebar_groups_rows_by_workspace() -> TestResult {
    run(
        "root-groups",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", named_list),
            Step::Expect("session/list", two_root_ledger),
        ],
        "wait-frame 5000 1 RE release notes\n\
         wait-frame 3000 2 FI fix login bug\n\
         wait-frame 3000 3 SG s-gamma\n\
         quit\n",
    )
}

/// The resumed session's worker reports a turn starting, the way a prompt does.
fn resume_then_run(frame: &Value) -> Vec<Value> {
    let id = frame
        .pointer("/params/sessionId")
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_owned();
    let mut frames = resume_named(frame);
    frames.push(update(
        &id,
        json!({"sessionUpdate": "state_update", "state": "running"}),
    ));
    frames
}

/// A session that starts a turn moves to slot 1 the moment its state changes, not at
/// the next list: the older row was slot 2 until its worker said `running`.
#[test]
fn a_state_transition_moves_the_row_to_the_top() -> TestResult {
    run(
        "transition-reorders",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", named_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_then_run),
            Step::Expect("_yi/seen", seen_ok),
        ],
        "wait-frame 5000 1 RE release notes\n\
         wait-frame 3000 2 FI fix login bug\n\
         key down\n\
         key enter\n\
         wait-frame 5000 resumed s-alpha\n\
         wait-frame 3000 1 FI fix login bug\n\
         wait-frame 3000 2 RE release notes\n\
         quit\n",
    )
}

/// The rail lists every session, windowed to the viewport like the full sidebar: the
/// thirteenth session is a row, not a session the rail has no slot for.
#[test]
fn the_rail_stops_at_its_numbered_slots() -> TestResult {
    use yi_console::model::{SessionId, SessionRow, SessionStatus};
    let theme = yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::Ansi16, true);
    let mut app = yi_console::app::App::new("/tmp/demo-root".to_owned(), theme);
    app.state.sidebar = SidebarMode::Rail;
    for n in 0..14_u64 {
        app.state.upsert_row(SessionRow {
            id: SessionId(format!("s-{n:02}")),
            root: "/tmp/demo-root".to_owned(),
            status: SessionStatus::Idle,
            attached: false,
            name: None,
            created_ms: 1000 + n,
            last_ms: 0,
        });
    }
    let rows = yi_console::sidebar::sidebar_lines(&app, &theme, 60);
    let mut listed: Vec<usize> = rows.iter().filter_map(|row| row.index).collect();
    listed.dedup();
    assert_eq!(
        listed.len(),
        yi_console::sidebar::RAIL_CAP,
        "the rail holds its numbered slots: {listed:?}"
    );
    let tail = rows
        .last()
        .map(|row| row.line.to_string())
        .unwrap_or_default();
    assert_eq!(tail.trim(), "+5", "the rest are counted, not listed");
    Ok(())
}

/// Rows carry the session's name and age, newest first, and the row behind the
/// focused pane wears the focus bar once it opens.
#[test]
fn sidebar_rows_show_names_and_ages_newest_first() -> TestResult {
    run(
        "named-rows",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", named_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_named),
            Step::Expect("_yi/seen", seen_ok),
        ],
        "wait-frame 5000 1 RE release notes\n\
         wait-frame 3000 2 FI fix login bug\n\
         wait-frame 3000 this hour\n\
         wait-frame 3000 today\n\
         key down\n\
         key up\n\
         key enter\n\
         wait-frame 5000 resumed s-beta\n\
         wait-frame 3000 RE · release\n\
         quit\n",
    )
}

fn named_after_prompt(frame: &Value) -> Vec<Value> {
    vec![ok(
        frame,
        json!({"sessions": [
            {"sessionId": "s-alpha", "cwd": "/tmp/demo-root", "attached": true,
             "unseen": 0, "lastState": "idle", "lastEventMs": now_ms(), "name": "fix login"},
        ]}),
    )]
}

/// The first prompt names the session: the console re-lists as soon as the prompt is
/// accepted, and the ledger's name lands on the row the pane is showing.
#[test]
fn the_first_prompt_names_the_session_row() -> TestResult {
    let mut fixture = session_fixture();
    fixture.push(Step::Expect("session/prompt", prompt_stream));
    fixture.push(Step::Expect("session/list", named_after_prompt));
    run(
        "prompt-names",
        fixture,
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         type fix login\n\
         key enter\n\
         wait-frame 5000 fix login\n\
         wait-frame 3000 !s-alpha\n\
         quit\n",
    )
}

/// The CLI opens the sidebar as a rail of status glyphs; ⌘B walks rail, full, hidden.
#[test]
fn the_rail_is_the_default_and_cmd_b_walks_to_full_and_back() -> TestResult {
    run_sidebar(
        "rail",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", ledger_list),
        ],
        "wait-frame 5000 ●\n\
         wait-frame 3000 1 SB   ●│\n\
         wait-frame 3000 2 SA\n\
         wait-frame 3000 !s-alpha\n\
         wait-frame 3000 !workspaces\n\
         cmd-b\n\
         wait-frame 3000 2 SA s-alpha\n\
         wait-frame 3000 workspaces\n\
         cmd-b\n\
         wait-frame 3000 1 SB   ●│\n\
         wait-frame 3000 !s-alpha\n\
         quit\n",
        false,
        SidebarMode::Rail,
    )
}

fn forty_session_list(frame: &Value) -> Vec<Value> {
    let sessions: Vec<Value> = (0..40)
        .map(|n| {
            json!({"sessionId": format!("s-{n:02}"), "name": format!("s-{n:02}"),
                       "attached": false, "createdAt": 1000 + n})
        })
        .collect();
    vec![ok(frame, json!({"sessions": sessions}))]
}

/// More rows than the sidebar has lines: the window starts at the newest and follows
/// the cursor down, so nothing is unreachable and nothing overflows the column.
#[test]
fn the_sidebar_windows_to_the_viewport_and_follows_the_cursor() -> TestResult {
    let mut script = String::from("wait-frame 5000 s-39\nwait-frame 3000 !s-05\n");
    for _ in 0..34 {
        script.push_str("key down\n");
    }
    script.push_str("wait-frame 3000 s-05\nwait-frame 3000 !s-39\nquit\n");
    run(
        "forty-rows",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", forty_session_list),
            Step::Expect("session/list", empty_list),
        ],
        &script,
    )
}

fn slash_reply(frame: &Value) -> Vec<Value> {
    let line = frame
        .pointer("/params/line")
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_owned();
    vec![ok(
        frame,
        json!({"text": format!("ran /{line} on the worker")}),
    )]
}

/// `/` over an empty composer opens the verb popup; Enter on a verb sends it to the
/// worker and the reply lands in the transcript as a note.
#[test]
fn the_slash_popup_runs_a_verb_on_the_worker() -> TestResult {
    let mut fixture = session_fixture();
    fixture.push(Step::Expect("_yi/slash", slash_reply));
    run(
        "slash",
        fixture,
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         type /\n\
         wait-frame 3000 plantree\n\
         type goal\n\
         key enter\n\
         wait-frame 5000 ran /goal on the worker\n\
         wait-frame 3000 !plantree\n\
         quit\n",
    )
}

/// `/new` is the console's own verb: it opens a fresh session in the focused pane.
#[test]
fn slash_new_opens_a_fresh_session_in_the_pane() -> TestResult {
    let mut fixture = session_fixture();
    fixture.push(Step::Expect("session/new", new_session_reply));
    fixture.push(Step::Expect("_yi/seen", seen_ok));
    run(
        "slash-new",
        fixture,
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         type /new\n\
         key enter\n\
         wait-frame 5000 !replayed world\n\
         quit\n",
    )
}

fn status_push() -> Vec<Value> {
    let mut reply = assistant("");
    if let AgentMessage::Assistant { usage, .. } = &mut reply {
        usage.cost.total = serde_json::Number::from_f64(0.12).unwrap_or_else(|| 0.into());
        usage.total_tokens = 2000;
    }
    vec![
        event("s-alpha", 30, &AgentEvent::MessageEnd { message: reply }),
        update(
            "s-alpha",
            json!({"sessionUpdate": "usage_update", "used": 2000, "size": 200000}),
        ),
    ]
}

/// The status row is the solo one: model, reasoning effort, cost and context share, with
/// the session's name on the right.
#[test]
fn the_status_row_shows_model_effort_cost_and_context() -> TestResult {
    let mut fixture = session_fixture();
    fixture.push(Step::Push(status_push));
    run(
        "status-row",
        fixture,
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         wait-frame 3000 faux-1\n\
         wait-frame 3000 ◉ medium\n\
         wait-frame 3000 $0.12\n\
         wait-frame 3000 2,000 / 200K\n\
         quit\n",
    )
}

fn running_push() -> Vec<Value> {
    vec![
        update(
            "s-alpha",
            json!({"sessionUpdate": "state_update", "state": "running"}),
        ),
        event("s-alpha", 30, &AgentEvent::AgentStart),
    ]
}

/// A working session shows the solo working line above the composer, esc hint included.
#[test]
fn a_working_session_shows_the_working_line() -> TestResult {
    let mut fixture = session_fixture();
    fixture.push(Step::Push(running_push));
    run(
        "working-line",
        fixture,
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         wait-frame 3000 [esc] interrupt\n\
         quit\n",
    )
}

/// Two panes on one session both reduce the same events; each keeps its own composer.
#[test]
fn two_panes_one_session_both_render_events() -> TestResult {
    let mut fixture = session_fixture();
    fixture.push(Step::Expect("session/resume", resume_alpha));
    fixture.push(Step::Expect("_yi/seen", seen_ok));
    fixture.push(Step::Push(|| {
        stream("s-alpha", 50, &assistant("both see me"))
    }));
    let frame = run_frames(
        "two-panes",
        fixture,
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         key alt-v\n\
         key alt-/\n\
         type alpha\n\
         key enter\n\
         wait-frame 5000 both see me\n\
         wait 300\n\
         quit\n",
    )?;
    assert_eq!(
        frame.matches("both see me").count(),
        2,
        "both panes render the streamed reply: {frame}"
    );
    Ok(())
}

fn malformed_event() -> Vec<Value> {
    vec![update(
        "s-alpha",
        json!({"sessionUpdate": "_yi/event", "event": {"type": "nonsense"}, "seq": 0}),
    )]
}

/// A `_yi/event` that does not decode is counted and dropped, never a panic or a wedge.
#[test]
fn a_malformed_event_frame_is_counted_and_ignored() -> TestResult {
    let mut fixture = session_fixture();
    fixture.push(Step::Push(malformed_event));
    fixture.push(Step::Push(|| {
        stream("s-alpha", 1, &assistant("still alive"))
    }));
    run(
        "malformed",
        fixture,
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         wait-frame 5000 dropped:1\n\
         wait-frame 5000 still alive\n\
         quit\n",
    )
}

/// ⌥2 resumes the second rail slot into the focused pane, whatever the cursor says.
#[test]
fn alt_digit_resumes_the_rail_slot() -> TestResult {
    run(
        "slot-jump",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", named_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_named),
            Step::Expect("_yi/seen", seen_ok),
        ],
        "wait-frame 5000 2 FI fix login bug\n\
         key alt-2\n\
         wait-frame 5000 resumed s-alpha\n\
         wait-frame 3000 FI · fix login\n\
         quit\n",
    )
}

/// The row of the session in front carries the active background; the cursor row the
/// louder selection one. Text frames cannot show it, so the spans are read directly.
#[test]
fn the_focused_session_row_wears_the_active_background() -> TestResult {
    use yi_console::app::App;
    use yi_console::model::{PaneContent, SessionId, SessionRow, SessionStatus, Zone};
    use yi_tui::colors::{ColorTier, Theme};
    let theme = Theme::new(ColorTier::TrueColor, true);
    let mut app = App::new("/tmp/demo-root".to_owned(), theme);
    for id in ["s-alpha", "s-beta"] {
        app.state.upsert_row(SessionRow {
            id: SessionId(id.to_owned()),
            root: "/tmp/demo-root".to_owned(),
            status: SessionStatus::Idle,
            attached: false,
            name: Some(id.to_owned()),
            created_ms: 1,
            last_ms: 1,
        });
    }
    if let Some(pane) = app.state.focused_pane_mut() {
        pane.content = PaneContent::Session {
            session: Some(SessionId("s-beta".to_owned())),
            chat: None,
        };
    }
    app.state.zone = Zone::Panes;
    app.state.sidebar = yi_console::model::SidebarMode::Full;
    let rows = yi_console::sidebar::sidebar_lines(&app, &theme, 10);
    let bg_of = |id: &str| {
        let index = app.state.order.iter().position(|row| row.0 == id);
        rows.iter()
            .find(|row| row.index == index)
            .and_then(|row| row.line.spans.first())
            .and_then(|span| span.style.bg)
    };
    assert_eq!(bg_of("s-beta"), Some(theme.active_row_bg()));
    assert_eq!(bg_of("s-alpha"), None);
    Ok(())
}

fn old_daemon_resume(frame: &Value) -> Vec<Value> {
    vec![
        update(
            "s-alpha",
            json!({"sessionUpdate": "agent_message", "messageId": "msg_1",
            "content": [{"type": "text", "text": "standard kinds only"}]}),
        ),
        ok(frame, json!({"sessionId": "s-alpha", "configOptions": []})),
    ]
}

/// A daemon that resumes without a `_yi/replay` is an older binary; the pane says so
/// instead of sitting silent under every prompt.
#[test]
fn an_old_daemon_is_named_when_the_resume_brings_no_replay() -> TestResult {
    run(
        "old-daemon",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", old_daemon_resume),
            Step::Expect("_yi/seen", seen_ok),
        ],
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 older yi\n\
         quit\n",
    )
}

/// Unnamed sessions take their tile and colour from the id, so no two look alike.
#[test]
fn unnamed_rows_take_their_tile_from_the_id() -> TestResult {
    use yi_console::app::App;
    use yi_console::model::{SessionId, SessionRow, SessionStatus};
    use yi_tui::colors::{ColorTier, Theme, name_accent};
    let theme = Theme::new(ColorTier::TrueColor, true);
    let mut app = App::new("/tmp/demo-root".to_owned(), theme);
    for id in ["alpha-1", "beta-2"] {
        app.state.upsert_row(SessionRow {
            id: SessionId(id.to_owned()),
            root: "/tmp/demo-root".to_owned(),
            status: SessionStatus::Idle,
            attached: false,
            name: None,
            created_ms: 1,
            last_ms: 1,
        });
    }
    let rows = yi_console::sidebar::sidebar_lines(&app, &theme, 10);
    let tiles: Vec<(String, Option<ratatui::style::Color>)> = rows
        .iter()
        .filter_map(|row| row.line.spans.get(1))
        .map(|span| (span.content.to_string(), span.style.bg))
        .collect();
    assert!(
        tiles.contains(&("AL".to_owned(), Some(name_accent("alpha-1")))),
        "{tiles:?}"
    );
    assert!(
        tiles.contains(&("BE".to_owned(), Some(name_accent("beta-2")))),
        "{tiles:?}"
    );
    Ok(())
}

/// The command palette runs an action by name: `split` splits the focused pane.
#[test]
fn the_command_palette_runs_an_action_by_name() -> TestResult {
    run(
        "palette-action",
        session_fixture(),
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         key alt-/\n\
         wait-frame 3000 actions\n\
         type split right\n\
         wait-frame 3000 ▸ split right\n\
         key enter\n\
         wait-frame 3000 no session\n\
         wait-frame 3000 ╭\n\
         quit\n",
    )
}

/// ⌥? shows every chord with its keys; the next key closes it.
#[test]
fn the_keys_overlay_lists_every_chord() -> TestResult {
    run(
        "keys",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
        ],
        "wait-frame 5000 s-alpha\n\
         key alt-?\n\
         wait-frame 3000 stop the daemon and quit\n\
         wait-frame 3000 ctrl+b then\n\
         key esc\n\
         wait-frame 3000 !ctrl+b then\n\
         quit\n",
    )
}

#[test]
fn the_rail_reads_from_the_top() -> TestResult {
    let frame = run_frames_with(
        "rail-top",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", ledger_list),
        ],
        "wait-frame 5000 ●\n\
         wait 200\n\
         quit\n",
        SidebarMode::Rail,
    )?;
    let first = frame.lines().next().ok_or("an empty frame")?;
    assert!(
        first.contains("1 SB"),
        "the first slot is on the first row: {first}"
    );
    Ok(())
}
