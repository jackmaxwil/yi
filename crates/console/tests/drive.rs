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
use yi_types::plan::doc::{AgentId, Todo, TodoLabel, TodoState};
use yi_types::todo::{PhaseName, TodoList, TodoPhase};

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

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

fn scratch_socket(name: &str) -> std::io::Result<(Scratch, PathBuf)> {
    let dir = Scratch::new(&format!("yi-console-test-{name}"))?;
    let socket = dir.join("sock");
    Ok((dir, socket))
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
    AgentMessage::user_input(UserContent::Text(text.to_owned()), 0)
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
                assistant_message_event: AssistantMessageEvent::TextDelta {
                    content_index: 0,
                    delta: text,
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

/// `extra` is Yi's own fields, which ride `_meta.yi` beside the name.
fn session_result(frame: &Value, session: &str, name: Option<&str>, extra: Value) -> Value {
    let mut yi = json!({"name": name});
    if let (Some(map), Some(more)) = (yi.as_object_mut(), extra.as_object()) {
        for (key, value) in more {
            map.insert(key.clone(), value.clone());
        }
    }
    ok(
        frame,
        json!({
            "sessionId": session,
            "configOptions": [
                {"configId": "model", "name": "Model", "type": "select",
                 "currentValue": "faux/faux-1", "options": []},
                {"configId": "thought_level", "name": "Thinking level", "type": "select",
                 "currentValue": "medium", "options": []},
            ],
            "_meta": {"yi": yi},
        }),
    )
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

/// Incident: bound inside the thread, the socket lost the race to a console that dialled
/// first, and the join then waited on an accept until nextest killed the test at 60s.
fn spawn_fixture(socket: PathBuf, script: Vec<Step>) -> JoinHandle<Result<(), String>> {
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).map_err(|error| format!("fixture bind: {error}"));
    std::thread::spawn(move || fixture_loop(&listener?, script))
}

fn init_reply(frame: &Value) -> Vec<Value> {
    vec![ok(
        frame,
        json!({
            "protocolVersion": 2,
            "info": {"name": "yi", "version": "test"},
            "capabilities": {"session": {"delete": {}}},
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
    run_frames_with(name, fixture, script, SidebarMode::Full, 100)
}

fn run_frames_with(
    name: &str,
    fixture: Vec<Step>,
    script: &str,
    sidebar: SidebarMode,
    width: u16,
) -> Result<String, Box<dyn Error>> {
    let dir = Scratch::new(&format!("yi-console-frames-{name}"))?;
    run_opts(
        name,
        fixture,
        script,
        false,
        sidebar,
        Some(dir.to_path_buf()),
        width,
    )?;
    last_frame(&dir)
}

fn last_frame(dir: &std::path::Path) -> Result<String, Box<dyn Error>> {
    let mut names: Vec<PathBuf> = std::fs::read_dir(dir)?
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
    run_opts(name, fixture, script, autostart, sidebar, None, 100)
}

fn run_opts(
    name: &str,
    fixture: Vec<Step>,
    script: &str,
    autostart: bool,
    sidebar: SidebarMode,
    frames: Option<PathBuf>,
    width: u16,
) -> TestResult {
    let (_dir, socket) = scratch_socket(name)?;
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
            width,
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
            {"sessionId": "s-alpha", "cwd": "/tmp/demo-root", "title": "s-alpha",
             "_meta": {"yi": {"attached": false}}},
            {"sessionId": "s-beta", "cwd": "/tmp/demo-root", "title": "s-beta",
             "_meta": {"yi": {"attached": false}}},
        ]}),
    )]
}

fn ledger_list(frame: &Value) -> Vec<Value> {
    vec![ok(
        frame,
        json!({"sessions": [
            {"sessionId": "s-beta", "title": "s-beta", "cwd": "/tmp/demo-root",
             "_meta": {"yi": {"attached": false, "unseen": 2, "lastState": "idle", "lastEventMs": 1}}},
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

fn resume_alpha_on_a_lane(frame: &Value) -> Vec<Value> {
    let mut frames = resume_alpha(frame);
    frames.insert(
        0,
        update(
            "s-alpha",
            json!({
                "sessionUpdate": "_yi/workdir",
                "cwd": "/home/user/.yi/lanes/897d6e91/1",
                "lane": "yi ⎇ lane 1",
            }),
        ),
    );
    frames
}

/// D208: the row named the root the console opened, for a session that ran in a lane.
#[test]
fn the_status_row_follows_the_workers_lane() -> TestResult {
    run(
        "workdir",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_alpha_on_a_lane),
            Step::Expect("_yi/seen", seen_ok),
        ],
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         wait-frame 5000 ⎇ lane 1\n\
         quit\n",
    )
}

#[test]
fn prompt_rejected_while_daemon_unreachable() -> TestResult {
    // No listener at all: the client keeps retrying and the composer submit
    // is refused with a visible status note, never queued.
    let (_dir, socket) = scratch_socket("unreachable")?;
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
    let (_dir, socket) = scratch_socket("tiny")?;
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
                    json!({"configOptions": [], "_meta": {"yi": {"name": "s-beta"}}}),
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

fn ipython_updates(content: Value, media: Value) -> Vec<Value> {
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
            "toolCallId": "call_1", "status": "completed", "content": content,
            "rawOutput": {
                "stdout": "computing drift…",
                "result": "+4.1 mm",
                "attachments": 1, "attachmentMedia": media,
            }}),
        ),
    ]
}

/// A session written before the image moved to the result content still draws it.
#[test]
fn notebook_pane_shows_cells_and_image_placeholder() -> TestResult {
    notebook_shows_image("notebook", |frame| {
        ipython_prompt(
            frame,
            json!([{"type": "content", "content": {"type": "text", "text": "attached"}}]),
            json!([{"mime_type": "image/png", "data": "aWJvcnk="}]),
        )
    })
}

#[test]
fn notebook_pane_draws_the_image_from_the_result_content() -> TestResult {
    notebook_shows_image("notebook-content", |frame| {
        ipython_prompt(
            frame,
            json!([{"type": "content", "content": {"type": "image", "data": "aWJvcnk=", "mimeType": "image/png"}}]),
            json!([{"mime_type": "image/png"}]),
        )
    })
}

fn ipython_prompt(frame: &Value, content: Value, media: Value) -> Vec<Value> {
    let mut frames = vec![ok(frame, json!({}))];
    frames.extend(ipython_updates(content, media));
    frames
}

fn notebook_shows_image(name: &str, prompt: Responder) -> TestResult {
    run(
        name,
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_alpha),
            Step::Expect("_yi/seen", seen_ok),
            Step::Expect("session/prompt", prompt),
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
    let dir = Scratch::new("yi-console-md")?;
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
    if let Some(reply) = frames.last_mut() {
        reply["result"]["_meta"]["yi"]["replayedTo"] = json!(2);
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

/// A switch parks the chat it leaves; switching back restores it and asks only for what
/// streamed since its replay, so the transcript shows before any byte of it is resent.
#[test]
fn switching_back_restores_the_parked_chat_and_skips_its_replay() -> TestResult {
    run(
        "parked",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_with_offset),
            Step::Expect("_yi/seen", seen_ok),
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
            Step::Expect("session/resume", |frame| {
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
            Step::Expect("_yi/seen", seen_ok),
        ],
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         key alt-/\n\
         type beta\n\
         key enter\n\
         wait-frame 5000 beta transcript\n\
         key alt-/\n\
         type alpha\n\
         key enter\n\
         wait-frame 5000 offset honored\n\
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
        json!({"sessionId": "s-new", "configOptions": [], "_meta": {"yi": {"name": "s-new"}}}),
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
            Step::Expect("session/new", new_session_reply),
            Step::Expect("session/list", named_list),
            Step::Expect("session/list", empty_list),
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
            Step::Expect("session/new", new_session_reply),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/list", empty_list),
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
         wait-frame 5000 Review · session · 1 file +1 −0\n\
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
         wait-frame 2000 !Review ·\n\
         cmd-g\n\
         wait-frame 3000 Review · session · 1 file +1 −0\n\
         cmd-g\n\
         wait-frame 3000 !Review ·\n\
         quit\n",
    )
}

/// A finished call as the worker reports it: the start, then the end carrying its result.
fn tool_run(seq: u64, id: &str, name: &str, args: Value, text: &str, details: Value) -> [Value; 2] {
    let result = yi_types::event::ToolResult {
        content: vec![Content::Text {
            text: text.to_owned(),
            text_signature: None,
        }],
        details,
        usage: None,
        added_tool_names: None,
        terminate: None,
    };
    let (tool_call_id, tool_name) = (id.to_owned(), name.to_owned());
    [
        event(
            "s-alpha",
            seq,
            &AgentEvent::ToolExecutionStart {
                tool_call_id: tool_call_id.clone(),
                tool_name: tool_name.clone(),
                args,
            },
        ),
        event(
            "s-alpha",
            seq + 1,
            &AgentEvent::ToolExecutionEnd {
                tool_call_id,
                tool_name,
                result,
                is_error: false,
            },
        ),
    ]
}

/// Real producers: sha1_smol 1.0.1's Makefile read through `cat`, and `diff -u` of a
/// one-line recipe edit to it. Make wants its recipes tab-indented.
fn makefile_push() -> Vec<Value> {
    let patch = include_str!("fixtures/makefile.diff");
    let mut frames = Vec::from(tool_run(
        20,
        "b1",
        "bash",
        json!({"command": "cat Makefile"}),
        include_str!("fixtures/makefile.txt"),
        json!({"exitCode": 0}),
    ));
    frames.extend(tool_run(
        22,
        "e1",
        "edit",
        json!({"path": "/tmp/demo-root/Makefile"}),
        "[Makefile#1]\nupdated; first change at line 13",
        json!({"path": "/tmp/demo-root/Makefile", "patch": patch}),
    ));
    frames.push(update(
        "s-alpha",
        json!({"sessionUpdate": "tool_call_update",
        "toolCallId": "e1", "title": "edit", "kind": "edit", "status": "completed",
        "rawOutput": {"patch": patch, "added": 1, "removed": 0}}),
    ));
    frames
}

/// Incident: a tab is one cell to ratatui and a tab stop to the terminal, so the rest of a card
/// row, its grey pad included, landed past the pane edge where no later frame repainted it.
#[test]
fn tab_indented_cards_put_no_control_character_in_a_cell() -> TestResult {
    let dir = Scratch::new("yi-console-frames-card-tabs")?;
    // Read before the exit code: the drive guard fails the run on the frame this asserts.
    let run = run_opts(
        "card-tabs",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_alpha),
            Step::Expect("_yi/seen", seen_ok),
            Step::Push(makefile_push),
            Step::Expect("_yi/tracked", tracked_no),
        ],
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         wait-frame 3000 @cargo build\n\
         wait-frame 3000 @cargo clippy --all-targets\n\
         quit\n",
        false,
        SidebarMode::Full,
        Some(dir.to_path_buf()),
        100,
    );
    let frame = last_frame(&dir)?;
    let control = frame.chars().find(|c| c.is_control() && *c != '\n');
    assert_eq!(
        control, None,
        "a control character reached a cell:\n{frame}"
    );
    run
}

fn why_reply(frame: &Value) -> Vec<Value> {
    let path = frame["params"]["path"].clone();
    let mut answers = vec![
        json!({"line": 2, "uncommitted": true}),
        json!({"line": 40, "error": "git blame failed: fatal: no such path"}),
    ];
    answers.extend(
        (3..9).map(|n| json!({"line": n * 10, "commit": "1a2b3c4d5e6f", "subject": "Add a"})),
    );
    vec![ok(
        frame,
        json!({"path": path, "cap": 8, "unasked": [90, 120], "answers": answers}),
    )]
}

/// Dies with `w` asking nothing once ↓ ran past the last file, though the `▸` still marked
/// one; then with a missing chain read as uncommitted and the hunks past the cap unnamed.
#[test]
fn why_from_review_asks_for_the_marked_file_and_names_what_it_could_not_answer() -> TestResult {
    let last = run_frames(
        "review-why",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_alpha),
            Step::Expect("_yi/seen", seen_ok),
            Step::Push(edit_push),
            Step::Expect("_yi/tracked", tracked_yes),
            Step::Expect("_yi/why", why_reply),
        ],
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         wait-frame 5000 Review · session · 1 file +1 −0\n\
         key alt-right\n\
         key down\n\
         key down\n\
         key w\n\
         wait-frame 3000 L2 not committed yet\n\
         quit\n",
    )?;
    for needle in ["L40 no chain: git blame failed", "[…] 8 of 10 hunks asked"] {
        assert!(last.contains(needle), "{needle}\n{last}");
    }
    Ok(())
}

