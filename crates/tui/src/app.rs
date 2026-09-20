use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{self, Event as CtEvent};
use ratatui::text::Line;
use serde_json::{Value, json};
use yi_runtime::{AgentSession, ChildUpdate, SubagentHost};
use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, Attribution, StopReason};

use crate::approval::{ApprovalView, AskChoice};
use crate::cell::{Cell, TaskCell, TaskStatus, ToolCell, ToolStatus, TranscriptMode};
use crate::colors::{Theme, detect_dark, detect_tier};
use crate::composer::Composer;
use crate::focus::set_focus;
use crate::frame::FrameScheduler;
use crate::input::handle_terminal_event;
use crate::keymap::{Keymap, default_keymap};
use crate::motion::elapsed_ms;
use crate::orb;
use crate::popup::ListPopup;
use crate::port::SessionPort;
use crate::term;
use crate::transcript::{arg_summary, intent_of, preview_lines, text_of, thinking_of, user_text};
use crate::tree::TreeView;
use yi_orb::OrbState;

pub struct AskRequest {
    pub title: String,
    pub description: String,
    pub grants: Vec<String>,
    pub reply: Sender<AskChoice>,
}

pub enum UiEvent {
    Agent(AgentEvent),
    Child { child_id: String, event: AgentEvent },
    Children(Vec<yi_runtime::ChildView>),
    ChildUpdates(Vec<ChildUpdate>),
    Ask(AskRequest),
    Reply(crate::port::Reply),
}

pub enum Command {
    Prompt(String),
    Steer(String),
    /// E1: the rewind itself is synchronous on the render thread; the
    /// summarizer call it earns is not.
    SummarizeBranch(yi_runtime::BranchStub),
    Slash(String),
    StopChild(String),
    ChildHistory(String),
    Abort,
    Shutdown,
}

pub(crate) enum Bottom {
    Approval(ApprovalView, Sender<AskChoice>),
    Command(ListPopup),
    File(ListPopup),
    Agents(crate::agents::AgentsPopup),
    Model(Box<crate::model::ModelPopup>),
}

pub type CommandSender = tokio::sync::mpsc::UnboundedSender<Command>;
pub type CommandReceiver = tokio::sync::mpsc::UnboundedReceiver<Command>;

pub fn command_channel() -> (CommandSender, CommandReceiver) {
    tokio::sync::mpsc::unbounded_channel()
}

pub struct TuiOptions {
    pub model: yi_types::model::Model,
    pub session_name: String,
    pub cwd: String,
    pub lane: Option<String>,
    pub context_window: u64,
    pub session_dir: String,
    pub keys: Vec<(String, String)>,
    pub initial_prompt: Option<String>,
    /// The streamed reveal's speed as a percentage of the default; 0 paints text on arrival.
    pub pace: u16,
}

// U2: starting tall anchors the composer mid-screen until the first commit pushes it down,
// so the viewport opens at an empty live region's height and grows upward from the cursor.
const MIN_VIEWPORT_ROWS: u16 = 4;

pub(crate) const SPINNER_PERIOD_MS: u128 = 80;
pub(crate) const ORB_COLS: u16 = 6;
pub(crate) const ORB_ROWS: u16 = 3;
pub(crate) const ORB_PX: usize = 192;
pub struct TaskState {
    pub(crate) cell: TaskCell,
    pub(crate) streaming: Option<AgentMessage>,
    pub(crate) started: Instant,
    /// When the child reached a terminal state, so the strike can sweep.
    pub(crate) finished: Option<Instant>,
    /// The call id of the kernel cell running when the child appeared, if one was.
    pub(crate) born_under: Option<String>,
    pub(crate) session: Option<Arc<AgentSession>>,
}

