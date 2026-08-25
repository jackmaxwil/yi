use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{self, Event as CtEvent, KeyEventKind};
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use serde_json::Value;
use yi_runtime::{AgentSession, ChildStatus, SubagentHost};
use yi_types::entry::Entry;
use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, Content, StopReason, UserContent};

use crate::approval::{ApprovalView, AskChoice};
use crate::cell::{Cell, TaskCell, TaskStatus, ToolCell, ToolStatus};
use crate::colors::{Theme, detect_dark, detect_tier};
use crate::composer::Composer;
use crate::focus::{FocusMove, focus_move, set_focus};
use crate::frame::FrameScheduler;
use crate::hud::{BoardCard, CardKind, CardStatus, GoalView, HudInput};
use crate::keymap::{
    Action, EvalContext, KeyCodeValue, KeyInput, Keymap, SingleKey, default_keymap,
};
use crate::popup::{BottomView, ListPopup, PopupResult, walk_files};
use crate::status::{StatusInput, render as render_status, working_line};
use crate::term;
use crate::tree::{TreeFilter, TreeResult, TreeView};

pub struct AskRequest {
    pub title: String,
    pub description: String,
    pub reply: Sender<AskChoice>,
}

pub enum UiEvent {
    Agent(AgentEvent),
    Child { child_id: String, event: AgentEvent },
}

pub(crate) enum Command {
    Prompt(String),
    Steer(String),
    Abort,
    Rewind(String),
    Shutdown,
}

enum Bottom {
    Approval(ApprovalView, Sender<AskChoice>),
    Command(ListPopup),
    File(ListPopup),
}

pub struct TuiOptions {
    pub model_label: String,
    pub session_name: String,
    pub cwd: String,
    pub context_window: u64,
    pub keys: Vec<(String, String)>,
    pub initial_prompt: Option<String>,
}

const VIEWPORT_ROWS: u16 = 16;
const LIVE_TAIL_ROWS: usize = 6;
const SPINNER_PERIOD_MS: u128 = 80;
const SLASH_COMMANDS: [&str; 3] = ["quit", "expand", "tree"];

pub struct TaskState {
    pub(crate) cell: TaskCell,
    pub(crate) started: Instant,
    pub(crate) subscribed: bool,
    pub(crate) session: Arc<AgentSession>,
}

pub struct App {
    theme: Theme,
    keymap: Keymap,
    pub(crate) scheduler: FrameScheduler,
    composer: Composer,
    bottom: Option<Bottom>,
    tree: Option<TreeView>,
    pending_commit: Vec<Line<'static>>,
    pending_open_tree: bool,
    live_markdown: String,
    live_thought: String,
    live_tools: Vec<ToolCell>,
    last_finished_tool: Option<ToolCell>,
    tool_started: HashMap<String, Instant>,
    pub(crate) tasks: HashMap<String, TaskState>,
    pub(crate) task_order: Vec<String>,
    committed_tasks: HashSet<String>,
    steering: Vec<String>,
    running: bool,
    intent: Option<String>,
    esc_armed_at: Option<Instant>,
    last_esc_at: Option<Instant>,
    ctrl_c_at: Option<Instant>,
    pub(crate) focused: Option<String>,
    hud_hidden: bool,
    seen_turn: bool,
    started_at: Instant,
    quit: bool,
    exit_code: i32,
    options: TuiOptions,
    context_used: u64,
    cost_total: f64,
    width: usize,
}