/// Dies with one stray `l` in Review landing the branch: the first press shows what a second
/// would run, another key cancels it, and only the second press sends it.
#[test]
fn landing_from_review_asks_for_a_second_press() -> TestResult {
    run(
        "review-land",
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
         cmd-g\n\
         wait-frame 3000 Review · session\n\
         key alt-right\n\
         key l\n\
         wait-frame 3000 │l again to /land s-alpha · any other\n\
         key x\n\
         wait-frame 3000 !l again to /land\n\
         key l\n\
         key l\n\
         wait-frame 3000 landing: the gate's jobs\n\
         quit\n",
    )
}

fn unnamed_list(frame: &Value) -> Vec<Value> {
    vec![ok(
        frame,
        json!({"sessions": [
            {"sessionId": "s-alpha", "cwd": "/tmp/demo-root", "_meta": {"yi": {"attached": false}}},
        ]}),
    )]
}

/// Dies with a pull request titled "untitled": a session with no title lands nothing from
/// Review and says how to name the landing.
#[test]
fn an_untitled_session_does_not_land_from_review() -> TestResult {
    run(
        "review-untitled",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", unnamed_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_alpha),
            Step::Expect("_yi/seen", seen_ok),
        ],
        "wait-frame 5000 untitled\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         cmd-g\n\
         wait-frame 3000 Review · session\n\
         key alt-right\n\
         key l\n\
         key l\n\
         wait-frame 3000 this session has no title yet: /land <title> in its chat\n\
         wait-frame 1000 !landing: the gate's jobs\n\
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
         wait-frame 2000 !Review ·\n\
         quit\n",
    )
}