pub struct App {
    pub(crate) theme: Theme,
    pub(crate) keymap: Keymap,
    pub(crate) scheduler: FrameScheduler,
    pub(crate) composer: Composer,
    pub(crate) bottom: Option<Bottom>,
    pub(crate) tree: Option<TreeView>,
    pub(crate) plan_tree: Option<crate::plantree::PlanTreeView>,
    pub(crate) plan_progress: Option<crate::hud::PlanProgress>,
    pub(crate) todos: Option<yi_types::todo::TodoList>,
    pub(crate) todo_clock: crate::hud::TodoClock,
    pub(crate) pending_commit: Vec<Line<'static>>,
    pub(crate) pending_open_tree: bool,
    pub(crate) pending_open_plan_tree: bool,
    pub(crate) pending_rewind: Option<String>,
    pub(crate) pending_new: bool,
    pub(crate) pending_editor: bool,
    pub(crate) pending_undo: bool,
    /// A slash line the event loop runs against the session (A5 dispatch).
    pub(crate) pending_command: Option<String>,
    pub(crate) pending_slash: Option<String>,
    pub(crate) pending_clear: bool,
    pub(crate) pending_summary: Option<yi_runtime::BranchStub>,
    pub(crate) pending_stop: Option<String>,
    pub(crate) pending_focus: Option<String>,
    pub selection: crate::model::Selection,
    pending_repaint: bool,
    pub(crate) pending_prompt_mark: bool,
    /// U34: 0 = the `Yi` wordmark at rest, 1 = the working orb. The dots travel between the
    /// two; there is one orb, never a static one beside a moving one.
    pub(crate) logo_phase: f64,
    pub(crate) logo_target: f64,
    pub(crate) history: crate::history::History,
    pub(crate) reflow: crate::reflow::ReflowState,
    pub(crate) streaming: Option<AgentMessage>,
    pub(crate) live_markdown: String,
    /// Fence-open line active at `live_cut`: any slice rendered from there
    /// reopens the fence so its rows still render as code.
    pub(crate) live_reopen: Option<String>,
    /// Syntax parse state at `live_cut`: a string or comment that spans the cut
    /// keeps one colour instead of being re-lexed from the reopened fence.
    pub(crate) live_lang: Option<crate::highlight::Lang>,
    pub(crate) live_thought: String,
    pub(crate) live_cut: usize,
    pub(crate) live_spaced: bool,
    /// The byte of `live_thought` already committed to scrollback, the mirror of
    /// `live_cut` for reasoning.
    pub(crate) live_thought_cut: usize,
    pub(crate) pacing: stream::Pacing,
    pub(crate) live_tools: Vec<ToolCell>,
    /// Finished read-only calls waiting to commit as one `Explored` cell.
    pub(crate) explored: Vec<ToolCell>,
    last_commit_rows: usize,
    last_commit_blank: bool,
    tool_started: HashMap<String, Instant>,
    pub(crate) tasks: HashMap<String, TaskState>,
    pub(crate) task_order: Vec<String>,
    pub(crate) committed_tasks: HashSet<String>,
    pub(crate) steering: Vec<String>,
    pub(crate) running: bool,
    pub(crate) intent: Option<String>,
    pub(crate) esc_armed_at: Option<Instant>,
    pub(crate) last_esc_at: Option<Instant>,
    pub(crate) ctrl_c_at: Option<Instant>,
    pub(crate) focused: Option<String>,
    pub(crate) hud_hidden: bool,
    seen_turn: bool,
    /// Incident: a submitted prompt reaches the runtime thread over a channel, so the UI can
    /// look idle after Enter; a script checking only `running` opened the tree mid-write.
    submitted_turns: u64,
    started_turns: u64,
    user_turns: usize,
    pub(crate) mode: TranscriptMode,
    pub(crate) kitty: bool,
    /// Where the picker's first model row sits, set by the last paint; the logo slot column.
    pub(crate) logo_rows: Option<(u16, u16)>,
    pub(crate) orb_placement: Option<(u16, u16)>,
    /// Incident: a resize reflows the text a kitty placement scrolls with, but
    /// [`crate::terminal::Terminal::resize_viewport`] reports only viewport-rect moves.
    orb_stale: bool,
    pub(crate) pending_title: Option<String>,
    pub(crate) started_at: Instant,
    pub(crate) quit: bool,
    exit_code: i32,
    pub(crate) options: TuiOptions,
    pub(crate) status_name_hidden: bool,
    pub(crate) branch: Option<String>,
    pub(crate) landing: Option<yi_types::lane::Landing>,
    /// When the landing last arrived; the row's age is this clock, not a wire field.
    pub(crate) landing_at: Option<Instant>,
    pub(crate) context_used: u64,
    pub(crate) cost_total: f64,
    pub(crate) cost_unknown: bool,
    turn_started: Instant,
    turn_tools: u64,
    turn_tokens: crate::status::TurnTokens,
    turn_cost: f64,
    pub(crate) width: usize,
    pub(crate) rows: usize,
}

