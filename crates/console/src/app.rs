//! The protocol state machine and reducer: synchronous and deterministic,
//! so the drive harness runs this exact code over a fixture socket.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{Event as CtEvent, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Direction;
use serde_json::{Value, json};
use yi_tui::composer::Composer;
use yi_tui::popup::{ListPopup, walk_files};
use yi_types::acp::{AcpPermissionParams, AcpSessionUpdate, AcpUpdateParams};

use crate::client::{ClientEvent, Outbound};
use crate::keys::{self, Action};
use crate::layout::{NavDirection, PaneId};
use crate::model::{
    ActiveAsk, Bottom, ConsoleState, Link, Mode, PaneContent, RequestId, SessionId, SessionRow,
    SessionStatus, Zone, now_ms,
};
use crate::notify::{NoteQueue, OscFlavor, escape};

const REQUEST_DEADLINE: Duration = Duration::from_secs(10);
const LIST_POLL: Duration = Duration::from_secs(5);
const SPLIT_ANIM: Duration = Duration::from_millis(140);
const QUIT_WINDOW: Duration = Duration::from_secs(1);

/// One split-open animation: the fresh split's ratio eases 0.12 -> 0.5.
struct Anim {
    tab: usize,
    path: Vec<bool>,
    start: Instant,
}