fn other_root_ledger(frame: &Value) -> Vec<Value> {
    vec![ok(
        frame,
        json!({"sessions": [
            {"sessionId": "s-gamma", "title": "s-gamma", "cwd": "/tmp/other-root",
             "_meta": {"yi": {"attached": false, "unseen": 0, "lastState": "idle", "lastEventMs": 1}}},
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
         wait-frame 5000 └GR  grep-bot-sub-1a2b3c4 ◐\n\
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

/// At 100 columns beside the full sidebar the hint row drops whole keys by rank,
/// notebook first, and still ends in `⌥? keys` rather than a key cut in half.
#[test]
fn the_hint_row_keeps_whole_keys() -> TestResult {
    let frame = run_frames(
        "hint-row",
        session_fixture(),
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         quit\n",
    )?;
    let last = frame.lines().last().unwrap_or_default();
    let last = last.trim_end_matches(['"', ',', ' ']);
    assert!(
        last.ends_with("   ⌥/ command palette   ⌥n new session   ⌥b sidebar   ⌥g diff   ⌥? keys"),
        "the notebook drops whole and ⌥? keys stays: {last:?}"
    );
    Ok(())
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
    // The sidebar fits `s-alpha` to its ten-column floor, 19 wide: border at x=19, inner
    // x=20, gutter "1 " puts text at x=22; row 0 at y=1.
    run(
        "editor-mouse",
        session_fixture(),
        &open_editor_script(
            &path,
            "wait-frame 3000 abcdef\nmouse down 24 1\nmouse up 24 1\ntype X\n\
             wait-frame 3000 abXcdef\nmouse down 22 1\nmouse drag 24 1\nmouse up 24 1\n\
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

/// Dies with ctrl+c on an open `/` popup arming the daemon-stop note while the popup stayed:
/// the popup took the key and ignored it, and the console counted it as the first of two.
#[test]
fn ctrl_c_closes_an_open_popup_without_arming_the_quit() -> TestResult {
    run(
        "ctrl-c-popup",
        session_fixture(),
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         type /\n\
         wait-frame 3000 plantree\n\
         key ctrl-c\n\
         wait-frame 3000 !plantree\n\
         wait-frame 3000 !stops the daemon\n\
         quit\n",
    )
}

/// With focus off the chat a drafted box is not this key's business: the draft stays and the
/// press is the first of the two that stop the daemon.
#[test]
fn ctrl_c_off_the_chat_arms_the_quit_and_keeps_the_draft() -> TestResult {
    run(
        "ctrl-c-sidebar",
        session_fixture(),
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         type draft text\n\
         key tab\n\
         key ctrl-c\n\
         wait-frame 3000 stops the daemon\n\
         wait-frame 3000 draft text\n\
         quit\n",
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
            {"sessionId": "s-alpha", "cwd": "/tmp/demo-root", "title": "fix login bug",
             "_meta": {"yi": {"attached": false, "createdAt": now - 3 * 3_600_000}}},
            {"sessionId": "s-beta", "cwd": "/tmp/demo-root", "title": "release notes",
             "_meta": {"yi": {"attached": false, "createdAt": now - 5 * 60_000}}},
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
            {"sessionId": "s-gamma", "title": "s-gamma", "cwd": "/tmp/other-root",
             "_meta": {"yi": {"attached": false, "unseen": 0, "lastState": "idle",
                              "lastEventMs": now_ms()}}},
        ]}),
    )]
}

/// Dies with rows grouped by workspace: the inbox ranks every root's sessions by what they
/// need, so the idle session of the other root outranks two the ledger never placed. A row
/// opens with a quote in the frame, so the section needle cannot match an `idle · 5m` line.
#[test]
fn the_inbox_ranks_rows_by_need_across_workspaces() -> TestResult {
    run(
        "root-groups",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", named_list),
            Step::Expect("session/list", two_root_ledger),
        ],
        "wait-frame 5000 1 SG   s-gamma\n\
         wait-frame 3000 2 RE   release notes\n\
         wait-frame 3000 3 FI   fix login bug\n\
         wait-frame 3000 \"  idle\n\
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
        "wait-frame 5000 1 RE   release notes\n\
         wait-frame 3000 2 FI   fix login bug\n\
         key down\n\
         key enter\n\
         wait-frame 5000 resumed s-alpha\n\
         wait-frame 3000 1 FI   fix login bug\n\
         wait-frame 3000 2 RE   release notes\n\
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
        "wait-frame 5000 1 RE   release notes\n\
         wait-frame 3000 2 FI   fix login bug\n\
         wait-frame 3000 idle · 5m\n\
         wait-frame 3000 idle · 3h\n\
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
            {"sessionId": "s-alpha", "cwd": "/tmp/demo-root", "title": "fix login",
             "_meta": {"yi": {"attached": true, "unseen": 0, "lastState": "idle",
                              "lastEventMs": now_ms()}}},
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

/// One session, `s-alpha`, with its children as `_yi/subagent_update` sends them.
fn session_with_children(
    mode: SidebarMode,
    status: yi_console::model::SessionStatus,
    name: &str,
    children: &[Value],
) -> Result<yi_console::app::App, Box<dyn Error>> {
    use yi_console::model::{SessionId, SessionRow};
    let theme = yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::Ansi16, true);
    let mut app = yi_console::app::App::new("/tmp/demo-root".to_owned(), theme);
    app.state.sidebar = mode;
    app.state.upsert_row(SessionRow {
        id: SessionId("s-alpha".to_owned()),
        root: "/tmp/demo-root".to_owned(),
        status,
        attached: false,
        name: Some(name.to_owned()),
        created_ms: 1000,
        last_ms: 0,
    });
    let children = children
        .iter()
        .map(|child| serde_json::from_value(child.clone()))
        .collect::<Result<_, _>>()?;
    app.state
        .children
        .insert(SessionId("s-alpha".to_owned()), children);
    Ok(app)
}

fn wire_child(name: &str, status: &str, flag: Option<Value>) -> Value {
    let mut update = json!({"id": format!("c-{name}"), "name": name, "status": status,
        "activity": "executing", "toolUseCount": 2, "tokenCount": 100});
    if let Some(flag) = flag {
        update["flag"] = flag;
    }
    update
}

fn needs_you_flag() -> Option<Value> {
    Some(json!({"state": "needs_you", "note": "asks r1: which file?"}))
}

/// A row's last mark and the terminal cell ratatui draws it in.
fn last_mark(line: &ratatui::text::Line<'_>) -> Option<(usize, String)> {
    let area = ratatui::layout::Rect::new(0, 0, 80, 1);
    let mut buffer = ratatui::buffer::Buffer::empty(area);
    buffer.set_line(0, 0, line, 80);
    buffer
        .content()
        .iter()
        .enumerate()
        .rev()
        .find(|(_, cell)| !cell.symbol().trim().is_empty())
        .map(|(at, cell)| (at, cell.symbol().to_owned()))
}

fn cell_of(line: &ratatui::text::Line<'_>) -> Option<usize> {
    last_mark(line).map(|(cell, _)| cell)
}

/// The rail's child rows read as the session's children: a connector under its
/// avatar and the status glyph in the session's column, not one cell left of it.
#[test]
fn a_child_row_hangs_off_its_session_with_the_glyph_in_the_column() -> TestResult {
    use yi_console::model::SessionStatus;
    let theme = yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::Ansi16, true);
    let children = [
        wire_child("grep-bot", "running", None),
        wire_child("edit-bot", "error", None),
    ];
    let app = session_with_children(
        SidebarMode::Rail,
        SessionStatus::Working,
        "fix login",
        &children,
    )?;
    let rows = yi_console::sidebar::sidebar_lines(&app, &theme, 60);
    let [session, _, first, last] = rows.as_slice() else {
        return Err(format!("session, spacer and two children: {}", rows.len()).into());
    };
    for (row, connector) in [(first, '├'), (last, '└')] {
        let text = row.line.to_string();
        assert_eq!(text.chars().nth(2), Some(connector), "{text}");
        assert_eq!(row.avatar.as_ref().map(|at| at.col), Some(3), "{text}");
    }
    for row in [session, first, last] {
        assert_eq!(cell_of(&row.line), Some(7), "glyph column: {}", row.line);
    }
    Ok(())
}

fn full_rows(app: &yi_console::app::App) -> Vec<ratatui::text::Line<'static>> {
    let theme = yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::Ansi16, true);
    yi_console::sidebar::sidebar_lines(app, &theme, 60)
        .into_iter()
        .map(|row| row.line)
        .collect()
}

fn row_with<'a>(
    rows: &'a [ratatui::text::Line<'static>],
    text: &str,
) -> Result<&'a ratatui::text::Line<'static>, String> {
    rows.iter()
        .find(|row| row.to_string().contains(text))
        .ok_or(format!("no row with {text}: {rows:#?}"))
}

