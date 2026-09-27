//! The session port: every touch the chat makes on its session, answered now by the
//! in-process session or later by whatever hosts the chat over a wire.

use std::sync::Arc;

use yi_runtime::session_store::{
    BranchBounds, CreateOptions, EntryOrder, EntryQuery, JsonlRepo, SessionRepo, age_label,
    lock_session, now_ms,
};
use yi_runtime::{AgentSession, BranchStub};
use yi_types::entry::Entry;
use yi_types::model::{Effort, Model};
use yi_types::plan::doc::Plan;

use crate::app::App;
use crate::cell::Cell;
use crate::cell::{ToolCell, ToolStatus};
use crate::hud::GoalView;
use crate::plantree::PlanTreeView;
use crate::transcript::{text_of, user_text};
use crate::tree::{TreeFilter, TreeView};
use serde_json::Value;
use yi_types::message::AgentMessage;

pub enum Answer {
    Now(Box<Reply>),
    Later,
}

impl Answer {
    pub fn now(reply: Reply) -> Self {
        Self::Now(Box::new(reply))
    }
}

pub enum Reply {
    History(Vec<Entry>),
    Entries {
        entries: Vec<Entry>,
        leaf: Option<String>,
    },
    Rewound {
        entries: Vec<Entry>,
        unsent: Option<String>,
        abandoned: Option<BranchStub>,
    },
    Fresh {
        name: String,
    },
    Selected {
        model: Model,
        effort: Effort,
    },
    Plan {
        plan: Plan,
        subplans: Vec<Plan>,
    },
    ChildHistory {
        child_id: String,
        entries: Vec<Entry>,
    },
    Notice(String),
}

pub trait SessionPort {
    fn history(&mut self) -> Answer;
    fn entries(&mut self) -> Answer;
    fn rewind(&mut self, entry_id: &str) -> Answer;
    fn new_session(&mut self, session_dir: &str, cwd: &str) -> Answer;
    fn undo(&mut self, cwd: &str) -> Answer;
    fn slash(&mut self, line: &str, session_dir: &str, cwd: &str) -> Answer;
    fn select(&mut self, model: Model, effort: Effort) -> Answer;
    fn plan(&mut self) -> Answer;
    fn goal(&self) -> Option<GoalView>;
    /// The open plan's checklist count for the HUD; a port with no plan service shows none.
    fn plan_progress(&self) -> Option<crate::hud::PlanProgress> {
        None
    }
    /// The session's live todo list for the HUD block; a port with no store shows none.
    fn todo_list(&self) -> Option<yi_types::todo::TodoList> {
        None
    }
    fn memory(&self) -> Option<Arc<yi_runtime::memory::Activity>> {
        None
    }
}

/// The status row's branch, from the runtime's HEAD reader: a file read, never a git process.
pub(crate) fn git_branch(cwd: &str) -> Option<String> {
    yi_runtime::lane::head(std::path::Path::new(cwd))
        .ok()
        .map(|head| head.label())
}

pub(crate) fn child_of(
    host: &yi_runtime::SubagentHost,
    child_id: &str,
) -> Option<yi_runtime::ChildView> {
    host.children_view()
        .into_iter()
        .find(|child| child.update.id.as_str() == child_id)
}

/// A lane verb may wait on git or the forge, so it runs on its own thread and comes back
/// as a notice; the render thread never waits on it.
pub(crate) fn slash_off_thread(
    session: &Arc<AgentSession>,
    line: String,
    reply_tx: &std::sync::mpsc::Sender<crate::app::UiEvent>,
) {
    let session = Arc::clone(session);
    let reply_tx = reply_tx.clone();
    std::thread::spawn(move || {
        let (command, args) = line
            .split_once(char::is_whitespace)
            .map_or((line.as_str(), ""), |(head, rest)| (head, rest.trim()));
        let text = yi_runtime::slash::run(&session, command, args)
            .unwrap_or_else(|| format!("unknown command: /{command}"));
        let _ = reply_tx.send(crate::app::UiEvent::Reply(Reply::Notice(text)));
    });
}

/// The active branch only: the whole tree is for the tree view, and a transcript built
/// from it leaves rewound turns on screen.
pub fn branch_of(session: &AgentSession) -> Vec<Entry> {
    branch_in(session.store())
}

pub(crate) fn branch_in(store: Option<yi_runtime::session_store::SharedSession>) -> Vec<Entry> {
    let Some(store) = store else {
        return Vec::new();
    };
    lock_session(&store)
        .find_entries_on_branch(
            "main",
            &EntryQuery {
                order: EntryOrder::OldestFirst,
                ..EntryQuery::default()
            },
            &BranchBounds::default(),
        )
        .unwrap_or_default()
}