pub(crate) fn text_of(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn thinking_of(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Thinking { thinking, .. } => Some(thinking.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn user_text(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Blocks(blocks) => text_of(blocks),
    }
}

fn arg_summary(tool: &str, args: &Value) -> String {
    let arg = match tool {
        "bash" => args.get("cmd").or_else(|| args.get("command")),
        "read" | "edit" | "write" => args.get("path").or_else(|| args.get("file_path")),
        _ => None,
    };
    match arg.and_then(Value::as_str) {
        Some(text) => {
            let mut text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if text.chars().count() > 60 {
                text = text.chars().take(60).collect::<String>() + "…";
            }
            text
        }
        None => String::new(),
    }
}

fn intent_of(args: &Value) -> Option<String> {
    args.get("i").and_then(Value::as_str).map(str::to_owned)
}

fn elapsed_ms(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_millis()).unwrap_or(0)
}

impl App {
    pub fn new(options: TuiOptions, theme: Theme, keymap: Keymap, width: usize) -> Self {
        let mut app = Self {
            theme,
            keymap,
            scheduler: FrameScheduler::default(),
            composer: Composer::default(),
            bottom: None,
            tree: None,
            pending_commit: Vec::new(),
            pending_open_tree: false,
            live_markdown: String::new(),
            live_thought: String::new(),
            live_tools: Vec::new(),
            last_finished_tool: None,
            tool_started: HashMap::new(),
            tasks: HashMap::new(),
            task_order: Vec::new(),
            committed_tasks: HashSet::new(),
            steering: Vec::new(),
            running: false,
            intent: None,
            esc_armed_at: None,
            last_esc_at: None,
            ctrl_c_at: None,
            focused: None,
            hud_hidden: false,
            seen_turn: false,
            started_at: Instant::now(),
            quit: false,
            exit_code: 0,
            options,
            context_used: 0,
            cost_total: 0.0,
            width,
        };
        app.scheduler.request();
        let banner = Line::from(vec![
            ratatui::text::Span::styled(
                "  yi ".to_owned(),
                ratatui::style::Style::default()
                    .fg(app.theme.accent)
                    .add_modifier(ratatui::style::Modifier::BOLD),
            ),
            ratatui::text::Span::styled(
                format!(
                    "{} · 易 · {}",
                    env!("CARGO_PKG_VERSION"),
                    app.options.model_label
                ),
                app.theme.dim_style(),
            ),
        ]);
        app.pending_commit.push(Line::default());
        app.pending_commit.push(banner);
        app
    }

    pub fn take_commits(&mut self) -> Vec<Line<'static>> {
        std::mem::take(&mut self.pending_commit)
    }

    pub fn focused(&self) -> Option<&str> {
        self.focused.as_deref()
    }

    pub fn is_quit(&self) -> bool {
        self.quit
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn has_run(&self) -> bool {
        self.seen_turn
    }

    pub(crate) fn open_approval(&mut self, ask: AskRequest) {
        self.bottom = Some(Bottom::Approval(
            ApprovalView::new(ask.title, ask.description),
            ask.reply,
        ));
        self.scheduler.request();
    }

    pub(crate) fn handle_event(
        &mut self,
        cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
        ct_event: CtEvent,
    ) {
        handle_terminal_event(self, cmd_tx, ct_event);
    }

    pub fn commit_cell(&mut self, cell: &Cell) {
        let spinner = self.spinner_phase();
        self.pending_commit
            .extend(cell.lines(self.width, &self.theme, false, spinner));
        self.scheduler.request();
    }

    fn spinner_phase(&self) -> usize {
        usize::try_from(self.started_at.elapsed().as_millis() / SPINNER_PERIOD_MS).unwrap_or(0)
    }

    pub fn reduce_agent(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::AgentStart => {
                self.running = true;
                self.seen_turn = true;
                self.esc_armed_at = None;
            }
            AgentEvent::AgentEnd { .. } => {
                self.running = false;
                self.intent = None;
                self.esc_armed_at = None;
                self.steering.clear();
                self.live_tools.clear();
                self.scheduler.request();
            }
            AgentEvent::MessageStart {
                message: AgentMessage::User { content, .. },
            } => {
                let cell = Cell::User {
                    text: user_text(&content),
                };
                self.commit_cell(&cell);
            }
            AgentEvent::MessageUpdate {
                message: AgentMessage::Assistant { content, .. },
                ..
            } => {
                self.live_markdown = text_of(&content);
                self.live_thought = thinking_of(&content);
                self.scheduler.request();
            }
            AgentEvent::MessageEnd { message } => self.reduce_message_end(&message),
            AgentEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            } => {
                self.intent = intent_of(&args);
                self.tool_started
                    .insert(tool_call_id.clone(), Instant::now());
                self.live_tools.push(ToolCell {
                    name: tool_name.clone(),
                    intent: self.intent.clone(),
                    status: ToolStatus::Running,
                    summary: ToolCell::summary_of(&tool_name, &arg_summary(&tool_name, &args)),
                    preview: Vec::new(),
                    elapsed_ms: 0,
                    calls: 1,
                });
                self.scheduler.request();
            }
            AgentEvent::ToolExecutionEnd {
                tool_call_id,
                tool_name,
                result,
                is_error,
            } => {
                let elapsed = self
                    .tool_started
                    .remove(&tool_call_id)
                    .map(elapsed_ms)
                    .unwrap_or(0);
                let index = self
                    .live_tools
                    .iter()
                    .position(|tool| tool.status == ToolStatus::Running && tool.name == tool_name);
                let mut cell = match index {
                    Some(i) => self.live_tools.remove(i),
                    None => ToolCell {
                        name: tool_name.clone(),
                        intent: None,
                        status: ToolStatus::Running,
                        summary: ToolCell::summary_of(&tool_name, ""),
                        preview: Vec::new(),
                        elapsed_ms: 0,
                        calls: 1,
                    },
                };
                cell.status = if is_error {
                    ToolStatus::Failed
                } else {
                    ToolStatus::Done
                };
                cell.elapsed_ms = elapsed;
                cell.preview = text_of(&result.content)
                    .lines()
                    .take(10)
                    .map(str::to_owned)
                    .collect();
                self.commit_cell(&Cell::Tool(cell.clone()));
                self.last_finished_tool = Some(cell);
                self.intent = None;
            }
            _ => {}
        }
    }

    fn reduce_message_end(&mut self, message: &AgentMessage) {
        match message {
            AgentMessage::Assistant {
                content,
                stop_reason,
                error_message,
                ..
            } => {
                let thought = thinking_of(content);
                if !thought.is_empty() {
                    self.commit_cell(&Cell::Thought { markdown: thought });
                }
                let text = text_of(content);
                if !text.is_empty() {
                    self.commit_cell(&Cell::Assistant { markdown: text });
                }
                self.live_markdown.clear();
                self.live_thought.clear();
                if *stop_reason == StopReason::Error {
                    let text = error_message
                        .clone()
                        .unwrap_or_else(|| "provider error".to_owned());
                    self.commit_cell(&Cell::Notice { text });
                }
            }
            AgentMessage::Custom {
                custom_type,
                content,
                ..
            } => {
                let cell = Cell::Advisory {
                    source: custom_type.clone(),
                    text: user_text(content),
                };
                self.commit_cell(&cell);
            }
            _ => {}
        }
    }

    pub fn reduce_child(&mut self, child_id: &str, event: AgentEvent) {
        if let AgentEvent::ToolExecutionStart {
            tool_name, args, ..
        } = &event
            && let Some(state) = self.tasks.get_mut(child_id)
        {
            state.cell.toolcalls += 1;
            let summary = arg_summary(tool_name, args);
            state.cell.last_tool = Some(if summary.is_empty() {
                tool_name.clone()
            } else {
                format!("{tool_name} {summary}")
            });
            self.scheduler.request();
        }
        if self.focused.as_deref() != Some(child_id) {
            return;
        }
        match event {
            AgentEvent::MessageStart {
                message: AgentMessage::User { content, .. },
            } => {
                let cell = Cell::User {
                    text: user_text(&content),
                };
                self.commit_cell(&cell);
            }
            AgentEvent::MessageEnd { message } => self.reduce_message_end(&message),
            AgentEvent::ToolExecutionEnd {
                tool_name, result, ..
            } => {
                let cell = Cell::Tool(ToolCell {
                    name: tool_name.clone(),
                    intent: None,
                    status: ToolStatus::Done,
                    summary: ToolCell::summary_of(&tool_name, ""),
                    preview: text_of(&result.content)
                        .lines()
                        .take(10)
                        .map(str::to_owned)
                        .collect(),
                    elapsed_ms: 0,
                    calls: 1,
                });
                self.commit_cell(&cell);
            }
            _ => {}
        }
    }

    pub fn sync_children(&mut self, children: &[yi_runtime::ChildView]) {
        for child in children {
            if !self.tasks.contains_key(&child.child_id) {
                self.task_order.push(child.child_id.clone());
                self.tasks.insert(
                    child.child_id.clone(),
                    TaskState {
                        cell: TaskCell {
                            agent: "rlm".to_owned(),
                            child_id: child.child_id.clone(),
                            description: child.session_name.clone(),
                            status: TaskStatus::Running,
                            last_tool: None,
                            toolcalls: 0,
                            elapsed_ms: 0,
                            error: None,
                        },
                        started: Instant::now(),
                        subscribed: false,
                        session: Arc::clone(&child.session),
                    },
                );
                self.scheduler.request();
            }
            let Some(state) = self.tasks.get_mut(&child.child_id) else {
                continue;
            };
            state.subscribed = true;
            let status = match child.status {
                ChildStatus::Running => TaskStatus::Running,
                ChildStatus::Completed => TaskStatus::Done,
                ChildStatus::Error => TaskStatus::Failed,
            };
            if state.cell.status != status {
                state.cell.status = status;
                state.cell.error = child.error.clone();
                state.cell.elapsed_ms = elapsed_ms(state.started);
                self.scheduler.request();
            }
            if status != TaskStatus::Running && !self.committed_tasks.contains(&child.child_id) {
                self.committed_tasks.insert(child.child_id.clone());
                let cell = self
                    .tasks
                    .get(&child.child_id)
                    .map(|state| Cell::Task(state.cell.clone()));
                if let Some(cell) = cell {
                    self.commit_cell(&cell);
                }
            }
        }
    }

    pub fn set_focus(&mut self, target: Option<String>) {
        set_focus(self, target);
    }

    pub fn hud_input(&self, goal: Option<GoalView>) -> HudInput {
        let mut cards = Vec::new();
        let any_running = self
            .tasks
            .values()
            .any(|state| state.cell.status == TaskStatus::Running);
        for id in &self.task_order {
            let Some(state) = self.tasks.get(id) else {
                continue;
            };
            if !any_running {
                break;
            }
            let status = match state.cell.status {
                TaskStatus::Running => CardStatus::Running,
                TaskStatus::Done => CardStatus::Done,
                TaskStatus::Failed => CardStatus::Blocked,
            };
            cards.push(BoardCard {
                title: format!("{} ⟨{}⟩", state.cell.child_id, state.cell.agent),
                kind: CardKind::Subagent,
                status,
                detail: state.cell.description.clone(),
            });
        }
        HudInput {
            goal,
            cards,
            steering: self.steering.clone(),
            follow_up: Vec::new(),
        }
    }
}