/// The full sidebar lines a child's glyph up with its session's, whatever the names
/// hold: a wide character takes two cells, so padding by characters would shift it.
#[test]
fn full_sidebar_child_glyphs_share_the_session_column() -> TestResult {
    use yi_console::model::SessionStatus;
    let children = [
        wire_child("grep-bot", "running", None),
        wire_child("构建-🔧-bot", "completed", None),
    ];
    let app = session_with_children(
        SidebarMode::Full,
        SessionStatus::Working,
        "修复登录 fix",
        &children,
    )?;
    let rows = full_rows(&app);
    let column = cell_of(row_with(&rows, "修复登录")?);
    let kids: Vec<_> = rows
        .iter()
        .filter(|row| row.to_string().contains("bot"))
        .collect();
    assert_eq!(kids.len(), 2, "{rows:#?}");
    for (row, connector) in kids.iter().zip(['├', '└']) {
        assert_eq!(row.to_string().chars().nth(2), Some(connector), "{row}");
        assert_eq!(cell_of(row), column, "{row}");
    }
    Ok(())
}

/// A name longer than the name column is cut in the cells the terminal draws. `⚠️` is
/// one cell and a zero-width selector a character at a time, but two cells drawn, so a
/// cut that sums characters keeps a cell too many and pushes the session's `?` past the
/// sidebar's edge.
#[test]
fn a_long_wide_name_is_cut_in_drawn_cells_and_keeps_its_glyph() -> TestResult {
    use yi_console::model::SessionStatus;
    let children = [wire_child(
        "构建-⚠️-a-very-long-child-bot-name",
        "running",
        None,
    )];
    let mut app = session_with_children(
        SidebarMode::Full,
        SessionStatus::Blocked,
        "修复 ⚠️ fix the login redirect loop now",
        &children,
    )?;
    let edge = usize::from(yi_console::sidebar::width(&mut app));
    let rows = full_rows(&app);
    let session = last_mark(row_with(&rows, "修复")?);
    let child = last_mark(row_with(&rows, "构建")?);
    assert_eq!(session, Some((28, "?".to_owned())), "{rows:#?}");
    assert_eq!(child, Some((28, "◐".to_owned())), "{rows:#?}");
    assert!(28 < edge, "the glyph fits the sidebar: {edge}");
    Ok(())
}

