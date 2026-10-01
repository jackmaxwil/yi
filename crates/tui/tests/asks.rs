//! A child's question, stall and second run as the human sees them, and the reply box.
use crate::common;

use std::error::Error;
use std::sync::Arc;

use common::{VT100Backend, test_model};
use serde_json::{Map, json};
use yi_runtime::{AgentSession, ProviderStream, SessionConfig, SubagentHost, SubagentHostOptions};
use yi_tui::app::{App, TuiOptions};
use yi_tui::colors::{ColorTier, Theme};
use yi_tui::keymap::default_keymap;
use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, StopReason, UserContent};
use yi_types::subagent::{ChildActivity, ChildFlag, ChildId, ChildStatus, ChildUpdate};

use crate::scratch;
use scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

fn options() -> TuiOptions {
    TuiOptions {
        model: test_model("faux-1"),
        session_name: "asks".to_owned(),
        cwd: "/tmp".to_owned(),
        lane: None,
        context_window: 128_000,
        session_dir: String::new(),
        keys: Vec::new(),
        initial_prompt: None,
        pace: 0,
    }
}

fn app() -> App {
    App::new(
        options(),
        Theme::new(ColorTier::TrueColor, true),
        default_keymap(),
        80,
    )
}

fn update(status: ChildStatus, flag: Option<ChildFlag>) -> ChildUpdate {
    ChildUpdate {
        id: ChildId("sub-writer".to_owned()),
        name: "writer".to_owned(),
        status,
        activity: ChildActivity::Waiting,
        tool_use_count: 1,
        token_count: 40,
        answer_preview: None,
        error: None,
        exit: None,
        flag,
    }
}

/// The live region as rows of a real screen.
fn live_rows(app: &mut App) -> Result<Vec<String>, Box<dyn Error>> {
    let mut terminal = yi_tui::terminal::Terminal::new(VT100Backend::new(80, 24), 4)?;
    yi_tui::render::draw(app, &mut terminal, None);
    let backend = terminal.backend();
    Ok((0..24).map(|row| backend.row_text(row)).collect())
}