type Bridge = (
    Sender<UiEvent>,
    Receiver<UiEvent>,
    tokio::sync::mpsc::UnboundedSender<Command>,
    tokio::runtime::Handle,
    std::thread::JoinHandle<()>,
);

/// The runtime lives on its own thread: events flow out over std mpsc
/// forwarders, user intents flow in over the command channel, and every
/// session call happens inside the runtime context (spawn_run needs it).
pub(crate) fn spawn_runtime_bridge(
    runtime: tokio::runtime::Runtime,
    session: &Arc<AgentSession>,
) -> Bridge {
    let (ui_tx, ui_rx) = std::sync::mpsc::channel::<UiEvent>();
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<Command>();
    let handle = runtime.handle().clone();

    let events_tx = ui_tx.clone();
    let mut events = session.subscribe();
    handle.spawn(async move {
        while let Ok(event) = events.recv().await {
            if events_tx.send(UiEvent::Agent(event)).is_err() {
                break;
            }
        }
    });

    let driver_session = Arc::clone(session);
    let runtime_thread = std::thread::spawn(move || {
        runtime.block_on(async move {
            while let Some(command) = cmd_rx.recv().await {
                match command {
                    Command::Prompt(text) => {
                        let _ = driver_session.prompt(&text);
                    }
                    Command::Steer(text) => driver_session.steer(&text),
                    Command::Abort => driver_session.abort(),
                    Command::Rewind(id) => {
                        if let Some(store) = driver_session.store() {
                            let moved = yi_runtime::session_store::lock_session(&store)
                                .move_lane("main", Some(&id));
                            if moved.is_ok() {
                                let _ = driver_session.attach_store(store);
                            }
                        }
                    }
                    Command::Shutdown => break,
                }
            }
        });
    });
    (ui_tx, ui_rx, cmd_tx, handle, runtime_thread)
}