fn ease_out_cubic(t: f32) -> f32 {
    let inverse = 1.0 - t.clamp(0.0, 1.0);
    1.0 - inverse * inverse * inverse
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestKind {
    Initialize,
    ListWorker,
    ListDaemon,
    NewSession(PaneId),
    Resume(SessionId),
    Prompt,
    Cancel,
    Seen,
    Tracked(SessionId, Vec<String>),
    KernelExecute,
    KernelCancel,
    Slash(SessionId),
}

struct Pending {
    kind: RequestKind,
    deadline: Instant,
}

pub struct App {
    pub state: ConsoleState,
    pub composer: Composer,
    pub bottom: Option<Bottom>,
    pub spinner: usize,
    pending: HashMap<RequestId, Pending>,
    next_request: u64,
    next_list_poll: Instant,
    pub dirty: bool,
    /// Split-open animations; empty under the headless harness.
    animations: Vec<Anim>,
    pub animate: bool,
    pub cmd_hints: bool,
    pub autostart: bool,
    next_disk_check: Instant,
    editor_drag: bool,
    notes: NoteQueue,
    pub osc_flavor: OscFlavor,
    /// Escapes for the real loop to write to the terminal; drained per draw.
    pub osc_out: Vec<String>,
    /// Hit-test table from the last draw; the mouse path reads it.
    pub hits: Option<crate::render::Hits>,
    /// Split path under an active border drag, pinned to its tab so a
    /// mid-drag tab switch can never resize a colliding path elsewhere.
    drag: Option<(usize, Vec<bool>)>,
    /// Invariant: a `replayedTo` offset is valid only while nothing later streamed for that
    /// session; any update clears it, keeping a skip-ahead resume equal to a full replay.
    resume_offsets: HashMap<SessionId, u64>,
    interrupt_at: Option<Instant>,
}

fn frame(id: Option<u64>, method: &str, params: Value) -> Value {
    match id {
        Some(id) => json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
        None => json!({"jsonrpc": "2.0", "method": method, "params": params}),
    }
}

impl App {
    pub fn new(root: String) -> Self {
        Self {
            state: ConsoleState::new(root),
            composer: Composer::default(),
            bottom: None,
            spinner: 0,
            pending: HashMap::new(),
            next_request: 0,
            next_list_poll: Instant::now() + LIST_POLL,
            dirty: true,
            animations: Vec::new(),
            cmd_hints: false,
            autostart: false,
            next_disk_check: Instant::now(),
            editor_drag: false,
            animate: false,
            notes: NoteQueue::default(),
            osc_flavor: OscFlavor::None,
            osc_out: Vec::new(),
            hits: None,
            drag: None,
            resume_offsets: HashMap::new(),
            interrupt_at: None,
        }
    }

    fn send_request(
        &mut self,
        outbound: &Outbound,
        kind: RequestKind,
        method: &str,
        params: Value,
    ) {
        self.next_request = self.next_request.wrapping_add(1);
        let id = self.next_request;
        if outbound.send(&frame(Some(id), method, params)) {
            self.pending.insert(
                RequestId(id),
                Pending {
                    kind,
                    deadline: Instant::now() + REQUEST_DEADLINE,
                },
            );
        } else {
            self.note("send failed: daemon link is down or wedged");
        }
        self.dirty = true;
    }

    fn note(&mut self, text: &str) {
        self.state.status_note = Some(text.to_owned());
        self.dirty = true;
    }

    fn connected(&self) -> bool {
        self.state.link == Link::Connected
    }

    pub fn tick(&mut self, outbound: &Outbound) {
        let now = Instant::now();
        let expired: Vec<RequestId> = self
            .pending
            .iter()
            .filter(|(_, pending)| pending.deadline <= now)
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            if let Some(pending) = self.pending.remove(&id) {
                self.note(&format!("request timed out: {:?}", pending.kind));
            }
        }
        if self.connected() && now >= self.next_list_poll {
            self.next_list_poll = now + LIST_POLL;
            self.send_request(outbound, RequestKind::ListDaemon, "session/list", json!({}));
        }
        self.tick_animations(now);
        self.tick_notes(now);
        self.check_editors(now);
        if self.focused_status() == Some(SessionStatus::Working) {
            self.spinner = self.spinner.wrapping_add(1);
            self.dirty = true;
        }
    }

    fn focused_status(&self) -> Option<SessionStatus> {
        let id = self.state.focused_session()?;
        self.state.sessions.get(&id).map(|row| row.status)
    }

    fn tick_animations(&mut self, now: Instant) {
        let mut index = 0;
        while index < self.animations.len() {
            let Some(anim) = self.animations.get(index) else {
                break;
            };
            let elapsed = now.saturating_duration_since(anim.start);
            let t = elapsed.as_secs_f32() / SPLIT_ANIM.as_secs_f32();
            let ratio = 0.12 + (0.5 - 0.12) * ease_out_cubic(t);
            let tab = anim.tab;
            let path = anim.path.clone();
            if let Some(tab) = self.state.tabs.get_mut(tab) {
                tab.layout.set_ratio_at(&path, ratio);
            }
            self.dirty = true;
            if t >= 1.0 {
                self.animations.remove(index);
            } else {
                index = index.saturating_add(1);
            }
        }
    }

    /// Delay then re-validate: a note fires only when the session still
    /// holds the armed status and is not the pane the user is looking at.
    fn tick_notes(&mut self, now: Instant) {
        if self.notes.is_empty() {
            return;
        }
        let focused = self.state.focused_session();
        for note in self.notes.due(now) {
            if focused.as_ref() == Some(&note.session) {
                continue;
            }
            let still = self
                .state
                .sessions
                .get(&note.session)
                .is_some_and(|row| row.status == note.status);
            if !still {
                continue;
            }
            let short: String = note.session.0.chars().take(12).collect();
            let (label, body) = match note.status {
                SessionStatus::Blocked => ("blocked", "needs your approval"),
                _ => ("done", "finished while you were away"),
            };
            self.note(&format!("{short} {label} — {body}"));
            if let Some(seq) = escape(self.osc_flavor, &format!("yi: {short}"), body) {
                self.osc_out.push(seq);
            }
        }
    }

    pub fn reduce_client(&mut self, outbound: &Outbound, event: ClientEvent) {
        match event {
            ClientEvent::Connected => {
                self.state.link = Link::Connected;
                self.pending.clear();
                self.send_request(
                    outbound,
                    RequestKind::Initialize,
                    "initialize",
                    json!({"protocolVersion": 2, "clientInfo": {"name": "yi-console"}}),
                );
            }
            ClientEvent::Disconnected { reason } => {
                self.state.link = Link::Disconnected { reason };
                // Cancel correctness: nothing in flight survives the socket.
                self.pending.clear();
                self.state.ask = None;
                self.dirty = true;
            }
            ClientEvent::BadFrame => {
                self.state.dropped_frames = self.state.dropped_frames.saturating_add(1);
                self.dirty = true;
            }
            ClientEvent::Frame(value) => self.reduce_frame(outbound, value),
        }
    }

    fn reduce_frame(&mut self, outbound: &Outbound, value: Value) {
        let method = value.get("method").and_then(Value::as_str);
        match method {
            None => self.reduce_response(outbound, &value),
            Some("session/update") => {
                if let Some(params) = value.get("params").cloned()
                    && let Ok(update) = serde_json::from_value::<AcpUpdateParams>(params)
                {
                    self.reduce_update(outbound, update);
                } else {
                    self.state.dropped_frames = self.state.dropped_frames.saturating_add(1);
                }
                self.dirty = true;
            }
            Some("session/request_permission") => {
                let id = value.get("id").and_then(Value::as_str).map(str::to_owned);
                let params = value
                    .get("params")
                    .cloned()
                    .and_then(|params| serde_json::from_value::<AcpPermissionParams>(params).ok());
                match (id, params) {
                    (Some(request_id), Some(params)) => {
                        let session = SessionId(params.session_id.clone());
                        self.state
                            .set_session_status(&session, SessionStatus::Blocked);
                        self.notes
                            .arm(&session, SessionStatus::Blocked, Instant::now());
                        self.state.ask = Some(ActiveAsk { request_id, params });
                    }
                    _ => {
                        self.state.dropped_frames = self.state.dropped_frames.saturating_add(1);
                    }
                }
                self.dirty = true;
            }
            Some(_) => {
                self.state.dropped_frames = self.state.dropped_frames.saturating_add(1);
            }
        }
    }

    fn reduce_response(&mut self, outbound: &Outbound, value: &Value) {
        let Some(id) = value.get("id").and_then(Value::as_u64) else {
            return;
        };
        let Some(pending) = self.pending.remove(&RequestId(id)) else {
            return;
        };
        if let Some(error) = value.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            self.note(&format!("{:?}: {message}", pending.kind));
            return;
        }
        let result = value.get("result").cloned().unwrap_or(Value::Null);
        match pending.kind {
            RequestKind::Initialize => {
                let root = self.state.root.clone();
                self.send_request(
                    outbound,
                    RequestKind::ListWorker,
                    "session/list",
                    json!({"cwd": root}),
                );
                self.send_request(outbound, RequestKind::ListDaemon, "session/list", json!({}));
                // Invariant: one resume per session, not per pane. A replay update fans out
                // to every bound pane, so a second resume renders the transcript twice.
                let mut seen: Vec<SessionId> = Vec::new();
                let visible: Vec<(PaneId, SessionId)> = self
                    .state
                    .panes
                    .iter()
                    .filter_map(|(pane_id, pane)| {
                        pane.session().cloned().map(|session| (*pane_id, session))
                    })
                    .filter(|(_, session)| {
                        !seen.contains(session) && {
                            seen.push(session.clone());
                            true
                        }
                    })
                    .collect();
                for (pane_id, session) in visible {
                    self.resume_into(outbound, pane_id, &session);
                }
            }
            RequestKind::ListWorker => {
                self.merge_worker_list(&result);
                if std::mem::take(&mut self.autostart) {
                    self.autostart_session(outbound);
                } else if self.state.order.is_empty() {
                    self.note("no sessions for this root — alt+n starts one");
                }
            }
            RequestKind::ListDaemon => self.merge_daemon_list(&result),
            RequestKind::Tracked(session, paths) => self.absorb_tracked(&session, &paths, &result),
            RequestKind::KernelExecute | RequestKind::KernelCancel => {}
            RequestKind::Slash(session) => {
                if let Some(text) = result.get("text").and_then(Value::as_str) {
                    self.note_transcript(&session, text);
                }
            }
            RequestKind::NewSession(pane_id) => {
                if let Some(session_id) = result.get("sessionId").and_then(Value::as_str) {
                    let session = SessionId(session_id.to_owned());
                    let root = self.state.root.clone();
                    self.state.upsert_row(SessionRow {
                        id: session.clone(),
                        root,
                        status: SessionStatus::Idle,
                        attached: true,
                        name: None,
                        created_ms: now_ms(),
                        last_ms: 0,
                    });
                    self.bind_pane(pane_id, &session);
                    self.mark_seen(outbound, &session);
                }
            }
            RequestKind::Resume(session) => {
                if let Some(row) = self.state.sessions.get_mut(&session) {
                    row.attached = true;
                }
                if let Some(replayed_to) = result.get("replayedTo").and_then(Value::as_u64) {
                    self.resume_offsets.insert(session.clone(), replayed_to);
                }
                self.mark_seen(outbound, &session);
            }
            RequestKind::Prompt => {
                self.send_request(outbound, RequestKind::ListDaemon, "session/list", json!({}));
            }
            RequestKind::Cancel | RequestKind::Seen => {}
        }
        self.dirty = true;
    }

    fn bind_pane(&mut self, pane_id: PaneId, session: &SessionId) {
        if let Some(pane) = self.state.panes.get_mut(&pane_id) {
            match &mut pane.content {
                PaneContent::Session {
                    session: slot,
                    transcript,
                } => {
                    transcript.clear();
                    *slot = Some(session.clone());
                }
                PaneContent::Notebook { session: slot, .. } => *slot = Some(session.clone()),
                PaneContent::Markdown { .. }
                | PaneContent::Diff { .. }
                | PaneContent::SessionDiff { .. }
                | PaneContent::Editor(_) => {
                    pane.content = PaneContent::Session {
                        session: Some(session.clone()),
                        transcript: crate::transcript::Transcript::new(),
                    };
                }
            }
            pane.scroll_from_bottom = 0;
        }
    }

    fn merge_worker_list(&mut self, result: &Value) {
        let Some(sessions) = result.get("sessions").and_then(Value::as_array) else {
            return;
        };
        let root = self.state.root.clone();
        for entry in sessions {
            let Some(id) = entry.get("sessionId").and_then(Value::as_str) else {
                continue;
            };
            let id = SessionId(id.to_owned());
            let attached = entry
                .get("attached")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let status = self
                .state
                .sessions
                .get(&id)
                .map_or(SessionStatus::Unknown, |row| row.status);
            self.state.upsert_row(SessionRow {
                id,
                root: root.clone(),
                status,
                attached,
                name: entry.get("name").and_then(Value::as_str).map(str::to_owned),
                created_ms: entry.get("createdAt").and_then(Value::as_u64).unwrap_or(0),
                last_ms: 0,
            });
        }
    }

    fn merge_daemon_list(&mut self, result: &Value) {
        let Some(sessions) = result.get("sessions").and_then(Value::as_array) else {
            return;
        };
        for entry in sessions {
            let Some(id) = entry.get("sessionId").and_then(Value::as_str) else {
                continue;
            };
            let id = SessionId(id.to_owned());
            let name = entry.get("name").and_then(Value::as_str).map(str::to_owned);
            let last_ms = entry
                .get("lastEventMs")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            // A session on screen streams its own state; the ledger covers
            // the ones nobody is attached to, and names whichever it can.
            if self.state.session_visible(&id) {
                if let Some(row) = self.state.sessions.get_mut(&id) {
                    row.name = name.or(row.name.take());
                    row.last_ms = row.last_ms.max(last_ms);
                }
                continue;
            }
            let unseen = entry.get("unseen").and_then(Value::as_u64).unwrap_or(0);
            let status = entry
                .get("lastState")
                .and_then(Value::as_str)
                .map_or(SessionStatus::Unknown, |state| {
                    SessionStatus::from_ledger(state, unseen)
                });
            let attached = entry
                .get("attached")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let root = entry
                .get("cwd")
                .and_then(Value::as_str)
                .unwrap_or(&self.state.root)
                .to_owned();
            self.state.upsert_row(SessionRow {
                id,
                root,
                status,
                attached,
                name,
                created_ms: 0,
                last_ms,
            });
        }
    }

    fn reduce_update(&mut self, outbound: &Outbound, update: AcpUpdateParams) {
        let id = SessionId(update.session_id.clone());
        self.resume_offsets.remove(&id);
        if let AcpSessionUpdate::Extension(extension) = &update.update
            && extension.session_update == "_yi/subagent_update"
        {
            let fields: serde_json::Map<String, Value> = extension
                .fields
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            if let Ok(child) =
                serde_json::from_value::<yi_types::subagent::ChildUpdate>(Value::Object(fields))
            {
                let rows = self.state.children.entry(id.clone()).or_default();
                match rows.iter().position(|row| row.id == child.id) {
                    Some(at) => {
                        if let Some(slot) = rows.get_mut(at) {
                            *slot = child;
                        }
                    }
                    None if rows.len() < 32 => rows.push(child),
                    None => {}
                }
                self.dirty = true;
            }
        }
        if let AcpSessionUpdate::StateUpdate(state) = &update.update {
            let focused = self.state.focused_session().as_ref() == Some(&id);
            let mut status = SessionStatus::from_state(state, false);
            let was_working = self
                .state
                .sessions
                .get(&id)
                .is_some_and(|row| row.status == SessionStatus::Working);
            if status == SessionStatus::Working && !was_working {
                self.state.children.remove(&id);
            }
            // A pane the user is looking at needs no unseen shout, and a
            // resolved permission clears the ask bar.
            if focused && status == SessionStatus::DoneUnseen {
                status = SessionStatus::Idle;
            }
            if status != SessionStatus::Blocked
                && self
                    .state
                    .ask
                    .as_ref()
                    .is_some_and(|ask| ask.params.session_id == id.0)
            {
                self.state.ask = None;
            }
            self.state.set_session_status(&id, status);
            match status {
                SessionStatus::Blocked | SessionStatus::DoneUnseen => {
                    self.notes.arm(&id, status, Instant::now());
                }
                _ => self.notes.disarm(&id),
            }
        }
        if let AcpSessionUpdate::UsageUpdate { used, size } = &update.update {
            let info = self.state.status.entry(id.clone()).or_default();
            info.context_used = *used;
            info.context_window = *size;
        }
        if let AcpSessionUpdate::Extension(extension) = &update.update
            && extension.session_update == "_yi/status"
        {
            self.state
                .status
                .entry(id.clone())
                .or_default()
                .absorb(&extension.fields);
        }
        self.absorb_edit(outbound, &id, &update.update);
        self.absorb_kernel(&id, &update.update);
        for pane in self.state.panes.values_mut() {
            if pane.session() != Some(&id) {
                continue;
            }
            match &mut pane.content {
                PaneContent::Session { transcript, .. } => {
                    if transcript.apply(&update.update) {
                        pane.scroll_from_bottom = 0;
                    }
                }
                PaneContent::Notebook { cells, .. } => {
                    if apply_notebook(cells, &update.update) {
                        pane.scroll_from_bottom = 0;
                    }
                }
                PaneContent::Markdown { .. }
                | PaneContent::Diff { .. }
                | PaneContent::SessionDiff { .. }
                | PaneContent::Editor(_) => {}
            }
        }
    }

    fn resume_into(&mut self, outbound: &Outbound, pane_id: PaneId, session: &SessionId) {
        // Invariant: replay streams ahead of the resume response, so wipe and bind happen at
        // send time; a stored offset skips the wipe only if this pane shows the session.
        let continuous = self
            .state
            .panes
            .get(&pane_id)
            .is_some_and(|pane| pane.session() == Some(session));
        let offset = if continuous {
            self.resume_offsets.get(session).copied()
        } else {
            None
        };
        if offset.is_none() {
            let bound: Vec<PaneId> = self
                .state
                .panes
                .iter()
                .filter(|(id, pane)| **id == pane_id || pane.session() == Some(session))
                .map(|(id, _)| *id)
                .collect();
            for id in bound {
                self.bind_pane(id, session);
            }
        }
        let root = self
            .state
            .sessions
            .get(session)
            .map_or_else(|| self.state.root.clone(), |row| row.root.clone());
        self.send_request(
            outbound,
            RequestKind::Resume(session.clone()),
            "session/resume",
            json!({
                "sessionId": session.0,
                "cwd": root,
                "replayFrom": offset.unwrap_or(0),
            }),
        );
    }

    fn mark_seen(&mut self, outbound: &Outbound, session: &SessionId) {
        self.notes.disarm(session);
        if let Some(row) = self.state.sessions.get_mut(session)
            && row.status == SessionStatus::DoneUnseen
        {
            row.status = SessionStatus::Idle;
        }
        self.send_request(
            outbound,
            RequestKind::Seen,
            "_yi/seen",
            json!({"sessionId": session.0}),
        );
    }

    fn open_navigator(&mut self, query: &str) {
        self.state.mode = Mode::Navigator {
            query: query.to_owned(),
            selected: 0,
        };
        self.dirty = true;
    }

    fn on_notebook(&self) -> bool {
        self.state
            .focused_pane_id()
            .and_then(|id| self.state.panes.get(&id))
            .is_some_and(|pane| matches!(pane.content, PaneContent::Notebook { .. }))
    }

    fn run_notebook_cell(&mut self, outbound: &Outbound) {
        if !self.connected() {
            return self.note("not connected — cell not sent");
        }
        let Some(PaneContent::Notebook {
            session: Some(session),
            input,
            ..
        }) = self.state.focused_pane_mut().map(|pane| &mut pane.content)
        else {
            return self.note("no session behind this notebook");
        };
        let code = input.lines().join("\n");
        if code.trim().is_empty() {
            return;
        }
        let session = session.clone();
        *input = crate::model::notebook_input();
        self.send_request(
            outbound,
            RequestKind::KernelExecute,
            "_yi/kernel_execute",
            json!({"sessionId": session.0, "code": code}),
        );
    }

    fn cancel_notebook_cell(&mut self, outbound: &Outbound) {
        let Some(PaneContent::Notebook {
            session: Some(session),
            cells,
            ..
        }) = self
            .state
            .focused_pane_id()
            .and_then(|id| self.state.panes.get(&id))
            .map(|pane| &pane.content)
        else {
            return;
        };
        let running = cells
            .iter()
            .rev()
            .find(|cell| cell.running && cell.call_id.starts_with("user-"))
            .map(|cell| cell.call_id.clone());
        let Some(call_id) = running else {
            return self.note("no user cell is running");
        };
        let session = session.clone();
        self.send_request(
            outbound,
            RequestKind::KernelCancel,
            "_yi/kernel_cancel",
            json!({"sessionId": session.0, "callId": call_id}),
        );
    }

    fn answer_ask(&mut self, outbound: &Outbound, option_id: &str) {
        let Some(ask) = self.state.ask.take() else {
            return;
        };
        let response = json!({
            "jsonrpc": "2.0",
            "id": ask.request_id,
            "result": {"outcome": {"outcome": "selected", "optionId": option_id}},
        });
        if !outbound.send(&response) {
            self.note("send failed: permission answer not delivered");
        }
        self.dirty = true;
    }

    fn open_selected(&mut self, outbound: &Outbound) {
        if !self.connected() {
            self.note("not connected");
            return;
        }
        let Some(session) = self.state.selected_id().cloned() else {
            return;
        };
        let Some(pane_id) = self.state.focused_pane_id() else {
            return;
        };
        self.resume_into(outbound, pane_id, &session);
        self.state.zone = Zone::Panes;
    }

    fn autostart_session(&mut self, outbound: &Outbound) {
        let bound = self
            .state
            .panes
            .values()
            .any(|pane| pane.session().is_some());
        if bound {
            return;
        }
        self.state.zone = Zone::Panes;
        let newest = self
            .state
            .visible_rows()
            .first()
            .and_then(|index| self.state.order.get(*index))
            .cloned();
        match (newest, self.state.focused_pane_id()) {
            (Some(session), Some(pane_id)) => self.resume_into(outbound, pane_id, &session),
            _ => self.new_session(outbound),
        }
        let hint = if self.cmd_hints {
            "workspace · ⌘P palette · ⌘\\ split · ⌘J notebook · ⌘B sidebar"
        } else {
            "workspace · ⌥/ palette · ⌥v split · ⌥⇧J notebook · ⌥b sidebar"
        };
        self.note(hint);
    }

    fn new_session(&mut self, outbound: &Outbound) {
        if !self.connected() {
            self.note("not connected");
            return;
        }
        let Some(pane_id) = self.state.focused_pane_id() else {
            return;
        };
        self.state.zone = Zone::Panes;
        let root = self.state.root.clone();
        self.send_request(
            outbound,
            RequestKind::NewSession(pane_id),
            "session/new",
            json!({"cwd": root}),
        );
    }

    fn scroll_focused(&mut self, delta: isize) {
        if let Some(pane) = self.state.focused_pane_mut() {
            pane.scroll_from_bottom = if delta >= 0 {
                pane.scroll_from_bottom.saturating_add(delta.unsigned_abs())
            } else {
                pane.scroll_from_bottom.saturating_sub(delta.unsigned_abs())
            };
            self.dirty = true;
        }
    }

    fn apply_action(&mut self, outbound: &Outbound, action: Action) {
        match action {
            Action::SplitRight => self.split_with_anim(Direction::Horizontal),
            Action::SplitDown => self.split_with_anim(Direction::Vertical),
            Action::ClosePane => {
                if self.state.close_focused_pane().is_none() {
                    self.note("last pane — alt+q quits");
                }
            }
            Action::Zoom => {
                if let Some(tab) = self.state.tab_mut() {
                    tab.zoomed = !tab.zoomed;
                }
            }
            Action::FocusLeft => self.focus_direction(NavDirection::Left),
            Action::FocusRight => self.focus_direction(NavDirection::Right),
            Action::FocusUp => self.focus_direction(NavDirection::Up),
            Action::FocusDown => self.focus_direction(NavDirection::Down),
            Action::NewTab => self.state.new_tab(),
            Action::NextTab => {
                let next = self
                    .state
                    .active_tab
                    .saturating_add(1)
                    .checked_rem(self.state.tabs.len())
                    .unwrap_or(0);
                self.state.select_tab(next);
            }
            Action::PrevTab => {
                let len = self.state.tabs.len().max(1);
                let prev = self
                    .state
                    .active_tab
                    .checked_add(len.saturating_sub(1))
                    .and_then(|sum| sum.checked_rem(len))
                    .unwrap_or(0);
                self.state.select_tab(prev);
            }
            Action::SelectTab(n) => {
                self.state.select_tab(usize::from(n).saturating_sub(1));
            }
            Action::NewSession => self.new_session(outbound),
            Action::Navigator => {
                self.state.mode = Mode::Navigator {
                    query: String::new(),
                    selected: 0,
                };
            }
            Action::ToggleZone => {
                self.state.zone = match self.state.zone {
                    Zone::Sidebar => Zone::Panes,
                    Zone::Panes => Zone::Sidebar,
                };
            }
            Action::ScrollUp => self.scroll_focused(3),
            Action::ScrollDown => self.scroll_focused(-3),
            Action::PageUp => self.scroll_focused(20),
            Action::PageDown => self.scroll_focused(-20),
            Action::CancelTurn => self.cancel_turn(outbound),
            Action::Interrupt => self.interrupt(outbound),
            Action::Quit => self.state.quit = true,
            Action::ToggleSidebar => self.state.sidebar = self.state.sidebar.next(),
            Action::ToggleNotebook => self.toggle_side(outbound, diffs::SideKind::Notebook),
            Action::ToggleDiff => self.toggle_side(outbound, diffs::SideKind::Diff),
            Action::OpenEditor => self.open_navigator("e "),
            Action::Find => self.open_navigator("/"),
            Action::Save | Action::Undo | Action::Redo => {
                if !self.editor_action(action) {
                    self.note("no editor pane is focused");
                }
            }
        }
        // Whatever pane the action landed on, its session counts as looked-at.
        if let Some(session) = self.state.focused_session()
            && self
                .state
                .sessions
                .get(&session)
                .is_some_and(|row| row.status == SessionStatus::DoneUnseen)
        {
            self.mark_seen(outbound, &session);
        }
        self.dirty = true;
    }

    fn split_with_anim(&mut self, direction: Direction) {
        let Some(new_id) = self.state.split_focused(direction) else {
            return;
        };
        if !self.animate {
            return;
        }
        let tab = self.state.active_tab;
        if let Some(path) = self
            .state
            .tab()
            .and_then(|tab| tab.layout.split_path_of_second(new_id))
        {
            if let Some(tab_state) = self.state.tabs.get_mut(tab) {
                tab_state.layout.set_ratio_at(&path, 0.12);
            }
            self.animations.push(Anim {
                tab,
                path,
                start: Instant::now(),
            });
        }
    }

    fn focus_direction(&mut self, direction: NavDirection) {
        // A fixed virtual area: navigation needs only relative geometry.
        let area = ratatui::layout::Rect::new(0, 0, 200, 100);
        if let Some(tab) = self.state.tab_mut() {
            tab.layout.focus_direction(direction, area);
        }
    }

    pub fn handle_event(&mut self, outbound: &Outbound, event: CtEvent) {
        match event {
            CtEvent::Key(key) => self.handle_key(outbound, key),
            CtEvent::Resize(_, _) => {
                self.dirty = true;
            }
            CtEvent::Mouse(mouse) => {
                use ratatui::crossterm::event::MouseEventKind;
                let kind = match mouse.kind {
                    MouseEventKind::Down(_) => Some(MouseKind::Down),
                    MouseEventKind::Up(_) => Some(MouseKind::Up),
                    MouseEventKind::Drag(_) => Some(MouseKind::Drag),
                    MouseEventKind::ScrollUp => Some(MouseKind::ScrollUp),
                    MouseEventKind::ScrollDown => Some(MouseKind::ScrollDown),
                    _ => None,
                };
                if let Some(kind) = kind {
                    self.handle_mouse(outbound, kind, mouse.column, mouse.row);
                }
            }
            CtEvent::Paste(text) => {
                if self.state.zone == Zone::Panes && matches!(self.state.mode, Mode::Normal) {
                    self.composer.handle_paste(&text);
                    self.dirty = true;
                }
            }
            _ => {}
        }
    }

    fn handle_key(&mut self, outbound: &Outbound, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::SUPER) {
            self.cmd_hints = true;
        }
        if matches!(self.state.mode, Mode::Navigator { .. }) {
            self.handle_navigator_key(outbound, key);
            return;
        }
        if self.state.zone == Zone::Panes
            && !self.on_editor()
            && !self.on_notebook()
            && let Some(bottom) = self.bottom.take()
        {
            return self.handle_bottom_key(outbound, bottom, key);
        }
        let composer_empty = self.composer.is_empty();
        // Invariant: bare approval keys fire only over an empty composer, so
        // mid-prompt typing can never answer a permission by accident.
        if self.state.ask.is_some() && (composer_empty || self.state.zone == Zone::Sidebar) {
            match key.code {
                KeyCode::Char('a') if key.modifiers.is_empty() => {
                    return self.answer_ask(outbound, "allow_once");
                }
                KeyCode::Char('A') => return self.answer_ask(outbound, "allow_always"),
                KeyCode::Char('r') if key.modifiers.is_empty() => {
                    return self.answer_ask(outbound, "reject_once");
                }
                _ => {}
            }
        }
        if matches!(self.state.mode, Mode::Prefix) {
            self.state.mode = Mode::Normal;
            if let Some(action) = keys::prefixed(&key) {
                self.apply_action(outbound, action);
            }
            self.dirty = true;
            return;
        }
        if keys::is_prefix(&key) {
            self.state.mode = Mode::Prefix;
            self.dirty = true;
            return;
        }
        if let Some(action) = keys::direct(&key) {
            // Esc cancels the turn only from the panes zone; sidebar Esc is
            // inert rather than surprising, and on a notebook it cancels the cell.
            if action == Action::CancelTurn && self.state.zone == Zone::Sidebar {
                return;
            }
            if action == Action::CancelTurn && self.on_notebook() {
                return self.cancel_notebook_cell(outbound);
            }
            if self.on_editor()
                && matches!(
                    action,
                    Action::CancelTurn | Action::PageUp | Action::PageDown
                )
            {
                return self.editor_key(key);
            }
            self.apply_action(outbound, action);
            return;
        }
        match self.state.zone {
            Zone::Sidebar => match key.code {
                KeyCode::Up | KeyCode::Down => {
                    let rows = self.state.visible_rows();
                    let at = rows
                        .iter()
                        .position(|i| *i == self.state.selected)
                        .unwrap_or(0);
                    let next = if key.code == KeyCode::Up {
                        at.saturating_sub(1)
                    } else {
                        at.saturating_add(1).min(rows.len().saturating_sub(1))
                    };
                    if let Some(index) = rows.get(next) {
                        self.state.selected = *index;
                    }
                    self.state.cursor_moved = true;
                    self.dirty = true;
                }
                KeyCode::Left | KeyCode::Right => {
                    self.state.cycle_root_filter(key.code == KeyCode::Right);
                    self.dirty = true;
                }
                KeyCode::Enter => self.open_selected(outbound),
                _ => {}
            },
            Zone::Panes if self.on_editor() => self.editor_key(key),
            Zone::Panes if self.on_notebook() => match key.code {
                KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    self.run_notebook_cell(outbound);
                }
                _ => {
                    if let Some(PaneContent::Notebook { input, .. }) =
                        self.state.focused_pane_mut().map(|pane| &mut pane.content)
                    {
                        let _ = input.input(CtEvent::Key(key));
                    }
                    self.dirty = true;
                }
            },
            Zone::Panes => {
                let plain = !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
                match key.code {
                    KeyCode::Enter if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                        self.submit_prompt(outbound);
                    }
                    KeyCode::Enter => self.composer.insert_newline(),
                    KeyCode::Char('/') if plain && self.composer.is_empty() => {
                        let verbs = chat::CONSOLE_VERBS
                            .iter()
                            .map(|verb| (*verb).to_owned())
                            .collect();
                        self.bottom = Some(Bottom::Command(ListPopup::new('/', verbs)));
                    }
                    KeyCode::Char('@') if plain => {
                        let files = walk_files(std::path::Path::new(&self.state.root), 100);
                        self.bottom = Some(Bottom::File(ListPopup::new('@', files)));
                    }
                    KeyCode::Backspace if plain => self.composer.backspace(),
                    _ => self.composer.input(key),
                }
                self.dirty = true;
            }
        }
    }
}

mod chat;
mod diffs;
mod editor;
mod mouse;
mod navigator;
mod notebook;

pub use mouse::MouseKind;
use notebook::apply_notebook;