/// A control character takes no cell on screen, so it takes none in the name column.
#[test]
fn a_control_character_in_a_name_does_not_move_the_glyph() -> TestResult {
    use yi_console::model::SessionStatus;
    let children = [wire_child("bell\u{7}bot", "running", None)];
    let app = session_with_children(
        SidebarMode::Full,
        SessionStatus::Working,
        "tab\there",
        &children,
    )?;
    let rows = full_rows(&app);
    let session = row_with(&rows, "here")?;
    let child = row_with(&rows, "bot")?;
    assert!(
        !session.to_string().contains('\t') && !child.to_string().contains('\u{7}'),
        "{rows:#?}"
    );
    assert_eq!(cell_of(session), cell_of(child), "{rows:#?}");
    Ok(())
}

/// A child that needs you is never hidden behind three finished ones: children show
/// most in need first, and the rest are counted on the last connector.
#[test]
fn a_child_that_needs_you_is_never_hidden_behind_finished_ones() -> TestResult {
    use yi_console::model::SessionStatus;
    let theme = yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::Ansi16, true);
    let children = [
        wire_child("one", "completed", None),
        wire_child("two", "completed", None),
        wire_child("three", "completed", None),
        wire_child("asker", "running", needs_you_flag()),
    ];
    let app = session_with_children(
        SidebarMode::Rail,
        SessionStatus::Working,
        "fix login",
        &children,
    )?;
    let rows: Vec<String> = yi_console::sidebar::sidebar_lines(&app, &theme, 60)
        .iter()
        .skip(2)
        .map(|row| row.line.to_string())
        .collect();
    let starts: Vec<String> = rows.iter().map(|row| row.trim_end().to_owned()).collect();
    assert_eq!(
        starts,
        ["  ├AS  ?", "  ├ON  ○", "  ├TW  ○", "  └ +1"],
        "{rows:#?}"
    );
    Ok(())
}

