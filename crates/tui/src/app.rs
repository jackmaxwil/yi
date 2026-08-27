use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{self, Event as CtEvent};
use ratatui::text::Line;
use serde_json::Value;
use yi_runtime::{AgentSession, ChildStatus, SubagentHost};
use yi_types::entry::Entry;
use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, Content, StopReason, UserContent};

use crate::approval::{ApprovalView, AskChoice};
use crate::cell::{Cell, TaskCell, TaskStatus, ToolCell, ToolStatus, TranscriptMode};
use crate::colors::{Theme, detect_dark, detect_tier};
use crate::composer::Composer;
use crate::focus::set_focus;
use crate::frame::FrameScheduler;
use crate::hud::{BoardCard, CardKind, CardStatus, GoalView, HudInput};
use crate::input::handle_terminal_event;
use crate::keymap::{Keymap, default_keymap};
use crate::orb::{self, OrbState};
use crate::popup::ListPopup;
use crate::term;
use crate::tree::{TreeFilter, TreeView};

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
    Shutdown,
}

pub(crate) enum Bottom {
    Approval(ApprovalView, Sender<AskChoice>),
    Command(ListPopup),
    File(ListPopup),
}

pub struct TuiOptions {
    pub model_label: String,
    pub session_name: String,
    pub cwd: String,
    pub context_window: u64,
    pub session_dir: String,
    pub keys: Vec<(String, String)>,
    pub initial_prompt: Option<String>,
}

// U2: the viewport starts at the height of an empty live region (composer
// box + status row) anchored at the cursor, and grows upward from there —
// codex's model. Starting tall would anchor the composer mid-screen until the
// first history commit pushed it down.
const MIN_VIEWPORT_ROWS: u16 = 4;

/// Floor for the live tail on a very short screen.
const LIVE_TAIL_MIN: usize = 6;

/// Rows of the streaming answer the live region shows. A markdown table has
/// no blank line inside it, so nothing commits until the message ends and the
/// whole table sits in the live region — a fixed six-row tail cut its head off
/// mid-stream and read as clipped prose. Half the screen, the same share the
/// tree panel takes, keeps the viewport off the `insert_before` whole-screen
/// path.
pub fn live_tail_rows(rows: usize) -> usize {
    (rows / 2).max(LIVE_TAIL_MIN)
}