fn entries_of(session: &AgentSession) -> (Vec<Entry>, Option<String>) {
    let Some(store) = session.store() else {
        return (Vec::new(), None);
    };
    let locked = lock_session(&store);
    let entries = locked
        .find_entries(&EntryQuery {
            order: EntryOrder::OldestFirst,
            ..EntryQuery::default()
        })
        .unwrap_or_default();
    let leaf = locked.leaf_id("main").ok().flatten();
    (entries, leaf)
}

fn sessions_listing(session_dir: &str, cwd: &str) -> String {
    let mut repo = JsonlRepo::new(std::path::PathBuf::from(session_dir), cwd.to_owned());
    match repo.list() {
        Err(error) => format!("/sessions: {error}"),
        Ok(listed) if listed.is_empty() => "no sessions for this directory".to_owned(),
        Ok(listed) => {
            let now = now_ms();
            listed
                .iter()
                .map(|m| {
                    format!(
                        "{}  {:>8}",
                        m.id,
                        age_label(now.saturating_sub(m.created_at))
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
    }
}

fn undo_text(session: &AgentSession, cwd: &str) -> String {
    if session.status() == yi_runtime::Status::Running {
        return "/undo: the current turn is still running (Esc Esc to stop it)".to_owned();
    }
    let Some(store) = session.store() else {
        return "/undo: this session has no store to read checkpoints from".to_owned();
    };
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    match yi_runtime::undo(&store, std::path::Path::new(cwd), &home) {
        yi_runtime::UndoOutcome::Restored { changes, scoped } => {
            format!("/undo: {}", yi_runtime::describe_undo(&changes, scoped))
        }
        // Scoped to this session on purpose: undoing a turn the reader never saw is not what
        // the word means. An earlier session's turns stay reachable, just not from here.
        yi_runtime::UndoOutcome::NoCheckpoint => {
            "/undo: this session has taken no turn yet — `yi undo` restores an earlier session"
                .to_owned()
        }
        yi_runtime::UndoOutcome::Failed(error) => format!("/undo failed: {error}"),
    }
}

impl SessionPort for Arc<AgentSession> {
    fn history(&mut self) -> Answer {
        Answer::now(Reply::History(branch_of(self)))
    }

    fn entries(&mut self) -> Answer {
        let (entries, leaf) = entries_of(self);
        Answer::now(Reply::Entries { entries, leaf })
    }

    fn rewind(&mut self, entry_id: &str) -> Answer {
        Answer::now(match yi_runtime::rewind_to(self, entry_id) {
            Ok(rewound) => Reply::Rewound {
                entries: branch_of(self),
                unsent: rewound.unsent,
                abandoned: rewound.abandoned,
            },
            Err(error) => Reply::Notice(format!("rewind failed: {error}")),
        })
    }

    fn new_session(&mut self, session_dir: &str, cwd: &str) -> Answer {
        let mut repo = JsonlRepo::new(std::path::PathBuf::from(session_dir), cwd.to_owned());
        let store = match repo.create(CreateOptions::default()) {
            Ok(store) => store,
            Err(error) => return Answer::now(Reply::Notice(format!("/new failed: {error}"))),
        };
        let name = lock_session(&store).metadata().id.clone();
        self.reset();
        Answer::now(match self.attach_store(store) {
            Ok(_) => Reply::Fresh { name },
            Err(error) => Reply::Notice(format!("/new failed: {error}")),
        })
    }

    fn undo(&mut self, cwd: &str) -> Answer {
        Answer::now(Reply::Notice(undo_text(self, cwd)))
    }

    fn slash(&mut self, line: &str, session_dir: &str, cwd: &str) -> Answer {
        let (command, args) = line
            .split_once(char::is_whitespace)
            .map_or((line, ""), |(head, rest)| (head, rest.trim()));
        let text = match command {
            "sessions" if args.is_empty() || args == "list" => sessions_listing(session_dir, cwd),
            "sessions" => "/sessions — listing only; show and rm stay on `yi sessions`".to_owned(),
            other => yi_runtime::slash::run(self, other, args)
                .unwrap_or_else(|| format!("unknown command: /{other}")),
        };
        Answer::now(Reply::Notice(text))
    }

    fn select(&mut self, model: Model, effort: Effort) -> Answer {
        self.set_model(model);
        let effort = self.set_effort(effort);
        Answer::now(Reply::Selected {
            model: self.model(),
            effort,
        })
    }

    fn plan(&mut self) -> Answer {
        let Some(service) = self.plan_service() else {
            return Answer::now(Reply::Notice(
                "/plantree: no plan service is attached to this session".to_owned(),
            ));
        };
        Answer::now(match service.read_plan() {
            Err(error) => Reply::Notice(format!("/plantree: {error}")),
            Ok(plan) => {
                let subplans = yi_runtime::plan::subplans_of(&plan, service.plans_dir());
                Reply::Plan { plan, subplans }
            }
        })
    }

    fn plan_progress(&self) -> Option<crate::hud::PlanProgress> {
        let plan = self.plan_service()?.read_plan().ok()?;
        let progress = yi_types::plan::doc::progress(&plan.todos);
        Some(crate::hud::PlanProgress {
            done: progress.done,
            total: progress.total,
            running: progress.running.map(|label| label.to_string()),
        })
    }

    fn todo_list(&self) -> Option<yi_types::todo::TodoList> {
        AgentSession::todos(self.as_ref()).map(|store| store.list())
    }

    fn memory(&self) -> Option<Arc<yi_runtime::memory::Activity>> {
        AgentSession::memory(self.as_ref())
    }

    fn goal(&self) -> Option<GoalView> {
        let store = self.store()?;
        let goal = lock_session(&store).goal()?;
        Some(GoalView {
            objective: goal.objective,
            status: goal.status.as_str().to_owned(),
            tokens_used: goal.tokens_used,
            token_budget: goal.token_budget,
        })
    }
}

/// Invariant: only what the human typed draws as theirs; the host's words draw as a notice.
pub(crate) fn user_cell(text: String, typed: bool) -> Cell {
    if typed {
        Cell::User { text }
    } else {
        Cell::Notice { text }
    }
}

impl App {
    /// The model's context and the screen must agree about what was said.
    pub fn replay_entries(&mut self, entries: &[Entry]) {
        let _span = yi_types::trace::span("tui.replay_entries").arg("entries", entries.len());
        let cells: Vec<Cell> = entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Message {
                    message, timestamp, ..
                } => match message {
                    AgentMessage::User {
                        content,
                        attribution,
                        ..
                    } => Some(user_cell(
                        user_text(content),
                        attribution.reads_as_typed(*timestamp),
                    )),
                    AgentMessage::Custom {
                        custom_type,
                        content,
                        display: true,
                        details,
                        ..
                    } => Some(Cell::Advisory {
                        source: crate::app::mail_source(custom_type, details.as_ref()),
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
                    AgentMessage::ToolResult {
                        tool_name,
                        content,
                        is_error,
                        details,
                        ..
                    } => Some(Cell::Tool(ToolCell {
                        name: tool_name.clone(),
                        // A replayed entry has no live call to pair with, and the
                        // session never recorded how long the call took.
                        call_id: String::new(),
                        intent: None,
                        status: if *is_error {
                            ToolStatus::Failed
                        } else {
                            ToolStatus::Done
                        },
                        summary: ToolCell::summary_of(tool_name, ""),
                        digest: ToolCell::digest_of(tool_name, &text_of(content), *is_error),
                        preview: Vec::new(),
                        elapsed_ms: 0,
                        calls: 1,
                        details: details.clone().unwrap_or(Value::Null),
                    })),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        for cell in cells {
            self.commit_cell(&cell);
        }
    }

    /// The one writer for whatever a port answers, now or later.
    pub fn apply(&mut self, reply: Reply) {
        match reply {
            Reply::History(entries) => self.replay_entries(&entries),
            Reply::Entries { entries, leaf } => {
                if entries.is_empty() {
                    self.notice("no session store attached — tree unavailable");
                } else {
                    self.tree = Some(TreeView::new(
                        &entries,
                        leaf.as_deref(),
                        TreeFilter::Default,
                    ));
                }
            }
            Reply::Rewound {
                entries,
                unsent,
                abandoned,
            } => {
                self.pending_summary = abandoned;
                self.pending_clear = true;
                self.reset_transcript();
                self.replay_entries(&entries);
                if let Some(text) = unsent
                    && self.composer.is_empty()
                {
                    self.composer.set_text(&text);
                }
            }
            Reply::Fresh { name } => {
                self.options.session_name = name;
                self.cost_total = 0.0;
                self.cost_unknown = false;
                self.pending_clear = true;
                self.reset_transcript();
            }
            Reply::Selected { model, effort } => {
                let label = format!("{}/{}", model.provider, model.id);
                self.selection.model = model;
                self.selection.effort = effort;
                self.notice(if effort == Effort::Off {
                    format!("model {label}")
                } else {
                    format!("model {label} · reasoning {effort}")
                });
            }
            Reply::Plan { plan, subplans } => {
                self.plan_tree = Some(PlanTreeView::new(&plan, &subplans));
            }
            Reply::ChildHistory { child_id, entries } => {
                if self.focused.as_deref() == Some(child_id.as_str()) {
                    self.replay_entries(&entries);
                }
            }
            Reply::Notice(text) => self.notice(text),
        }
        self.scheduler.request();
    }

    pub fn load_history(&mut self, port: &mut dyn SessionPort) {
        if let Answer::Now(reply) = port.history() {
            self.apply(*reply);
        }
    }

    /// Every pending flag a key raised, settled against the port in one place.
    pub fn settle(&mut self, port: &mut dyn SessionPort) {
        let (session_dir, cwd) = (self.options.session_dir.clone(), self.options.cwd.clone());
        let mut answers = Vec::new();
        if std::mem::take(&mut self.pending_open_tree) {
            answers.push(port.entries());
        }
        if std::mem::take(&mut self.pending_open_plan_tree) {
            answers.push(port.plan());
        }
        if let Some(entry_id) = self.pending_rewind.take() {
            answers.push(port.rewind(&entry_id));
        }
        if std::mem::take(&mut self.pending_new) {
            if self.running {
                self.notice("/new: the current turn is still running (Esc Esc to stop it)");
            } else {
                answers.push(port.new_session(&session_dir, &cwd));
            }
        }
        if std::mem::take(&mut self.pending_undo) {
            if self.running {
                self.notice("/undo: the current turn is still running (Esc Esc to stop it)");
            } else {
                answers.push(port.undo(&cwd));
            }
        }
        if let Some((model, effort)) = self.selection.pending.take() {
            answers.push(port.select(model, effort));
        }
        if let Some(line) = self.pending_command.take() {
            answers.push(port.slash(&line, &session_dir, &cwd));
        }
        for answer in answers {
            if let Answer::Now(reply) = answer {
                self.apply(*reply);
            }
        }
    }
}

/// One turn of the loop every host shares: drain what arrived, then settle what a key asked.
pub fn tick(
    app: &mut App,
    port: &mut dyn SessionPort,
    events: &std::sync::mpsc::Receiver<crate::app::UiEvent>,
    cmd_tx: &tokio::sync::mpsc::UnboundedSender<crate::app::Command>,
) {
    app.pacing.held.extend(events.try_iter());
    while let Some(ui_event) = app.pacing.held.pop_front() {
        // The end waits behind the reveal, and everything after it waits too, so a tool row
        // never commits above prose the reader has not seen yet.
        if app.reveal_behind()
            && matches!(
                ui_event,
                crate::app::UiEvent::Agent(yi_types::event::AgentEvent::MessageEnd { .. })
            )
        {
            app.pacing.drain();
            app.pacing.held.push_front(ui_event);
            break;
        }
        match ui_event {
            crate::app::UiEvent::Agent(event) => app.reduce_agent(event),
            crate::app::UiEvent::Child { child_id, event } => app.reduce_child(&child_id, event),
            crate::app::UiEvent::Children(children) => app.sync_children(&children),
            crate::app::UiEvent::ChildUpdates(updates) => {
                for update in &updates {
                    app.adopt(update, None);
                }
            }
            crate::app::UiEvent::Ask(ask) => app.open_approval(ask),
            crate::app::UiEvent::Reply(reply) => app.apply(reply),
        }
    }
    for text in port
        .memory()
        .map(|feed| feed.take_lines())
        .unwrap_or_default()
    {
        app.commit_cell(&Cell::Footer { text });
    }
    app.step_reveal(std::time::Instant::now());
    app.settle(port);
    if let Some(stub) = app.pending_summary.take() {
        let _ = cmd_tx.send(crate::app::Command::SummarizeBranch(stub));
    }
    if let Some(line) = app.pending_slash.take() {
        let _ = cmd_tx.send(crate::app::Command::Slash(line));
    }
    if let Some(child_id) = app.pending_stop.take() {
        let _ = cmd_tx.send(crate::app::Command::StopChild(child_id));
    }
    if let Some(child_id) = app.pending_focus.take() {
        let _ = cmd_tx.send(crate::app::Command::ChildHistory(child_id));
    }
}

impl App {
    pub fn set_model_selector(
        &mut self,
        model: yi_types::model::Model,
        effort: yi_types::model::Effort,
    ) {
        self.selection.model = model;
        self.selection.effort = effort;
        self.scheduler.request();
    }

    pub fn set_draft_if_empty(&mut self, text: &str) {
        if self.composer.is_empty() {
            self.composer.set_text(text);
            self.scheduler.request();
        }
    }

    pub fn set_status_name_shown(&mut self, shown: bool) {
        self.status_name_hidden = !shown;
    }

    pub fn set_pane(&mut self) {
        self.pane = true;
    }

    pub fn take_pending_editor(&mut self) -> bool {
        std::mem::take(&mut self.pending_editor)
    }

    pub fn orb_animating(&self) -> bool {
        self.kitty && self.orb_placement.is_some() && !self.orb.at_rest(self.orb_state())
    }
}

impl crate::app::App {
    pub fn set_workdir(&mut self, cwd: String, lane: Option<String>) {
        if self.options.cwd == cwd && self.options.lane == lane {
            return;
        }
        self.branch = git_branch(&cwd);
        self.options.cwd = cwd;
        self.options.lane = lane;
        self.scheduler.request();
    }
}