/// Owner, 2026-09-28: "? = needs you, ✕ = failed". A failed child no longer wears
/// the session's needs-you mark, a child that needs you wears it in the session's
/// colour, and every mark is one cell, so none can push the column.
#[test]
fn one_glyph_one_meaning() -> TestResult {
    use yi_console::model::SessionStatus;
    use yi_types::status_mark::StatusMark;
    let theme = yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::Ansi16, true);
    let stuck = json!({"state": "stuck", "note": "idle 300s"});
    let children = [
        wire_child("asker", "running", needs_you_flag()),
        wire_child("broke", "error", None),
        wire_child("stalled", "running", Some(stuck)),
    ];
    let app = session_with_children(
        SidebarMode::Rail,
        SessionStatus::Working,
        "fix login",
        &children,
    )?;
    let rows = yi_console::sidebar::sidebar_lines(&app, &theme, 60);
    let marks: Vec<(String, Option<ratatui::style::Color>)> = rows
        .iter()
        .skip(2)
        .filter_map(|row| {
            let glyph = row.line.spans.last()?;
            Some((glyph.content.to_string(), glyph.style.fg))
        })
        .collect();
    let needs_you = SessionStatus::Blocked.glyph();
    assert_eq!(needs_you, "?", "a session that needs you");
    assert_eq!(
        marks,
        [
            ("?".to_owned(), Some(theme.error)),
            ("✕".to_owned(), Some(theme.error)),
            ("!".to_owned(), Some(theme.warning)),
        ],
        "child marks and colours"
    );
    let every = [
        StatusMark::NeedsYou,
        StatusMark::Working,
        StatusMark::Stuck,
        StatusMark::DoneUnseen,
        StatusMark::Idle,
        StatusMark::Failed,
        StatusMark::Unknown,
    ];
    for glyph in every.map(StatusMark::glyph) {
        assert_eq!(ratatui::text::Line::from(glyph).width(), 1, "{glyph}");
    }
    Ok(())
}