/// U7 drain-then-draw loop on a synchronous UI thread. The tokio runtime
/// lives on its own thread; user intents go out over an unbounded command
/// channel, runtime events come back over std mpsc from forwarder tasks.
pub fn run_tui(
    runtime: tokio::runtime::Runtime,
    session: Arc<AgentSession>,
    host: Arc<SubagentHost>,
    ask_rx: Receiver<AskRequest>,
    options: TuiOptions,
) -> i32 {
    let (ui_tx, ui_rx, cmd_tx, handle, runtime_thread) = spawn_runtime_bridge(runtime, &session);

    let mut writer = match term::terminal_writer() {
        Ok(writer) => writer,
        Err(error) => {
            eprintln!("error: no terminal available: {error}");
            return 1;
        }
    };
    let guard = match term::TerminalGuard::new(&mut writer) {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!("error: failed to enter raw mode: {error}");
            return 1;
        }
    };
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let mut out = std::io::stdout();
        term::restore_terminal(&mut out);
        default_hook(info);
    }));
    let (cols, rows) = ratatui::crossterm::terminal::size().unwrap_or((80, 24));
    let height = VIEWPORT_ROWS.min(rows.saturating_sub(1)).max(6);
    let mut terminal = match term::build_terminal(writer, height) {
        Ok(terminal) => terminal,
        Err(error) => {
            eprintln!("error: failed to build terminal: {error}");
            return 1;
        }
    };

    let tier = detect_tier(
        std::env::var("COLORTERM").ok().as_deref(),
        std::env::var("TERM").ok().as_deref(),
    );
    let dark = detect_dark(std::env::var("COLORFGBG").ok().as_deref());
    let mut keymap = default_keymap();
    let overrides: Vec<(&str, &str)> = options
        .keys
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    if let Err(error) = keymap.apply_overrides(overrides) {
        eprintln!("warning: keys config: {error}");
    }

    let mut app = App::new(options, Theme::new(tier, dark), keymap, usize::from(cols));

    if let Some(prompt) = app.options.initial_prompt.clone() {
        let _ = cmd_tx.send(Command::Prompt(prompt));
    }

    let mut last_roster = Instant::now();
    while !app.quit {
        let timeout = app.scheduler.poll_timeout(Instant::now());
        let timeout = if app.running {
            timeout.min(Duration::from_millis(90))
        } else {
            timeout
        };
        if event::poll(timeout).unwrap_or(false) {
            while event::poll(Duration::ZERO).unwrap_or(false) {
                match event::read() {
                    Ok(ct_event) => handle_terminal_event(&mut app, &cmd_tx, ct_event),
                    Err(_) => break,
                }
            }
        }
        for ui_event in ui_rx.try_iter().collect::<Vec<_>>() {
            match ui_event {
                UiEvent::Agent(event) => app.reduce_agent(event),
                UiEvent::Child { child_id, event } => app.reduce_child(&child_id, event),
            }
        }
        for ask in ask_rx.try_iter().collect::<Vec<_>>() {
            app.bottom = Some(Bottom::Approval(
                ApprovalView::new(ask.title, ask.description),
                ask.reply,
            ));
            app.scheduler.request();
        }
        if last_roster.elapsed() >= Duration::from_millis(300) {
            last_roster = Instant::now();
            sync_roster(&mut app, &host, &handle, &ui_tx);
        }
        if app.running
            || app
                .tasks
                .values()
                .any(|s| s.cell.status == TaskStatus::Running)
        {
            app.scheduler.request();
        }
        if app.pending_open_tree {
            app.pending_open_tree = false;
            open_tree(&mut app, &session);
        }
        if app.scheduler.should_draw(Instant::now()) {
            let start = Instant::now();
            draw(&mut app, &mut terminal, Some(&session));
            app.scheduler.mark_drawn(start, Instant::now());
        }
    }

    let _ = cmd_tx.send(Command::Shutdown);
    drop(terminal);
    drop(guard);
    let _ = runtime_thread.join();
    app.exit_code
}