/// The rows of a streaming block the live region can show on a `rows`-tall
/// screen, taken from the end.
pub fn live_tail(lines: Vec<Line<'static>>, rows: usize) -> Vec<Line<'static>> {
    let skip = lines.len().saturating_sub(live_tail_rows(rows));
    lines.into_iter().skip(skip).collect()
}

const SPINNER_PERIOD_MS: u128 = 80;
pub(crate) const ORB_COLS: u16 = 6;
pub(crate) const ORB_ROWS: u16 = 3;
pub(crate) const ORB_PX: usize = 192;
pub(crate) const SLASH_COMMANDS: [&str; 5] = ["new", "quit", "expand", "tree", "editor"];

pub struct TaskState {
    pub(crate) cell: TaskCell,
    pub(crate) started: Instant,
    pub(crate) subscribed: bool,
    pub(crate) session: Arc<AgentSession>,
}

pub struct App {
    pub(crate) theme: Theme,
    pub(crate) keymap: Keymap,
    pub(crate) scheduler: FrameScheduler,
    pub(crate) composer: Composer,
    pub(crate) bottom: Option<Bottom>,
    pub(crate) tree: Option<TreeView>,
    pub(crate) pending_commit: Vec<Line<'static>>,
    pub(crate) pending_open_tree: bool,
    pub(crate) pending_rewind: Option<String>,
    pub(crate) pending_new: bool,
    pub(crate) pending_editor: bool,
    pub(crate) pending_prompt_mark: bool,
    /// U34: 0 = the `Yi` wordmark at rest, 1 = the working orb. The dots
    /// travel between the two; there is one orb, never a static one beside a
    /// moving one.
    pub(crate) logo_phase: f64,
    pub(crate) logo_target: f64,
    pub(crate) history: crate::history::History,
    pub(crate) live_markdown: String,
    pub(crate) live_thought: String,
    pub(crate) live_cut: usize,
    pub(crate) live_tools: Vec<ToolCell>,
    pub(crate) last_finished_tool: Option<ToolCell>,
    tool_started: HashMap<String, Instant>,
    pub(crate) tasks: HashMap<String, TaskState>,
    pub(crate) task_order: Vec<String>,
    committed_tasks: HashSet<String>,
    pub(crate) steering: Vec<String>,
    pub(crate) running: bool,
    pub(crate) intent: Option<String>,
    pub(crate) esc_armed_at: Option<Instant>,
    pub(crate) last_esc_at: Option<Instant>,
    pub(crate) ctrl_c_at: Option<Instant>,
    pub(crate) focused: Option<String>,
    pub(crate) hud_hidden: bool,
    seen_turn: bool,
    user_turns: usize,
    pub(crate) mode: TranscriptMode,
    pub(crate) kitty: bool,
    pub(crate) orb_placement: Option<(u16, u16)>,
    pub(crate) pending_title: Option<String>,
    pub(crate) started_at: Instant,
    pub(crate) quit: bool,
    exit_code: i32,
    pub(crate) options: TuiOptions,
    pub(crate) context_used: u64,
    pub(crate) cost_total: f64,
    pub(crate) width: usize,
    pub(crate) rows: usize,
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
        "grep" | "glob" | "find" => args
            .get("pattern")
            .or_else(|| args.get("query"))
            .or_else(|| args.get("glob")),
        "ipython" => args.get("code"),
        "fetch" | "web_search" => args.get("url").or_else(|| args.get("query")),
        _ => None,
    };
    match arg.and_then(Value::as_str) {
        Some(text) => {
            let first = text.lines().next().unwrap_or("");
            let mut text = first.split_whitespace().collect::<Vec<_>>().join(" ");
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

pub(crate) fn elapsed_ms(since: Instant) -> u64 {
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
            pending_rewind: None,
            pending_new: false,
            pending_editor: false,
            pending_prompt_mark: false,
            logo_phase: 0.0,
            logo_target: 0.0,
            history: crate::history::History::default(),
            live_markdown: String::new(),
            live_thought: String::new(),
            live_cut: 0,
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
            user_turns: 0,
            mode: TranscriptMode::Normal,
            kitty: false,
            orb_placement: None,
            pending_title: Some("Yi".to_owned()),
            started_at: Instant::now(),
            quit: false,
            exit_code: 0,
            options,
            context_used: 0,
            cost_total: 0.0,
            width,
            rows: 24,
        };
        app.scheduler.request();
        app
    }

    pub fn take_commits(&mut self) -> Vec<Line<'static>> {
        std::mem::take(&mut self.pending_commit)
    }

    pub fn focused(&self) -> Option<&str> {
        self.focused.as_deref()
    }

    pub fn set_rows(&mut self, rows: usize) {
        self.rows = rows;
    }

    pub fn set_width(&mut self, width: usize) {
        self.width = width;
    }

    pub fn logo_target(&self) -> f64 {
        self.logo_target
    }

    pub fn set_kitty(&mut self, kitty: bool) {
        self.kitty = kitty;
    }

    pub fn composer_text(&self) -> String {
        self.composer.text()
    }

    /// U18 entry point for surfaces other than the keymap (`/editor`, tests).
    pub fn open_editor(&mut self) {
        self.pending_editor = true;
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

    fn content_width(&self) -> usize {
        self.width.saturating_sub(2)
    }

    pub fn commit_cell(&mut self, cell: &Cell) {
        if matches!(cell, Cell::User { .. }) {
            self.pending_prompt_mark = true;
        }
        let spinner = self.spinner_phase();
        let width = self.content_width();
        self.pending_commit
            .extend(cell.lines(width, &self.theme, self.mode, spinner));
        self.retain(cell.clone());
        self.scheduler.request();
    }

    fn retain(&mut self, cell: Cell) {
        self.history.retain(cell);
    }

    /// The retained transcript re-rendered, as the resize repaint draws the
    /// rows above the viewport.
    pub fn reflowed(&self, rows: usize) -> Vec<Line<'static>> {
        self.history
            .lines(self.content_width(), &self.theme, self.mode, rows)
    }

    /// Drops everything the screen was showing so a rewound branch can be
    /// replayed onto a clean transcript.
    pub(crate) fn reset_transcript(&mut self) {
        self.history.clear();
        self.pending_commit.clear();
        self.live_markdown.clear();
        self.live_thought.clear();
        self.live_cut = 0;
        self.live_tools.clear();
    }

    pub fn take_title(&mut self) -> Option<String> {
        self.pending_title.take()
    }

    /// Activity → reference orb state (D41/A.13): the verb the agent is doing.
    pub fn orb_state(&self) -> Option<OrbState> {
        if matches!(self.bottom, Some(Bottom::Approval(..))) {
            return Some(OrbState::Listening);
        }
        if !self.running {
            return None;
        }
        if let Some(tool) = self
            .live_tools
            .iter()
            .rev()
            .find(|t| t.status == ToolStatus::Running)
        {
            return Some(match tool.name.as_str() {
                "grep" | "glob" | "find" | "web_search" | "fetch" => OrbState::Searching,
                "edit" | "write" => OrbState::Solving,
                _ => OrbState::Working,
            });
        }
        if self
            .tasks
            .values()
            .any(|s| s.cell.status == TaskStatus::Running)
        {
            return Some(OrbState::Connecting);
        }
        if !self.live_markdown.is_empty() {
            return Some(OrbState::Composing);
        }
        Some(OrbState::Working)
    }

    /// U13 streaming: each newly stable slice (blank-line boundary outside
    /// code fences) renders standalone and commits; a byte cursor tracks what
    /// has been committed. Slices never re-render, so trailing-blank trimming
    /// in the renderer cannot misalign the committed count (the duplicated
    /// list bug), and only the unstable tail repaints in the live region.
    fn commit_stable_prefix(&mut self) {
        let cut = crate::markdown::stable_cut(&self.live_markdown);
        if cut <= self.live_cut {
            return;
        }
        let slice = self
            .live_markdown
            .get(self.live_cut..cut)
            .unwrap_or_default()
            .to_owned();
        let width = self.content_width();
        let first = self.live_cut == 0;
        let rendered = crate::markdown::render(
            &slice,
            width.saturating_sub(crate::cell::GUTTER.len()),
            &self.theme,
        );
        if !rendered.is_empty() {
            self.pending_commit.push(Line::default());
            self.pending_commit
                .extend(crate::cell::gutter(rendered, first, &self.theme));
            self.retain(Cell::Assistant { markdown: slice });
        }
        self.live_cut = cut;
    }

    pub(crate) fn spinner_phase(&self) -> usize {
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
                let text = user_text(&content);
                if self.user_turns > 0 {
                    self.commit_cell(&Cell::Divider);
                }
                self.user_turns += 1;
                let focus: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
                let focus: String = focus.chars().take(40).collect();
                if !focus.is_empty() {
                    self.pending_title = Some(format!("Yi — {focus}"));
                }
                let cell = Cell::User { text };
                self.commit_cell(&cell);
            }
            AgentEvent::MessageUpdate {
                message: AgentMessage::Assistant { content, .. },
                ..
            } => {
                self.live_markdown = text_of(&content);
                self.live_thought = thinking_of(&content);
                self.commit_stable_prefix();
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
                    let remainder = text.get(self.live_cut..).unwrap_or_default().to_owned();
                    let width = self.content_width();
                    let first = self.live_cut == 0;
                    let rendered = crate::markdown::render(
                        &remainder,
                        width.saturating_sub(crate::cell::GUTTER.len()),
                        &self.theme,
                    );
                    if !rendered.is_empty() {
                        self.pending_commit.push(Line::default());
                        self.pending_commit.extend(crate::cell::gutter(
                            rendered,
                            first,
                            &self.theme,
                        ));
                        self.retain(Cell::Assistant {
                            markdown: remainder,
                        });
                    }
                    self.scheduler.request();
                }
                self.live_markdown.clear();
                self.live_thought.clear();
                self.live_cut = 0;
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
    let height = MIN_VIEWPORT_ROWS.min(rows.saturating_sub(1)).max(1);
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
    app.set_rows(usize::from(rows));
    app.kitty = orb::kitty::supported();

    replay_session(&mut app, &session);
    if let Some(prompt) = app.options.initial_prompt.clone() {
        let _ = cmd_tx.send(Command::Prompt(prompt));
    }

    let mut last_roster = Instant::now();
    let mut orb_shown = false;
    let mut orb_at: Option<(u16, u16)> = None;
    let mut last_orb = Instant::now() - Duration::from_secs(1);
    while !app.quit {
        let timeout = app.scheduler.poll_timeout(Instant::now());
        let orb_moving = app.kitty
            && app.orb_placement.is_some()
            && (app.logo_target > 0.0 || app.logo_phase > 0.0);
        let timeout = if orb_moving {
            timeout.min(Duration::from_millis(33))
        } else if app.running {
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
        crate::rewind::process_pending_rewind(&mut app, &mut terminal, &session);
        crate::rewind::process_pending_new(&mut app, &mut terminal, &session);
        crate::editor::process_pending_editor(&mut app, &mut terminal, true);
        if app.scheduler.should_draw(Instant::now()) {
            let start = Instant::now();
            crate::render::draw(&mut app, &mut terminal, Some(&session));
            app.scheduler.mark_drawn(start, Instant::now());
        }
        if app.kitty {
            // U34: one image. The phase walks toward its target every frame, so
            // the dots visibly travel between the `Yi` mark and the orb; a
            // settled phase at rest needs no repaint at all.
            let animating = app.logo_phase != app.logo_target || app.logo_target > 0.0;
            let due =
                animating && last_orb.elapsed() >= Duration::from_millis(crate::logo::FRAME_MS);
            if !animating {
                last_orb = Instant::now();
            }
            match app.orb_placement {
                Some((col, row)) if due || !orb_shown || orb_at != Some((col, row)) => {
                    if due {
                        app.logo_phase = crate::logo::advance(
                            app.logo_phase,
                            app.logo_target,
                            last_orb.elapsed(),
                        );
                        last_orb = Instant::now();
                    }
                    let clock = app.started_at.elapsed().as_secs_f64();
                    if let Some(frame) = crate::logo::frame(app.logo_phase, clock, 64) {
                        let rgba = orb::kitty::paint_rgba(&frame, 64.0, ORB_PX);
                        let _ = orb::kitty::emit(
                            terminal.backend_mut(),
                            &rgba,
                            ORB_PX,
                            col,
                            row,
                            ORB_COLS,
                            ORB_ROWS,
                        );
                        orb_shown = true;
                        orb_at = Some((col, row));
                    }
                }
                None if orb_shown => {
                    orb_shown = false;
                    let _ = orb::kitty::delete(terminal.backend_mut());
                }
                _ => {}
            }
        }
    }

    if orb_shown {
        let _ = orb::kitty::delete(terminal.backend_mut());
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

/// The entries on the active branch only. `entries_of` returns the whole tree
/// because the tree view draws every branch; a transcript must show the one
/// the session is actually on, or a rewind leaves the abandoned turns on screen.
pub(crate) fn branch_of(session: &AgentSession) -> Vec<Entry> {
    let Some(store) = session.store() else {
        return Vec::new();
    };
    yi_runtime::session_store::lock_session(&store)
        .find_entries_on_branch(
            "main",
            &yi_runtime::session_store::EntryQuery {
                order: yi_runtime::session_store::EntryOrder::OldestFirst,
                ..yi_runtime::session_store::EntryQuery::default()
            },
            &yi_runtime::session_store::BranchBounds::default(),
        )
        .unwrap_or_default()
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
    replay_session(app, &session);
}

/// A resumed session (`yi --session <id>`) opens on its own transcript; the
/// model's context and the screen must agree about what was said.
pub(crate) fn replay_session(app: &mut App, session: &AgentSession) {
    let entries = branch_of(session);
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