/// A child's name starts under its session's name, so it needs no more cells than its
/// own: a 15-cell child name sizes the sidebar at 15, not 18.
#[test]
fn a_child_name_sizes_the_sidebar_by_its_own_width() -> TestResult {
    use yi_console::model::SessionStatus;
    let children = [wire_child("fifteen-cell-ab", "running", None)];
    let mut app =
        session_with_children(SidebarMode::Full, SessionStatus::Working, "fix", &children)?;
    assert_eq!(yi_console::sidebar::width(&mut app), 15 + 9);
    Ok(())
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
         wait-frame 3000 2 SA   s-alpha\n\
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
            json!({"sessionId": format!("s-{n:02}"), "cwd": "/tmp/demo-root",
                   "title": format!("s-{n:02}"),
                   "_meta": {"yi": {"attached": false, "createdAt": 1000 + n}}})
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

/// Dies with `2,000 / 200K` in the left third of a 254-column pane: every meter sat in the
/// left group and the right one, the session name, is hidden in the console.
#[test]
fn the_wide_pane_puts_its_meters_on_the_right() -> TestResult {
    let mut fixture = session_fixture();
    fixture.push(Step::Push(status_push));
    let frame = run_frames_with(
        "wide-status",
        fixture,
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         wait-frame 3000 2,000 / 200K\n\
         quit\n",
        SidebarMode::Full,
        254,
    )?;
    let row = frame
        .lines()
        .find(|line| line.contains("2,000 / 200K"))
        .ok_or_else(|| format!("no status row:\n{frame}"))?;
    let (_, after) = row
        .trim_end_matches('"')
        .split_once("2,000 / 200K")
        .unwrap_or_default();
    let tail = ratatui::text::Line::from(after).width();
    assert!(
        tail <= 3,
        "the context meter ends {tail} cells short of the edge: {row}"
    );
    Ok(())
}

/// Dies with `anthropic/claude-opus-5.5`: on a router the `vendor/` prefix names the maker,
/// so the session read as a direct one and hid which key pays.
#[test]
fn a_routed_model_names_the_router() -> TestResult {
    let mut fixture = session_fixture();
    fixture.push(Step::Push(|| {
        vec![config_frame("openrouter/anthropic/claude-opus-5.5", "high")]
    }));
    run(
        "routed-model",
        fixture,
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         wait-frame 3000 claude-opus-5.5 via openrouter\n\
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

fn tape_reply(frame: &Value) -> Vec<Value> {
    vec![ok(
        frame,
        json!({"start": 0, "end": 60_000, "model": [[0, 30_000]], "tools": [],
            "marks": [{"at": 0, "kind": "user", "entry": "e0", "label": "hello agent"}]}),
    )]
}

/// Dies with a rewind sent into a running turn from the Tape: the pane says to wait for the
/// turn to end, and sends nothing.
#[test]
fn the_tape_refuses_a_rewind_while_a_turn_runs() -> TestResult {
    let mut fixture = session_fixture();
    fixture.push(Step::Push(running_push));
    fixture.push(Step::Expect("_yi/tape", tape_reply));
    run(
        "tape-running",
        fixture,
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         wait-frame 3000 [esc] interrupt\n\
         cmd-y\n\
         wait-frame 3000 +0s · hello agent\n\
         key alt-right\n\
         key enter\n\
         wait-frame 3000 a turn is running: rewind once it ends\n\
         quit\n",
    )
}

fn tape_of(frame: &Value, labels: &[&str]) -> Vec<Value> {
    let marks: Vec<Value> = labels
        .iter()
        .enumerate()
        .map(|(index, label)| {
            json!({"at": index * 60_000, "kind": "user", "entry": format!("e{index}"), "label": label})
        })
        .collect();
    let end = labels.len() * 60_000;
    vec![ok(
        frame,
        json!({"start": 0, "end": end, "model": [], "tools": [], "marks": marks}),
    )]
}

fn prompt_then_idle(frame: &Value) -> Vec<Value> {
    vec![
        update(
            "s-alpha",
            json!({"sessionUpdate": "state_update", "state": "running"}),
        ),
        update(
            "s-alpha",
            json!({"sessionUpdate": "state_update", "state": "idle"}),
        ),
        ok(frame, json!({"stopReason": "end_turn"})),
    ]
}

/// Dies with the chosen mark jumping to the newest on every idle refresh: a user who picked
/// an older turn loses it as soon as a turn ends.
#[test]
fn a_tape_refresh_keeps_the_chosen_mark() -> TestResult {
    let mut fixture = session_fixture();
    fixture.push(Step::Expect("_yi/tape", |frame| {
        tape_of(frame, &["first turn", "second turn"])
    }));
    fixture.push(Step::Expect("session/prompt", prompt_then_idle));
    fixture.push(Step::Expect("_yi/tape", |frame| {
        tape_of(frame, &["first turn", "second turn", "third turn"])
    }));
    run(
        "tape-cursor",
        fixture,
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         cmd-y\n\
         wait-frame 3000 +1m · second turn\n\
         key alt-right\n\
         key left\n\
         wait-frame 3000 +0s · first turn\n\
         key alt-left\n\
         type third turn\n\
         key enter\n\
         wait-frame 5000 Tape · 3m\n\
         wait-frame 1000 +0s · first turn\n\
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

fn config_frame(model: &str, effort: &str) -> Value {
    update(
        "s-alpha",
        json!({"sessionUpdate": "_yi/config", "configOptions": [
            {"configId": "model", "name": "Model", "type": "select",
             "currentValue": model, "options": []},
            {"configId": "thought_level", "name": "Thinking level", "type": "select",
             "currentValue": effort, "options": []},
        ]}),
    )
}

/// A pick is two set_config_option requests, model then thought_level, each echoed by a
/// `_yi/config` frame before its answer; only the last frame is the pick.
#[test]
fn a_pick_is_announced_once() -> TestResult {
    let mut fixture = session_fixture();
    const OPUS: &str = "anthropic/claude-opus-5";
    const SONNET: &str = "anthropic/claude-sonnet-5";
    fixture.push(Step::Push(|| vec![config_frame(OPUS, "medium")]));
    // The daemon answers what it holds, not what was asked: the model frame names the
    // new model at the old effort, and would draw a line of its own.
    fixture.push(Step::Expect("session/set_config_option", |frame| {
        vec![config_frame(SONNET, "medium"), ok(frame, json!({}))]
    }));
    fixture.push(Step::Expect("session/set_config_option", |frame| {
        vec![config_frame(SONNET, "low"), ok(frame, json!({}))]
    }));
    let frame = run_frames(
        "config-pick",
        fixture,
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 reasoning medium\n\
         key shift-tab\n\
         wait-frame 5000 reasoning low\n\
         wait 300\n\
         quit\n",
    )?;
    assert_eq!(
        frame.matches("model anthropic/claude-sonnet-5").count(),
        1,
        "one pick, one line: {frame}"
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
        "wait-frame 5000 2 FI   fix login bug\n\
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
        100,
    )?;
    let first = frame.lines().next().ok_or("an empty frame")?;
    assert!(
        first.contains("1 SB"),
        "the first slot is on the first row: {first}"
    );
    Ok(())
}

fn todo_update() -> Vec<Value> {
    let item = |label: &str, state: TodoState| {
        TodoLabel::new(label).ok().map(|label| {
            let mut item = Todo::pending(label);
            item.state = state;
            item
        })
    };
    let Some(phase) = PhaseName::new("Tasks").ok() else {
        return Vec::new();
    };
    let list = TodoList {
        phases: vec![TodoPhase {
            name: phase,
            items: [
                item(
                    "read the record",
                    TodoState::Done {
                        output: None,
                        resolution: None,
                    },
                ),
                item(
                    "write the plan",
                    TodoState::Running {
                        by: AgentId::owner(),
                    },
                ),
            ]
            .into_iter()
            .flatten()
            .collect(),
            extra: serde_json::Map::new(),
        }],
        ..TodoList::default()
    };
    vec![update(
        "s-alpha",
        json!({"sessionUpdate": "_yi/todo", "list": list}),
    )]
}

/// The daemon streams `_yi/todo` and the port kept it, but the pane never asked the port,
/// so the console showed six todo cards and no block above the composer.
#[test]
fn a_todo_update_paints_the_block_above_the_composer() -> TestResult {
    run(
        "todo-hud",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_alpha),
            Step::Expect("_yi/seen", seen_ok),
            Step::Push(todo_update),
        ],
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 replayed world\n\
         wait-frame 5000 Todos 1/2\n\
         wait-frame 5000 2. ▷ write the plan\n\
         quit\n",
    )
}

/// A drag copied silently, so nothing said whether the release had taken; the banner
/// says what left for the clipboard.
#[test]
fn a_drag_over_the_transcript_flashes_what_it_copied() -> TestResult {
    run(
        "copy-flash",
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
         mouse down 32 2\n\
         mouse drag 60 5\n\
         mouse up 60 5\n\
         wait-frame 3000 copied 3 lines\n\
         quit\n",
    )
}

/// A drag stopped at the screen's edge: holding it on the pane's last text row now scrolls
/// the transcript, and the copy keeps the rows that scrolled past, more than a screen of them.
#[test]
fn a_drag_held_at_the_bottom_edge_scrolls_and_copies_past_the_screen() -> TestResult {
    let frame = run_frames(
        "copy-scroll",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_long),
            Step::Expect("_yi/seen", seen_ok),
        ],
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 row 59\n\
         mouse scrollup 50 5\nmouse scrollup 50 5\nmouse scrollup 50 5\n\
         mouse scrollup 50 5\nmouse scrollup 50 5\nmouse scrollup 50 5\n\
         wait-frame 2000 !row 59\n\
         mouse down 40 3\n\
         mouse drag 40 29\n\
         wait-frame 5000 row 59\n\
         mouse up 40 29\n\
         wait-frame 3000 copied\n\
         quit\n",
    )?;
    let copied = frame
        .split("copied ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|count| count.parse::<usize>().ok())
        .ok_or_else(|| format!("no copy flash in:\n{frame}"))?;
    assert!(
        copied > 30,
        "copied {copied} lines, less than a screen:\n{frame}"
    );
    Ok(())
}

fn resume_long(frame: &Value) -> Vec<Value> {
    let text: String = (0..60).map(|row| format!("row {row}\n\n")).collect();
    vec![
        replay(
            "s-alpha",
            &[
                entry("e1", None, 1, &user_message("hello agent")),
                entry("e2", Some("e1"), 2, &assistant(&text)),
            ],
            0,
            None,
        ),
        session_result(frame, "s-alpha", None, json!({"replayedTo": 2})),
    ]
}

/// A newline typed into the draft is not a send: only a prompt that leaves the composer
/// returns a scrolled reader to the bottom.
#[test]
fn only_a_sent_prompt_returns_a_scrolled_pane_to_the_bottom() -> TestResult {
    run(
        "scroll-send",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_long),
            Step::Expect("_yi/seen", seen_ok),
            Step::Expect("session/prompt", seen_ok),
        ],
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 row 59\n\
         mouse scrollup 50 5\nmouse scrollup 50 5\nmouse scrollup 50 5\n\
         mouse scrollup 50 5\nmouse scrollup 50 5\n\
         wait-frame 2000 !row 59\n\
         type more\n\
         key shift-enter\n\
         wait 200\n\
         wait-frame 1000 !row 59\n\
         key enter\n\
         wait-frame 2000 row 59\n\
         quit\n",
    )
}