fn flat(lines: &[ratatui::text::Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

/// Dies with the card reading only the child's status: a child blocked on a question drew as a
/// plain running card, a stalled one the same, and a woken one stayed `done` in scrollback.
#[test]
fn a_card_shows_the_question_the_stall_and_a_second_run() -> TestResult {
    let mut app = app();
    app.adopt(&update(ChildStatus::Running, None), None);
    let note = "asks writer-1: Which file name? notes.md or hello.txt";
    let asking = Some(ChildFlag::NeedsYou {
        note: note.to_owned(),
    });
    app.reduce_child_update(&update(ChildStatus::Running, asking));
    let rows = live_rows(&mut app)?;
    let row = |needle: &str| rows.iter().find(|row| row.contains(needle)).cloned();
    let title = row("Writer").ok_or(format!("no card: {rows:#?}"))?;
    assert!(title.contains("? Writer · needs you"), "{title:?}");
    assert!(
        row(note).is_some(),
        "the question is on the card: {rows:#?}"
    );
    assert!(
        row(yi_tui::cell::REPLY_HINT).is_some(),
        "and how to answer it: {rows:#?}"
    );

    let stalled = Some(ChildFlag::Stuck {
        note: "idle 312s".to_owned(),
    });
    app.reduce_child_update(&update(ChildStatus::Running, stalled));
    let rows = live_rows(&mut app)?;
    let title = rows.iter().find(|row| row.contains("Writer")).cloned();
    assert!(
        title.is_some_and(|title| title.contains("! Writer · stuck")),
        "{rows:#?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("idle 312s")),
        "{rows:#?}"
    );

    app.reduce_child_update(&update(ChildStatus::Completed, None));
    let first = flat(&app.take_commits());
    assert!(
        first.iter().any(|line| line.contains("↳ Writer")),
        "{first:?}"
    );
    app.reduce_child_update(&update(ChildStatus::Running, None));
    let rows = live_rows(&mut app)?;
    let title = rows.iter().find(|row| row.contains("Writer")).cloned();
    assert!(
        title.is_some_and(|title| !title.contains("done") && title.contains("waiting")),
        "a woken child's card runs again: {rows:#?}"
    );
    app.reduce_child_update(&update(ChildStatus::Completed, None));
    let second = flat(&app.take_commits());
    assert!(
        second.iter().any(|line| line.contains("↳ Writer")),
        "its second answer commits a second card: {second:?}"
    );
    Ok(())
}

/// Dies with host words drawn as the user's and mail drawn without its sender: a replayed
/// "[subagent writer finished]" took the prompt bar, and a request lost who asked it. Dies too
/// with a prompt typed before D25, which carries no attribution, replayed as a host notice.
#[test]
fn a_replayed_notice_and_mail_name_their_source() -> TestResult {
    let at = |seq: u64, timestamp: u64, message: AgentMessage| Entry::Message {
        id: format!("e{seq}"),
        message,
        terminate: None,
        parent_id: None,
        seq,
        timestamp,
    };
    let entry = |seq, message| at(seq, yi_types::message::ATTRIBUTED_SINCE_MS + seq, message);
    let notice = AgentMessage::host_text(
        yi_types::message::HostSource::Lifecycle,
        "[subagent writer (sub-writer) finished]",
        0,
    );
    let envelope = json!({"id": "writer-1", "from": "writer", "to": "parent", "kind": "request"});
    let mail = AgentMessage::Custom {
        custom_type: "agent_message".to_owned(),
        content: UserContent::Text(
            "<agent_message from=\"writer\" kind=\"request\" id=\"writer-1\">\nWhich file name?\n</agent_message>".to_owned(),
        ),
        display: true,
        details: Some(envelope),
        timestamp: 0,
    };
    let typed = AgentMessage::user_input(UserContent::Text("write a greeting".to_owned()), 0);
    let old = AgentMessage::User {
        content: UserContent::Text("hello from august".to_owned()),
        timestamp: 0,
        attribution: yi_types::message::Attribution::Unproven,
    };
    let old = at(0, 1_787_622_423_154, old);
    let mut app = app();
    app.replay_entries(&[old, entry(1, typed), entry(2, notice), entry(3, mail)]);
    let lines = flat(&app.take_commits());
    let find = |needle: &str| lines.iter().find(|line| line.contains(needle)).cloned();
    let notice = find("[subagent writer").ok_or(format!("no notice: {lines:#?}"))?;
    assert!(
        notice.contains('⚑'),
        "a host notice is a flagged callout: {notice:?}"
    );
    let asked = find("Which file name?").ok_or(format!("no mail: {lines:#?}"))?;
    assert!(asked.contains("request writer → parent"), "{asked:?}");
    for typed in ["write a greeting", "hello from august"] {
        let prompt = find(typed).ok_or(format!("no prompt: {lines:#?}"))?;
        assert!(
            !prompt.contains('⚑'),
            "the human's words stay theirs: {prompt:?}"
        );
    }
    Ok(())
}

/// Types `text` and presses Enter; what the composer sent, as `prompt <text>` or
/// `answer <question> <text>`.
fn typed(app: &mut App, text: &str) -> Vec<String> {
    use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let key = |code| Event::Key(KeyEvent::new(code, KeyModifiers::NONE));
    for c in text.chars() {
        yi_tui::input::handle_terminal_event(app, &tx, key(KeyCode::Char(c)));
    }
    yi_tui::input::handle_terminal_event(app, &tx, key(KeyCode::Enter));
    let mut sent = Vec::new();
    while let Ok(command) = rx.try_recv() {
        sent.push(match command {
            yi_tui::Command::Prompt(text) => format!("prompt {text}"),
            yi_tui::Command::Answer { question, text, .. } => format!("answer {question} {text}"),
            _ => "other".to_owned(),
        });
    }
    sent
}

fn asks(id: &str) -> Option<ChildFlag> {
    Some(ChildFlag::NeedsYou {
        note: format!("asks {id}: Which file name?"),
    })
}

/// Dies with the reply target read at Enter: a steer typed before the child asked answered the
/// child, and an answer typed before the parent answered first went to the root as a prompt.
#[test]
fn a_draft_answers_the_question_open_when_it_started() -> TestResult {
    let mut app = app();
    app.adopt(&update(ChildStatus::Running, None), None);
    app.set_focus(Some("sub-writer".to_owned()));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let key = |c| {
        use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
    };
    yi_tui::input::handle_terminal_event(&mut app, &tx, key('g'));
    app.reduce_child_update(&update(ChildStatus::Running, asks("writer-1")));
    let sent = typed(&mut app, "o on");
    assert_eq!(
        sent,
        ["prompt go on"],
        "a draft begun before the question is no answer"
    );

    yi_tui::input::handle_terminal_event(&mut app, &tx, key('n'));
    app.reduce_child_update(&update(ChildStatus::Running, None));
    app.reduce_child_update(&update(ChildStatus::Running, asks("writer-2")));
    let sent = typed(&mut app, "otes.md");
    assert_eq!(
        sent,
        ["answer writer-1 notes.md"],
        "the answer stays bound to the question it was written to"
    );
    Ok(())
}

fn faux_session(replies: Vec<AgentMessage>) -> AgentSession {
    let provider = Arc::new(ProviderStream::new(None));
    provider.queue_faux(replies);
    AgentSession::new(
        SessionConfig {
            system_prompt: String::new(),
            model: test_model("faux-1"),
            thinking_level: None,
            tool_execution: yi_runtime::ExecutionMode::Sequential,
        },
        provider,
    )
}

fn said(text: &str) -> AgentMessage {
    yi_runtime::faux::faux_assistant_message(
        vec![yi_runtime::faux::faux_text(text)],
        StopReason::Stop,
    )
}

/// A family whose one child asks its parent a question through `ask_user`.
fn asking_host(dir: &Scratch, parent: &Arc<AgentSession>) -> Arc<SubagentHost> {
    Arc::new(SubagentHost::new(SubagentHostOptions {
        provider: Arc::new(ProviderStream::new(None)),
        depth: 0,
        max_depth: 1,
        max_children: 4,
        parent_session_dir: dir.to_path_buf(),
        cwd: dir.to_path_buf(),
        home: dir.join("home"),
        lane_slots: 1,
        defaults: Arc::new(|| (test_model("faux-1"), yi_types::model::Effort::Medium)),
        factory: Arc::new(|build: yi_runtime::ChildBuild<'_>| {
            let question =
                json!({"question": "Which file name?", "options": ["notes.md", "hello.txt"]});
            let question = question.as_object().cloned().unwrap_or_default();
            let call = yi_runtime::faux::faux_tool_call("ask-1", "ask_user", question);
            let mut child = faux_session(vec![
                yi_runtime::faux::faux_assistant_message(vec![call], StopReason::ToolUse),
                said("wrote the file the parent named"),
            ]);
            let ask = yi_runtime::auto_review::AskUserTool::new(None).asking(Some(build.link));
            child.use_tools(vec![Arc::new(ask)], std::env::temp_dir(), None);
            Ok(child)
        }),
        notice: yi_runtime::wiring::lifecycle_notice(parent),
        events: parent.events_sender(),
        parent_messages: Arc::new(Vec::new),
        report: parent.deliver_hook(),
        attribute: Arc::new(|_usage| {}),
        store: Arc::new(|| None),
        plans_dir: dir.join(".yi/plans"),
        family_live: Arc::new(|| 0),
    }))
}

/// Dies with no road from the human to a child's question: the card said nothing, and Enter
/// with the child focused prompted the root instead of answering. Driven through the real loop.
#[test]
fn the_reply_box_answers_a_childs_question() -> TestResult {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let dir = Scratch::new("yi-tui-reply")?;
    let parent = Arc::new(faux_session((0..4).map(|_| said("noted")).collect()));
    let host = asking_host(&dir, &parent);
    {
        let _context = runtime.enter();
        let mut kwargs = Map::new();
        kwargs.insert("name".to_owned(), json!("writer"));
        host.spawn("write a greeting file".to_owned(), kwargs)?;
    }
    let frames = dir.join("frames");
    let script = yi_tui::parse_script(
        "wait-frame 10000 needs you\nkey alt-down\nwait-frame 5000 reply to writer\ntype notes.md\nkey enter\nwait-frame 10000 you → writer (answering writer-1): notes.md\nwait-frame 10000 · done\nquit\n",
    )?;
    let (_ask_tx, ask_rx) = std::sync::mpsc::channel();
    let code = yi_tui::run_headless(
        runtime,
        Arc::clone(&parent),
        Arc::clone(&host),
        ask_rx,
        options(),
        yi_tui::DriveOptions {
            script,
            frames_dir: Some(frames.clone()),
            record: None,
            snap: None,
            deadline_secs: 60,
            width: 100,
            height: 30,
        },
    );
    assert_eq!(code, 0, "the drive script ran clean");
    let child = host.children_view().pop().ok_or("no child")?;
    let transcript = serde_json::to_string(&child.session.messages())?;
    assert!(transcript.contains("answered: notes.md"), "{transcript}");
    let prompted = parent.messages().into_iter().any(|message| {
        matches!(&message, AgentMessage::User { content: UserContent::Text(text), .. } if text == "notes.md")
    });
    assert!(!prompted, "the answer never became a root prompt");
    let mut dumped: Vec<_> = std::fs::read_dir(&frames)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    dumped.sort();
    let screens: Vec<String> = dumped
        .iter()
        .map(std::fs::read_to_string)
        .collect::<Result<_, _>>()?;
    let asked = screens
        .iter()
        .find(|screen| screen.contains("needs you"))
        .ok_or("no frame showed the question")?;
    assert!(
        asked
            .lines()
            .any(|row| row.contains("asks ") && row.contains("Which file name?")),
        "{asked}"
    );
    let boxed = screens
        .iter()
        .find(|screen| screen.contains("reply to writer"))
        .ok_or("no frame showed the reply box")?;
    assert!(
        boxed
            .lines()
            .any(|row| row.contains("╭") && row.contains("reply to writer: asks ")),
        "the composer's own border is the reply box: {boxed}"
    );
    Ok(())
}

/// A permission prompt the broker settled without the human (its ask timed out) closes, and only
/// its own: another call settling leaves it up. Dies with a stale prompt whose answer goes nowhere.
#[test]
fn a_prompt_closes_when_its_own_call_settles_elsewhere() -> TestResult {
    use yi_types::event::AgentEvent;
    let mut app = app();
    let (reply, answered) = std::sync::mpsc::channel();
    app.open_approval(yi_tui::AskRequest {
        title: "bash requires permission".to_owned(),
        description: "make build".to_owned(),
        grants: Vec::new(),
        reply,
        tool_call_id: Some("c1".to_owned()),
    });
    let prompt_up = |app: &mut App| -> Result<bool, Box<dyn Error>> {
        Ok(live_rows(app)?.iter().any(|row| row.contains("Allow once")))
    };
    assert!(prompt_up(&mut app)?, "{:#?}", live_rows(&mut app)?);
    let settled = |id: &str| AgentEvent::PermissionResolved {
        tool_call_id: id.to_owned(),
        allowed: false,
    };
    app.reduce_agent(settled("c2"));
    assert!(
        prompt_up(&mut app)?,
        "another call settling leaves this prompt up"
    );
    app.reduce_agent(settled("c1"));
    assert!(!prompt_up(&mut app)?, "{:#?}", live_rows(&mut app)?);
    assert!(
        matches!(
            answered.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ),
        "the waiting asker is released, not answered"
    );
    Ok(())
}