pub use crate::frame::next_spinner_wake;

mod bridge;
mod children;
mod stream;

pub use bridge::forward;
pub(crate) use bridge::spawn_runtime_bridge;

impl App {
    pub fn new(options: TuiOptions, theme: Theme, keymap: Keymap, width: usize) -> Self {
        let mut app = Self {
            theme,
            keymap,
            scheduler: FrameScheduler::default(),
            composer: Composer::default(),
            bottom: None,
            tree: None,
            plan_tree: None,
            plan_progress: None,
            todos: None,
            todo_clock: crate::hud::TodoClock::default(),
            pending_commit: Vec::new(),
            pending_open_tree: false,
            pending_open_plan_tree: false,
            pending_rewind: None,
            pending_new: false,
            pending_editor: false,
            pending_undo: false,
            pending_command: None,
            pending_slash: None,
            pending_clear: false,
            pending_summary: None,
            pending_stop: None,
            pending_focus: None,
            selection: crate::model::Selection::new(options.model.clone()),
            pending_repaint: false,
            pending_prompt_mark: false,
            logo_phase: 0.0,
            logo_target: 0.0,
            history: crate::history::History::default(),
            reflow: crate::reflow::ReflowState::default(),
            streaming: None,
            live_markdown: String::new(),
            live_reopen: None,
            live_lang: None,
            live_spaced: true,
            live_thought: String::new(),
            live_cut: 0,
            live_thought_cut: 0,
            pacing: stream::Pacing::new(options.pace),
            live_tools: Vec::new(),
            explored: Vec::new(),
            last_commit_rows: 0,
            last_commit_blank: false,
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
            submitted_turns: 0,
            started_turns: 0,
            user_turns: 0,
            mode: TranscriptMode::default(),
            kitty: false,
            logo_rows: None,
            orb_placement: None,
            orb_stale: false,
            pending_title: Some("Yi".to_owned()),
            started_at: Instant::now(),
            quit: false,
            exit_code: 0,
            options,
            status_name_hidden: false,
            branch: None,
            landing: None,
            landing_at: None,
            context_used: 0,
            cost_total: 0.0,
            cost_unknown: false,
            turn_started: Instant::now(),
            turn_tools: 0,
            turn_tokens: crate::status::TurnTokens::default(),
            turn_cost: 0.0,
            width,
            rows: 24,
        };
        app.branch = crate::port::git_branch(&app.options.cwd);
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

    /// A prompt has been submitted whose turn has not started yet.
    pub fn awaiting_turn(&self) -> bool {
        self.submitted_turns > self.started_turns
    }

    pub(crate) fn note_submission(&mut self) {
        self.submitted_turns = self.submitted_turns.saturating_add(1);
    }

    pub fn open_approval(&mut self, ask: AskRequest) {
        self.bottom = Some(Bottom::Approval(
            ApprovalView::new(ask.title, ask.description, ask.grants),
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

    fn reset_live(&mut self) {
        self.live_markdown.clear();
        self.live_thought.clear();
        self.live_cut = 0;
        self.live_thought_cut = 0;
        self.live_reopen = None;
        self.live_lang = None;
        self.live_spaced = true;
        self.pacing.reset();
    }

    pub(crate) fn content_width(&self) -> usize {
        self.width.saturating_sub(2)
    }

    /// The mode decides how every cell renders, including those above the viewport, so the
    /// change repaints them rather than reaching only cells committed after it.
    pub fn cycle_mode(&mut self) {
        self.mode = self.mode.next();
        self.pending_repaint = true;
        self.scheduler.request();
    }

    pub(crate) fn mark_orb_stale(&mut self) {
        self.orb_stale = true;
    }

    pub fn take_orb_stale(&mut self) -> bool {
        std::mem::take(&mut self.orb_stale)
    }

    fn note_context(&mut self, message: &AgentMessage) {
        if let AgentMessage::Assistant { usage, .. } = message {
            self.context_used = u64::try_from(usage.total_tokens).unwrap_or(0);
            self.branch = crate::port::git_branch(&self.options.cwd);
        }
    }

    pub fn take_pending_clear(&mut self) -> bool {
        std::mem::take(&mut self.pending_clear)
    }

    pub fn take_quit(&mut self) -> bool {
        std::mem::take(&mut self.quit)
    }

    pub fn bottom_open(&self) -> bool {
        self.bottom.is_some()
    }

    pub fn set_session_name(&mut self, name: String) {
        self.options.session_name = name;
        self.scheduler.request();
    }

    pub fn set_context_window(&mut self, size: u64) {
        self.options.context_window = size;
    }

    /// The HUD's plan and todo state, read from whichever port hosts this chat; the console
    /// paints through `paint_pane`, which never saw the port, so its HUD stayed empty.
    pub fn sync_port(&mut self, port: Option<&dyn SessionPort>) {
        self.plan_progress = port.and_then(|port| port.plan_progress());
        self.todos = port.and_then(|port| port.todo_list());
        // ponytail: an idle console pane folds its list on the next event, not at the deadline.
        self.todo_clock.observe(self.todos.as_ref(), Instant::now());
    }

    pub fn mode(&self) -> TranscriptMode {
        self.mode
    }

    pub fn take_pending_repaint(&mut self) -> bool {
        std::mem::take(&mut self.pending_repaint)
    }

    pub fn commit_cell(&mut self, cell: &Cell) {
        if let Cell::Tool(tool) = cell
            && tool.status == ToolStatus::Done
            && crate::cell::explore_verb(&tool.name).is_some()
        {
            self.explored.push(tool.clone());
            self.scheduler.request();
            return;
        }
        self.flush_explored();
        self.write_cell(cell);
    }

    /// `Awaiting` while the permission gate holds the call.
    pub fn live_tool_status(&self, tool_call_id: &str) -> Option<ToolStatus> {
        self.live_tools
            .iter()
            .find(|tool| tool.call_id == tool_call_id)
            .map(|tool| tool.status)
    }

    /// The call id is the only handle a permission event carries.
    fn set_awaiting(&mut self, tool_call_id: &str, status: ToolStatus) {
        if let Some(cell) = self
            .live_tools
            .iter_mut()
            .find(|tool| tool.call_id == tool_call_id)
        {
            cell.status = status;
            self.scheduler.request();
        }
    }

    /// A run of read-only calls closes when anything else is said.
    pub(crate) fn flush_explored(&mut self) {
        if self.explored.is_empty() {
            return;
        }
        let rows = std::mem::take(&mut self.explored);
        let cell = match <[ToolCell; 1]>::try_from(rows) {
            Ok([single]) => Cell::Tool(single),
            Err(rows) => Cell::Explored(rows),
        };
        self.write_cell(&cell);
    }

    fn write_cell(&mut self, cell: &Cell) {
        if matches!(cell, Cell::User { .. }) {
            self.pending_prompt_mark = true;
        }
        let spinner = self.spinner_phase();
        let width = self.content_width();
        let mut lines = cell.lines(width, &self.theme, self.mode, spinner);
        let blank = |line: &Line<'_>| line.spans.iter().all(|s| s.content.trim().is_empty());
        // A blank separates blocks, never a run of one-line calls, measured
        // from what the previous cell rendered — all an append path knows.
        let leads_blank = lines.first().is_some_and(blank);
        if leads_blank && self.last_commit_blank {
            lines.remove(0);
        }
        if self.last_commit_rows > 1 && lines.len() > 1 && !leads_blank && !self.last_commit_blank {
            self.pending_commit.push(Line::default());
        }
        self.last_commit_rows = lines.len();
        self.last_commit_blank = lines.last().is_some_and(blank);
        self.pending_commit.extend(lines);
        self.retain(cell.clone());
        self.scheduler.request();
    }

    pub(crate) fn retain(&mut self, cell: Cell) {
        self.history.retain(cell);
    }

    pub fn reflowed(&self, rows: usize) -> Vec<Line<'static>> {
        self.history
            .lines(self.content_width(), &self.theme, self.mode, rows)
    }

    pub(crate) fn reset_transcript(&mut self) {
        self.history.clear();
        self.pending_commit.clear();
        self.reset_live();
        self.live_tools.clear();
    }

    pub fn take_title(&mut self) -> Option<String> {
        self.pending_title.take()
    }

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

    pub(crate) fn spinner_phase(&self) -> usize {
        usize::try_from(self.started_at.elapsed().as_millis() / SPINNER_PERIOD_MS).unwrap_or(0)
    }

    pub fn reduce_agent(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::AgentStart => self.start_turn(),
            AgentEvent::AgentEnd { .. } => {
                self.running = false;
                self.intent = None;
                self.esc_armed_at = None;
                self.steering.clear();
                self.live_tools.clear();
                self.flush_explored();
                self.commit_finished_tasks();
                self.commit_turn_footer();
                self.scheduler.request();
            }
            AgentEvent::MessageStart { message } => {
                if let AgentMessage::Assistant { .. } = &message {
                    return self.streaming = Some(message);
                }
                let attribution = message.attribution();
                let AgentMessage::User { content, .. } = message else {
                    return;
                };
                let text = user_text(&content);
                match attribution {
                    Attribution::Unproven => {
                        let carded = text.starts_with("[subagent ")
                            && self
                                .tasks
                                .keys()
                                .any(|id| text.contains(&format!("({id})")));
                        if !carded {
                            self.commit_cell(&Cell::Notice { text });
                        }
                    }
                    Attribution::User => {
                        if self.user_turns > 0 {
                            self.commit_cell(&Cell::Divider);
                        }
                        self.user_turns += 1;
                        let mut focus = text.split_whitespace().collect::<Vec<_>>().join(" ");
                        if focus.chars().count() > 40 {
                            focus = focus.chars().take(39).collect::<String>() + "…";
                        }
                        if !focus.is_empty() {
                            self.pending_title = Some(format!("Yi — {focus}"));
                        }
                        self.commit_cell(&Cell::User { text });
                    }
                }
            }
            AgentEvent::MessageUpdate {
                assistant_message_event,
            } => self.fold_stream(&assistant_message_event),
            AgentEvent::MessageEnd { message } => {
                self.streaming = None;
                self.note_context(&message);
                self.reduce_message_end(&message);
            }
            AgentEvent::ChildUpdate { update } => self.reduce_child_update(&update),
            AgentEvent::LandingState { landing } => {
                self.landing = Some(landing);
                self.landing_at = Some(Instant::now());
            }
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
                    call_id: tool_call_id.clone(),
                    intent: self.intent.clone(),
                    status: ToolStatus::Running,
                    summary: ToolCell::summary_of(&tool_name, &arg_summary(&tool_name, &args)),
                    digest: None,
                    preview: Vec::new(),
                    elapsed_ms: 0,
                    calls: 1,
                    // The result carries the source too, but a running cell has
                    // no result yet and its head is built from the same record.
                    details: json!({ "code": args.get("code").unwrap_or(&Value::Null) }),
                });
                self.scheduler.request();
            }
            AgentEvent::ToolExecutionEnd {
                tool_call_id,
                tool_name,
                result,
                is_error,
            } => {
                self.tool_ended(tool_call_id, tool_name, &result, is_error);
            }
            // The waiting call is coloured, not only the prompt, so the row
            // the user is being asked about says so in place.
            AgentEvent::PermissionRequested { tool_call_id, .. } => {
                self.set_awaiting(&tool_call_id, ToolStatus::Awaiting);
            }
            AgentEvent::PermissionResolved { tool_call_id, .. } => {
                self.set_awaiting(&tool_call_id, ToolStatus::Running);
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
                usage,
                ..
            } => {
                self.cost_total += usage.cost.total.as_f64().unwrap_or(0.0);
                self.cost_unknown |= usage.unknown;
                self.turn_tokens.record(usage);
                self.turn_cost += usage.cost.total.as_f64().unwrap_or(0.0);
                self.live_thought = thinking_of(content);
                self.flush_thought();
                self.live_markdown = text_of(content);
                self.commit_prose(self.live_markdown.len(), true);
                self.scheduler.request();
                self.reset_live();
                if *stop_reason == StopReason::Error {
                    let text = error_message
                        .clone()
                        .unwrap_or_else(|| "provider error".to_owned());
                    self.commit_cell(&Cell::Notice { text });
                }
            }
            // `display: false` is the model's alone; the todo prelude leaked as a dozen callouts.
            AgentMessage::Custom {
                custom_type,
                content,
                display: true,
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
        if stream::fold_child(&mut self.tasks, child_id, &event) {
            self.scheduler.request();
        }
        if let AgentEvent::ToolExecutionStart {
            tool_name, args, ..
        } = &event
            && let Some(state) = self.tasks.get_mut(child_id)
        {
            let summary = arg_summary(tool_name, args);
            state.cell.last_tool = Some(if summary.is_empty() {
                tool_name.clone()
            } else {
                format!("{tool_name} {summary}")
            });
            self.scheduler.request();
        }
        if self.focused.as_deref() != Some(child_id) {
            if let AgentEvent::MessageEnd { message } = &event
                && let AgentMessage::Assistant { usage, .. } = message
            {
                self.cost_total += usage.cost.total.as_f64().unwrap_or(0.0);
                self.cost_unknown |= usage.unknown;
            }
            return;
        }
        match event {
            AgentEvent::MessageStart {
                message: AgentMessage::User { content, .. },
            } => self.commit_cell(&Cell::User {
                text: user_text(&content),
            }),
            AgentEvent::MessageEnd { message } => self.reduce_message_end(&message),
            AgentEvent::ToolExecutionEnd {
                tool_name,
                result,
                is_error,
                ..
            } => {
                let text = text_of(&result.content);
                let cell = Cell::Tool(ToolCell {
                    name: tool_name.clone(),
                    call_id: String::new(),
                    intent: None,
                    status: ToolStatus::Done,
                    summary: ToolCell::summary_of(&tool_name, ""),
                    digest: ToolCell::digest_of(&tool_name, &text, is_error),
                    preview: preview_lines(&text, 12, 6),
                    elapsed_ms: 0,
                    calls: 1,
                    details: result.details.clone(),
                });
                self.commit_cell(&cell);
            }
            _ => {}
        }
    }

    fn start_turn(&mut self) {
        self.running = true;
        self.seen_turn = true;
        self.started_turns = self.started_turns.saturating_add(1);
        self.esc_armed_at = None;
        self.turn_started = Instant::now();
        self.turn_tools = 0;
        self.turn_tokens = crate::status::TurnTokens::default();
        self.turn_cost = 0.0;
    }

    /// One dim row closes a turn with what it cost, so the price of an answer is read
    /// where the answer is, not only in the status bar.
    fn commit_turn_footer(&mut self) {
        let crate::status::TurnTokens {
            input,
            output,
            cached,
        } = self.turn_tokens;
        if self.turn_tools == 0 && input == 0 && output == 0 {
            return;
        }
        let tools = usize::try_from(self.turn_tools).unwrap_or(usize::MAX);
        let mut text = format!(
            "{} · {} · {} in / {} out",
            crate::cell::count_label(tools, "tool"),
            crate::cell::elapsed_label(elapsed_ms(self.turn_started)),
            crate::status::fmt_tokens(input),
            crate::status::fmt_tokens(output),
        );
        if cached > 0 {
            text.push_str(&format!(" · {}% cached", cached * 100 / input.max(1)));
        }
        if self.turn_cost > 0.0 {
            text.push_str(&format!(" · ${:.3}", self.turn_cost));
        }
        self.commit_cell(&Cell::Footer { text });
    }

    pub fn set_focus(&mut self, target: Option<String>) {
        set_focus(self, target);
    }
}

/// U7 drain-then-draw loop, synchronous: the tokio runtime is on its own thread.
pub fn run_tui(
    runtime: tokio::runtime::Runtime,
    session: Arc<AgentSession>,
    host: Arc<SubagentHost>,
    ask_rx: Receiver<AskRequest>,
    options: TuiOptions,
) -> i32 {
    let (ui_rx, cmd_tx, runtime_thread) =
        spawn_runtime_bridge(runtime, &session, &host, ask_rx, Duration::from_millis(300));
    let mut port = Arc::clone(&session);

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
    app.kitty = yi_orb::kitty::supported();

    app.load_history(&mut port);
    if let Some(prompt) = app.options.initial_prompt.clone() {
        app.note_submission();
        let _ = cmd_tx.send(Command::Prompt(prompt));
    }

    let mut orb_tick = orb::Tick::default();

    let mut logo_tick = crate::logos::Tick::default();
    let mut last_spinner_phase = usize::MAX;
    let mut last_todo_full = false;
    while !app.quit {
        let timeout = app
            .scheduler
            .poll_timeout(Instant::now())
            .min(app.reveal_wake())
            .min(app.todo_clock.wake(Instant::now()));
        let animating = app.running
            || app
                .tasks
                .values()
                .any(|state| state.cell.status == TaskStatus::Running);
        let orb_moving = app.kitty
            && app.orb_placement.is_some()
            && (app.logo_target > 0.0 || app.logo_phase > 0.0);
        let mut timeout = if orb_moving {
            timeout.min(Duration::from_millis(33))
        } else if animating {
            timeout.min(next_spinner_wake(app.started_at.elapsed().as_millis()))
        } else {
            timeout
        };
        // U36: the reflow deadline is the only thing firing after a drag stops, and a settled
        // terminal sends no events, so without a wake the rebuild waits for a keypress.
        if let Some(deadline) = app.reflow.pending_until() {
            let now = Instant::now();
            if now >= deadline {
                app.scheduler.request();
            } else {
                timeout = timeout.min(deadline.saturating_duration_since(now));
            }
        }
        if event::poll(timeout).unwrap_or(false) {
            while event::poll(Duration::ZERO).unwrap_or(false) {
                match event::read() {
                    Ok(ct_event) => handle_terminal_event(&mut app, &cmd_tx, ct_event),
                    Err(_) => break,
                }
            }
        }
        crate::port::tick(&mut app, &mut port, &ui_rx, &cmd_tx);
        // A blanket request drew every turn at the 16 ms ceiling: five frames per spinner
        // step, four identical. Tokens dirty their own frame; only animation needs a timer.
        let phase = app.spinner_phase();
        if animating && phase != last_spinner_phase {
            last_spinner_phase = phase;
            app.scheduler.request();
        }
        let todo_full = app.todo_clock.full(Instant::now());
        if todo_full != last_todo_full {
            last_todo_full = todo_full;
            app.scheduler.request();
        }
        crate::editor::process_pending_editor(&mut app, &mut terminal, true);
        if app.scheduler.should_draw(Instant::now()) {
            let start = Instant::now();
            crate::render::draw(&mut app, &mut terminal, Some(&port));
            app.scheduler.mark_drawn(start, Instant::now());
        }
        orb::tick(&mut app, terminal.backend_mut(), &mut orb_tick);
        crate::logos::tick(&app, terminal.backend_mut(), &mut logo_tick);
    }
    crate::logos::delete_all(terminal.backend_mut(), &mut logo_tick);

    if orb_tick.shown {
        let _ = yi_orb::kitty::delete(terminal.backend_mut());
    }
    let _ = cmd_tx.send(Command::Shutdown);
    drop(terminal);
    drop(guard);
    let _ = runtime_thread.join();
    app.exit_code
}