fn sibling_roots_ledger(frame: &Value) -> Vec<Value> {
    vec![ok(
        frame,
        json!({"sessions": [
            {"sessionId": "s-fix", "title": "fix", "cwd": "/tmp/yi-feature-auth-fix",
             "_meta": {"yi": {"attached": false, "unseen": 0, "lastState": "idle", "lastEventMs": 1}}},
            {"sessionId": "s-ui", "title": "ui", "cwd": "/tmp/yi-feature-auth-ui",
             "_meta": {"yi": {"attached": false, "unseen": 0, "lastState": "idle", "lastEventMs": 1}}},
        ]}),
    )]
}

/// Sibling worktrees share a prefix: a sidebar fitted to short session names cut both
/// workspace names to the same `yi-feature-au`.
#[test]
fn a_fitted_sidebar_still_tells_sibling_workspaces_apart() -> TestResult {
    run(
        "sibling-roots",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/list", sibling_roots_ledger),
        ],
        "wait-frame 5000 workspaces\n\
         wait-frame 3000 yi-feature-auth-fix 1\n\
         wait-frame 3000 yi-feature-auth-ui 1\n\
         quit\n",
    )
}

fn prompt_starts_running(frame: &Value) -> Vec<Value> {
    vec![
        update(
            "s-alpha",
            json!({"sessionUpdate": "state_update", "state": "running"}),
        ),
        ok(frame, json!({"stopReason": "end_turn"})),
    ]
}

/// A turn clears its children and spawns them again; a sidebar that narrowed with them
/// re-wrapped every pane under a scrolled reader mid-turn.
#[test]
fn the_sidebar_does_not_narrow_when_children_clear() -> TestResult {
    let frame = run_frames(
        "no-narrow",
        vec![
            Step::Expect("initialize", init_reply),
            Step::Expect("session/list", two_session_list),
            Step::Expect("session/list", empty_list),
            Step::Expect("session/resume", resume_alpha),
            Step::Expect("_yi/seen", seen_ok),
            Step::Push(subagent_push),
            Step::Expect("session/prompt", prompt_starts_running),
        ],
        "wait-frame 5000 s-alpha\n\
         key enter\n\
         wait-frame 5000 grep-bot-sub-1a2b\n\
         type go\n\
         key enter\n\
         wait-frame 5000 !grep-bot\n\
         quit\n",
    )?;
    let top = frame.lines().next().ok_or("no frame")?;
    let at = top.find("s-alpha").ok_or(format!("no title: {top}"))?;
    let column = top.get(..at).unwrap_or_default().chars().count();
    assert!(
        column >= 30,
        "the pane moved left to column {column}: {top}"
    );
    Ok(())
}