pub(crate) fn process_pending_tree(app: &mut App, session: &Arc<AgentSession>) {
    if app.pending_open_tree {
        app.pending_open_tree = false;
        open_tree(app, session);
    }
}

pub(crate) fn sync_roster(
    app: &mut App,
    host: &Arc<SubagentHost>,
    handle: &tokio::runtime::Handle,
    ui_tx: &Sender<UiEvent>,
) {
    let children = host.children_view();
    for child in &children {
        let subscribed = app
            .tasks
            .get(&child.child_id)
            .is_some_and(|state| state.subscribed);
        if !subscribed {
            let mut events = child.session.subscribe();
            let child_id = child.child_id.clone();
            let ui_tx = ui_tx.clone();
            handle.spawn(async move {
                while let Ok(event) = events.recv().await {
                    if ui_tx
                        .send(UiEvent::Child {
                            child_id: child_id.clone(),
                            event,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            });
        }
    }
    app.sync_children(&children);
}

pub(crate) fn entries_of(session: &AgentSession) -> (Vec<Entry>, Option<String>) {
    let Some(store) = session.store() else {
        return (Vec::new(), None);
    };
    let locked = yi_runtime::session_store::lock_session(&store);
    let entries = locked
        .find_entries(&yi_runtime::session_store::EntryQuery {
            order: yi_runtime::session_store::EntryOrder::OldestFirst,
            ..yi_runtime::session_store::EntryQuery::default()
        })
        .unwrap_or_default();
    let leaf = locked.leaf_id("main").ok().flatten();
    (entries, leaf)
}

fn open_tree(app: &mut App, session: &Arc<AgentSession>) {
    let (entries, leaf) = entries_of(session);
    if entries.is_empty() {
        app.commit_cell(&Cell::Notice {
            text: "no session store attached — tree unavailable".to_owned(),
        });
        return;
    }
    app.tree = Some(TreeView::new(
        &entries,
        leaf.as_deref(),
        TreeFilter::Default,
    ));
    app.scheduler.request();
}

pub(crate) fn replay_child(app: &mut App, child_id: &str) {
    let Some(session) = app.tasks.get(child_id).map(|s| Arc::clone(&s.session)) else {
        return;
    };
    let (entries, _) = entries_of(&session);
    let cells: Vec<Cell> = entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Message { message, .. } => match message {
                AgentMessage::User { content, .. } => Some(Cell::User {
                    text: user_text(content),
                }),
                AgentMessage::Assistant { content, .. } => {
                    let text = text_of(content);
                    if text.is_empty() {
                        None
                    } else {
                        Some(Cell::Assistant { markdown: text })
                    }
                }
                AgentMessage::ToolResult { tool_name, .. } => Some(Cell::Tool(ToolCell {
                    name: tool_name.clone(),
                    intent: None,
                    status: ToolStatus::Done,
                    summary: ToolCell::summary_of(tool_name, ""),
                    preview: Vec::new(),
                    elapsed_ms: 0,
                    calls: 1,
                })),
                _ => None,
            },
            _ => None,
        })
        .collect();
    for cell in cells {
        app.commit_cell(&cell);
    }
}

fn handle_terminal_event(
    app: &mut App,
    cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    ct_event: CtEvent,
) {
    match ct_event {
        CtEvent::Paste(text) => {
            app.composer.handle_paste(&text);
            app.scheduler.request();
        }
        CtEvent::Resize(cols, _) => {
            app.width = usize::from(cols);
            app.scheduler.request();
        }
        CtEvent::Key(key_event) if key_event.kind != KeyEventKind::Release => {
            let Some(key) = SingleKey::from_event(&key_event) else {
                return;
            };
            app.scheduler.request();
            if app.tree.is_some() {
                handle_tree_key(app, cmd_tx, &key);
                return;
            }
            if app.bottom.is_some() {
                handle_bottom_key(app, &key);
                return;
            }
            let ctx = EvalContext {
                input_empty: app.composer.is_empty(),
            };
            match app.keymap.resolve(&KeyInput::Single(key), &ctx) {
                Some(action) => handle_action(app, cmd_tx, action),
                None => {
                    if key.code == KeyCodeValue::Char('/')
                        && !key.ctrl
                        && !key.alt
                        && app.composer.is_empty()
                    {
                        app.bottom = Some(Bottom::Command(ListPopup::new(
                            '/',
                            SLASH_COMMANDS.iter().map(|s| (*s).to_owned()).collect(),
                        )));
                    } else if key.code == KeyCodeValue::Char('@') && !key.ctrl && !key.alt {
                        let files = walk_files(std::path::Path::new(&app.options.cwd), 100);
                        app.bottom = Some(Bottom::File(ListPopup::new('@', files)));
                    } else if key.code == KeyCodeValue::Backspace && !key.ctrl && !key.alt {
                        app.composer.backspace();
                    } else {
                        app.composer.input(key_event);
                    }
                }
            }
        }
        _ => {}
    }
}

fn handle_tree_key(
    app: &mut App,
    cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    key: &SingleKey,
) {
    let Some(tree) = app.tree.as_mut() else {
        return;
    };
    match tree.handle_key(key) {
        TreeResult::Open => {}
        TreeResult::Close => app.tree = None,
        TreeResult::Rewind(id) => {
            app.tree = None;
            let _ = cmd_tx.send(Command::Rewind(id.clone()));
            app.commit_cell(&Cell::Notice {
                text: format!("rewound to {id}"),
            });
        }
    }
}

fn handle_bottom_key(app: &mut App, key: &SingleKey) {
    let Some(mut bottom) = app.bottom.take() else {
        return;
    };
    let result = match &mut bottom {
        Bottom::Approval(view, _) => view.handle_key(key),
        Bottom::Command(popup) | Bottom::File(popup) => popup.handle_key(key),
    };
    match result {
        PopupResult::Open => app.bottom = Some(bottom),
        PopupResult::Close => {
            if let Bottom::Approval(view, reply) = bottom {
                let _ = reply.send(view.outcome.unwrap_or(AskChoice::Reject));
            }
        }
        PopupResult::Insert(text) => match bottom {
            Bottom::Command(_) => {
                let command = text.trim_start_matches('/').to_owned();
                handle_slash(app, &command);
            }
            Bottom::File(_) => {
                app.composer
                    .textarea
                    .insert_str(format!("{} ", text.trim_start_matches('@')));
            }
            Bottom::Approval(view, reply) => {
                let _ = reply.send(view.outcome.unwrap_or(AskChoice::Reject));
            }
        },
    }
}

fn handle_action(
    app: &mut App,
    cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    action: Action,
) {
    match action {
        Action::Submit => {
            if let Some(text) = app.composer.take_submission() {
                if app.running {
                    app.steering.push(text.clone());
                    let _ = cmd_tx.send(Command::Steer(text));
                } else {
                    let _ = cmd_tx.send(Command::Prompt(text));
                }
            }
        }
        Action::InsertNewline => app.composer.insert_newline(),
        Action::Abort => handle_escape(app, cmd_tx),
        Action::Quit => {
            let now = Instant::now();
            if !app.composer.is_empty() {
                app.composer.set_text("");
                return;
            }
            match app.ctrl_c_at {
                Some(at) if now.duration_since(at) < Duration::from_secs(1) => app.quit = true,
                _ => {
                    if app.running {
                        let _ = cmd_tx.send(Command::Abort);
                    }
                    app.ctrl_c_at = Some(now);
                }
            }
        }
        Action::HistoryPrev => app.composer.history_prev(),
        Action::HistoryNext => app.composer.history_next(),
        Action::ToggleExpand => {
            if let Some(cell) = app.last_finished_tool.clone() {
                let lines = Cell::Tool(cell).lines(app.width, &app.theme, true, 0);
                app.pending_commit.extend(lines);
                app.scheduler.request();
            }
        }
        Action::ToggleHud => app.hud_hidden = !app.hud_hidden,
        Action::ExternalEditor => {}
        Action::FocusChild => focus_move(app, FocusMove::Child),
        Action::FocusParent => focus_move(app, FocusMove::Parent),
        Action::FocusNextSibling => focus_move(app, FocusMove::Next),
        Action::FocusPrevSibling => focus_move(app, FocusMove::Prev),
    }
}

fn handle_slash(app: &mut App, command: &str) {
    match command {
        "quit" => app.quit = true,
        "tree" => app.pending_open_tree = true,
        "expand" => {
            if let Some(cell) = app.last_finished_tool.clone() {
                let lines = Cell::Tool(cell).lines(app.width, &app.theme, true, 0);
                app.pending_commit.extend(lines);
            }
        }
        _ => app.commit_cell(&Cell::Notice {
            text: format!("unknown command: /{command}"),
        }),
    }
    app.scheduler.request();
}

fn handle_escape(app: &mut App, cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>) {
    let now = Instant::now();
    if app.focused.is_some() {
        set_focus(app, None);
        return;
    }
    if app.running {
        match app.esc_armed_at {
            Some(at) if now.duration_since(at) < Duration::from_secs(1) => {
                let _ = cmd_tx.send(Command::Abort);
                app.esc_armed_at = None;
            }
            _ => app.esc_armed_at = Some(now),
        }
        return;
    }
    if !app.composer.is_empty() {
        return;
    }
    match app.last_esc_at {
        Some(at) if now.duration_since(at) < Duration::from_millis(500) => {
            app.last_esc_at = None;
            app.pending_open_tree = true;
        }
        _ => app.last_esc_at = Some(now),
    }
}

pub fn draw<B>(app: &mut App, terminal: &mut ratatui::Terminal<B>, session: Option<&AgentSession>)
where
    B: ratatui::backend::Backend + std::io::Write,
{
    let commits = std::mem::take(&mut app.pending_commit);
    let _ = term::commit_lines(terminal, commits);
    if let Some(usage) = session.and_then(AgentSession::last_usage) {
        app.context_used = u64::try_from(usage.total_tokens).unwrap_or(0);
        app.cost_total = usage.cost.total.as_f64().unwrap_or(0.0);
    }
    let goal = session.and_then(AgentSession::store).and_then(|store| {
        yi_runtime::session_store::lock_session(&store)
            .goal()
            .map(|goal| GoalView {
                objective: goal.objective,
                status: goal.status.as_str().to_owned(),
                tokens_used: goal.tokens_used,
                token_budget: goal.token_budget,
            })
    });
    let spinner = app.spinner_phase();
    let theme = app.theme;
    let width = app.width;

    let mut live_lines: Vec<Line<'static>> = Vec::new();
    if !app.live_markdown.is_empty() {
        let rendered = crate::markdown::render(&app.live_markdown, width, &theme);
        let skip = rendered.len().saturating_sub(LIVE_TAIL_ROWS);
        live_lines.extend(rendered.into_iter().skip(skip));
    } else if !app.live_thought.is_empty() {
        // Reasoning-heavy models stream thought long before prose; show its
        // dim tail so the screen is never silently blank mid-turn.
        let cell = Cell::Thought {
            markdown: app.live_thought.clone(),
        };
        let rendered = cell.lines(width, &theme, false, spinner);
        let skip = rendered.len().saturating_sub(LIVE_TAIL_ROWS);
        live_lines.extend(rendered.into_iter().skip(skip));
    }
    for tool in &app.live_tools {
        live_lines.extend(tool.lines(width, &theme, false, spinner));
    }
    for id in &app.task_order {
        if let Some(state) = app.tasks.get(id)
            && state.cell.status == TaskStatus::Running
        {
            let mut cell = state.cell.clone();
            cell.elapsed_ms = elapsed_ms(state.started);
            live_lines.extend(cell.lines(width, &theme, spinner));
        }
    }
    let hud_lines = if app.hud_hidden {
        Vec::new()
    } else {
        crate::hud::render(&app.hud_input(goal), &theme, spinner)
    };
    let status_input = StatusInput {
        model: app.options.model_label.clone(),
        thinking: None,
        mode: None,
        cwd: app.options.cwd.clone(),
        branch: None,
        cost: if app.cost_total > 0.0 {
            Some(format!("${:.2}", app.cost_total))
        } else {
            None
        },
        session_name: app.options.session_name.clone(),
        subagents: app
            .tasks
            .values()
            .filter(|s| s.cell.status == TaskStatus::Running)
            .count(),
        context_used: app.context_used,
        context_window: app.options.context_window,
        threshold_pct: Some(80),
        focused_child: app.focused.clone(),
    };
    let status_row = render_status(&status_input, width, &theme);
    let bottom_lines: Option<Vec<Line<'static>>> = if let Some(tree) = &app.tree {
        Some(tree.lines(width, &theme, 8))
    } else {
        match &app.bottom {
            Some(Bottom::Approval(view, _)) => Some(view.lines(width, &theme)),
            Some(Bottom::Command(popup) | Bottom::File(popup)) => Some(popup.lines(width, &theme)),
            None => None,
        }
    };
    let show_working = app.running;
    let working = working_line(
        app.intent.as_deref(),
        spinner,
        app.esc_armed_at.is_some(),
        &theme,
    );
    let border = if app.running {
        theme.dim_style()
    } else {
        theme.muted_style()
    };
    app.composer.set_frame(border, theme.dim_style());
    let composer_height = app.composer.desired_height();
    let composer = &app.composer.textarea;

    let _ = terminal.draw(|frame| {
        // The inline viewport's buffer area starts at area.y, not 0 — a rect
        // outside the area renders nowhere, silently.
        let area = frame.area();
        let mut y = area.top();
        let put = |frame: &mut ratatui::Frame, lines: &[Line<'static>], y: &mut u16| {
            let height = u16::try_from(lines.len()).unwrap_or(0);
            if height == 0 || *y >= area.bottom() {
                return;
            }
            let height = height.min(area.bottom() - *y);
            let rect = Rect::new(area.left(), *y, area.width, height);
            frame.render_widget(Paragraph::new(lines.to_vec()), rect);
            *y += height;
        };
        put(frame, &live_lines, &mut y);
        put(frame, &hud_lines, &mut y);
        if show_working {
            put(frame, std::slice::from_ref(&working), &mut y);
        }
        match &bottom_lines {
            Some(lines) => put(frame, lines, &mut y),
            None => {
                if y < area.bottom() {
                    let height = composer_height.min(area.bottom() - y);
                    let rect = Rect::new(area.left(), y, area.width, height);
                    frame.render_widget(composer, rect);
                    y += height;
                }
            }
        }
        put(frame, std::slice::from_ref(&status_row), &mut y);
    });
}
