//! The protocol state machine and reducer: synchronous and deterministic,
//! so the drive harness runs this exact code over a fixture socket.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{Event as CtEvent, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Direction;
use serde_json::{Value, json};
use yi_tui::colors::Theme;
use yi_tui::input::handle_terminal_event;
use yi_tui::{Reply, UiEvent};
use yi_types::acp::{AcpPermissionParams, AcpSessionUpdate, AcpUpdateParams};

use crate::client::{ClientEvent, Outbound};
use crate::keys::{self, Action};
use crate::layout::{NavDirection, PaneId};
use crate::model::{
    ConsoleState, Link, Mode, PaneContent, RequestId, SessionId, SessionRow, SessionStatus, Zone,
    now_ms,
};
use crate::notify::{NoteQueue, OscFlavor, escape};
use port::EventSeq;

const REQUEST_DEADLINE: Duration = Duration::from_secs(10);
const LIST_POLL: Duration = Duration::from_secs(5);
const FLASH: Duration = Duration::from_secs(2);
const SPLIT_ANIM: Duration = Duration::from_millis(140);
const QUIT_WINDOW: Duration = Duration::from_secs(1);

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
    BranchDiff(SessionId),
    Tape(SessionId),
    Why(SessionId),
    KernelExecute,
    KernelCancel,
    Slash(SessionId),
    Rewind(SessionId),
    Plan(SessionId),
    SetConfig(SessionId),
    Steer,
    Shutdown,
}

struct Pending {
    kind: RequestKind,
    deadline: Instant,
    sent_us: u64,
}

pub struct App {
    pub state: ConsoleState,
    pub theme: Theme,
    pub kitty: bool,
    pending: HashMap<RequestId, Pending>,
    next_request: u64,
    next_list_poll: Instant,
    pub dirty: bool,
    /// Incident: a W1→W2→W1 flip inside one frame left ratatui's size unchanged, so it drew
    /// no clear over a screen the terminal had reflowed; every resize event now asks for one.
    pub pending_clear: bool,
    /// (payload length, rect) of the placed notebook image; unchanged frames skip the
    /// retransmit, and a drawn clear empties it, since the clear took the image with it.
    pub notebook_image: Option<(usize, ratatui::layout::Rect)>,
    animations: Vec<Anim>,
    pub animate: bool,
    pub cmd_hints: bool,
    pub autostart: bool,
    next_disk_check: Instant,
    editor_drag: bool,
    notes: NoteQueue,
    pub osc_flavor: OscFlavor,
    pub osc_out: Vec<String>,
    pub hits: Option<crate::render::Hits>,
    /// The live drag over the frame, and the text the last draw read under it.
    pub selection: Option<crate::select::Drag>,
    pub selected: String,
    pub flash: Option<(String, Instant)>,
    pub avatars: crate::avatar::Avatars,
    pub accents: HashMap<String, usize>,
    /// Split path under an active border drag, pinned to its tab so a
    /// mid-drag tab switch can never resize a colliding path elsewhere.
    drag: Option<(usize, Vec<bool>)>,
    /// Invariant: a `replayedTo` offset is valid only while nothing later streamed for that
    /// session; any update clears it, keeping a skip-ahead resume equal to a full replay.
    resume_offsets: HashMap<SessionId, u64>,
    /// The last `_yi/event` seen per session; `None` between a resume and its first event.
    seq: HashMap<SessionId, Option<EventSeq>>,
    /// Sessions whose resume brought a `_yi/replay`: a resume without one is an old daemon.
    pub(crate) replayed: std::collections::HashSet<SessionId>,
    /// The first ctrl+c anywhere; a second inside the window stops the daemon and quits.
    quit_at: Option<Instant>,
    /// When `_yi/shutdown` went out; the reply or two seconds ends the console.
    shutdown_at: Option<Instant>,
    /// Auto-opened notebook panes by their last kernel activity; an idle one closes itself.
    pub(crate) auto_notebooks: HashMap<PaneId, Instant>,
    pub window_title: String,
}

fn trace_answered(pending: &Pending) {
    if yi_types::trace::enabled() {
        let kind = format!("{:?}", pending.kind);
        let label = kind.split('(').next().unwrap_or_default();
        let name = format!("rpc {label}");
        yi_types::trace::complete(&name, pending.sent_us, serde_json::Map::new());
    }
}

fn frame(id: Option<u64>, method: &str, params: Value) -> Value {
    match id {
        Some(id) => json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
        None => json!({"jsonrpc": "2.0", "method": method, "params": params}),
    }
}

impl App {
    pub fn new(root: String, theme: Theme) -> Self {
        Self {
            state: ConsoleState::new(root),
            theme,
            kitty: false,
            pending: HashMap::new(),
            next_request: 0,
            next_list_poll: Instant::now() + LIST_POLL,
            dirty: true,
            pending_clear: false,
            notebook_image: None,
            animations: Vec::new(),
            cmd_hints: false,
            autostart: false,
            next_disk_check: Instant::now(),
            editor_drag: false,
            animate: false,
            notes: NoteQueue::default(),
            osc_flavor: OscFlavor::None,
            osc_out: Vec::new(),
            selection: None,
            selected: String::new(),
            flash: None,
            hits: None,
            avatars: crate::avatar::Avatars::default(),
            accents: HashMap::new(),
            drag: None,
            resume_offsets: HashMap::new(),
            seq: HashMap::new(),
            replayed: std::collections::HashSet::new(),
            quit_at: None,
            shutdown_at: None,
            auto_notebooks: HashMap::new(),
            window_title: String::new(),
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
                    sent_us: yi_types::trace::now_us(),
                },
            );
        } else {
            self.note("send failed: daemon link is down or wedged");
        }
        self.dirty = true;
    }

    fn note(&mut self, text: &str) {
        match self.focused_chat() {
            Some(chat) => chat.app.notice(text),
            None => self.state.banner = Some(text.to_owned()),
        }
        self.dirty = true;
    }

    pub fn accent_of(&self, key: &str, seed: &str) -> usize {
        self.accents
            .get(key)
            .copied()
            .unwrap_or_else(|| yi_tui::colors::accent_index(seed))
    }

    pub fn assign_accents(&mut self) {
        let mut keys: Vec<(String, String)> = Vec::new();
        for index in self.state.visible_rows() {
            let Some(id) = self.state.order.get(index) else {
                continue;
            };
            let Some(row) = self.state.sessions.get(id) else {
                continue;
            };
            keys.push((id.0.clone(), row.seed().to_owned()));
        }
        for index in self.state.visible_rows() {
            let Some(id) = self.state.order.get(index) else {
                continue;
            };
            for child in self.state.children.get(id).into_iter().flatten() {
                keys.push((child.id.as_str().to_owned(), child.name.clone()));
            }
        }
        let seeds: Vec<&str> = keys.iter().map(|(_, seed)| seed.as_str()).collect();
        let hues = crate::avatar::assign_accents(&seeds);
        self.accents = keys
            .into_iter()
            .zip(hues)
            .map(|((key, _), hue)| (key, hue))
            .collect();
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
        self.scroll_drag();
        if self
            .flash
            .as_ref()
            .is_some_and(|(_, at)| now.saturating_duration_since(*at) > FLASH)
        {
            self.flash = None;
            self.dirty = true;
        }
        self.close_idle_notebooks(now);
        self.tick_notes(now);
        self.check_editors(now);
        self.pump_chats(outbound);
        if self
            .shutdown_at
            .is_some_and(|at| now.saturating_duration_since(at) > Duration::from_secs(2))
        {
            self.state.quit = true;
        }
    }

    fn stop_daemon_and_quit(&mut self, outbound: &Outbound) {
        if !self.connected() {
            self.state.quit = true;
            return;
        }
        self.send_request(outbound, RequestKind::Shutdown, "_yi/shutdown", json!({}));
        self.shutdown_at = Some(Instant::now());
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
            let short = self.state.sessions.get(&note.session).map_or_else(
                || note.session.0.chars().take(12).collect(),
                SessionRow::label,
            );
            let body = match note.status {
                SessionStatus::Blocked => "needs your approval",
                _ => "done while you were away",
            };
            self.note(&format!("{short} {body}"));
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
                self.dirty = true;
            }
            ClientEvent::BadFrame => {
                self.state.dropped_frames = self.state.dropped_frames.saturating_add(1);
                self.dirty = true;
            }
            ClientEvent::Frame(value) => self.reduce_frame(outbound, value),
            ClientEvent::Input(event) => {
                let _span = yi_types::trace::span("console.input");
                self.handle_event(outbound, event);
            }
            ClientEvent::InputClosed(_) => self.state.quit = true,
        }
    }

    fn reduce_frame(&mut self, outbound: &Outbound, mut value: Value) {
        let _span = yi_types::trace::enabled().then(|| {
            let kind = value
                .pointer("/params/update/sessionUpdate")
                .and_then(Value::as_str)
                .unwrap_or("frame");
            yi_types::trace::span(format!("console.reduce {kind}"))
        });
        let method = value.get("method").and_then(Value::as_str);
        match method {
            None => self.reduce_response(outbound, &value),
            Some("session/update") => {
                if let Some(params) = value.get_mut("params").map(Value::take)
                    && let Some(update) = port::update_params(params)
                {
                    // An agent event shows only through a pane's chat, which asks for its frame.
                    let event = matches!(&update.update,
                        AcpSessionUpdate::Extension(e) if e.session_update == "_yi/event");
                    self.reduce_update(outbound, update);
                    self.dirty |= !event;
                } else {
                    self.state.dropped_frames = self.state.dropped_frames.saturating_add(1);
                    self.dirty = true;
                }
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
                        self.offer_ask(request_id, params);
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
        trace_answered(&pending);
        let _span = yi_types::trace::span("console.reduce_response");
        if pending.kind == RequestKind::Shutdown {
            self.state.quit = true;
            return;
        }
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
                if std::mem::take(&mut self.autostart) {
                    self.autostart_session(outbound);
                }
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
                self.sync_chat_names();
                if self.state.order.is_empty() {
                    self.note("no sessions for this root — alt+n starts one");
                }
            }
            RequestKind::ListDaemon => {
                self.merge_daemon_list(&result);
                self.sync_chat_names();
            }
            RequestKind::Tracked(session, paths) => self.absorb_tracked(&session, &paths, &result),
            RequestKind::BranchDiff(session) => self.absorb_branch(&session, &result),
            RequestKind::Tape(session) => self.absorb_tape(&session, &result),
            RequestKind::Why(session) => self.absorb_why(&session, &result),
            RequestKind::KernelExecute
            | RequestKind::KernelCancel
            | RequestKind::SetConfig(_)
            | RequestKind::Steer
            | RequestKind::Shutdown => {}
            RequestKind::Slash(session) => {
                if let Some(text) = result.get("text").and_then(Value::as_str) {
                    let text = text.to_owned();
                    self.fan_out(&session, || UiEvent::Reply(Reply::Notice(text.clone())));
                }
            }
            RequestKind::Rewind(session) => self.absorb_rewind(&session, &result),
            RequestKind::Plan(session) => {
                let plan = result.get("plan").cloned().unwrap_or(Value::Null);
                let subplans = result.get("subplans").cloned().unwrap_or(Value::Null);
                let decoded =
                    serde_json::from_value::<yi_types::plan::doc::Plan>(plan).map(|plan| {
                        let subplans: Vec<yi_types::plan::doc::Plan> =
                            serde_json::from_value(subplans).unwrap_or_default();
                        (plan, subplans)
                    });
                self.fan_out(&session, || {
                    UiEvent::Reply(match &decoded {
                        Ok((plan, subplans)) => Reply::Plan {
                            plan: plan.clone(),
                            subplans: subplans.clone(),
                        },
                        Err(error) => Reply::Notice(format!("/plantree: {error}")),
                    })
                });
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
                    self.absorb_result(&session, &result);
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
                self.absorb_result(&session, &result);
                if !self.replayed.contains(&session) {
                    self.note(
                        "the daemon is an older yi and streams nothing this pane can draw — \
                         restart it: pkill -f 'yi serve', then run yi again",
                    );
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
        let id = SessionId(update.session_id);
        if port::writes_transcript(&update.update) {
            self.resume_offsets.remove(&id);
            self.state.parked.remove(&id);
        }
        self.absorb_review(&id, &update.update);
        let update = match update.update {
            AcpSessionUpdate::Extension(extension) => {
                return self.reduce_extension(outbound, &id, extension);
            }
            other => other,
        };
        if let AcpSessionUpdate::StateUpdate(state) = &update {
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
            // A pane the user is looking at needs no unseen shout.
            if focused && status == SessionStatus::DoneUnseen {
                status = SessionStatus::Idle;
            }
            self.state.set_session_status(&id, status);
            if let Some(row) = self.state.sessions.get_mut(&id) {
                row.last_ms = row.last_ms.max(now_ms());
            }
            match status {
                SessionStatus::Blocked | SessionStatus::DoneUnseen => {
                    self.notes.arm(&id, status, Instant::now());
                }
                _ => self.notes.disarm(&id),
            }
        }
        if let AcpSessionUpdate::UsageUpdate { size, .. } = &update {
            for chat in self.state.chats_mut(&id) {
                chat.app.set_context_window(*size);
            }
        }
        self.absorb_edit(outbound, &id, &update);
        self.absorb_kernel(&id, &update);
        for (pane_id, pane) in &mut self.state.panes {
            if pane.session() != Some(&id) {
                continue;
            }
            if let PaneContent::Notebook { cells, .. } = &mut pane.content
                && apply_notebook(cells, &update)
            {
                pane.scroll_from_bottom = 0;
                if let Some(seen) = self.auto_notebooks.get_mut(pane_id) {
                    *seen = Instant::now();
                }
            }
        }
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

    /// Opening is not resuming: a resume is a sidebar pick, never the boot default.
    fn autostart_session(&mut self, outbound: &Outbound) {
        let bound = self
            .state
            .panes
            .values()
            .any(|pane| pane.session().is_some());
        if !bound {
            self.new_session(outbound);
        }
    }

    fn new_session(&mut self, outbound: &Outbound) {
        if let Some(pane_id) = self.state.focused_pane_id() {
            self.new_session_into(outbound, pane_id);
        }
    }

    pub(crate) fn new_session_into(&mut self, outbound: &Outbound, pane_id: PaneId) {
        if !self.connected() {
            self.note("not connected");
            return;
        }
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
            Action::SelectSlot(n) => self.select_slot(outbound, n),
            Action::Quit => self.state.quit = true,
            Action::StopDaemon => self.stop_daemon_and_quit(outbound),
            Action::Keys => self.state.mode = Mode::Keys,
            Action::ToggleSidebar => self.state.sidebar = self.state.sidebar.next(),
            Action::ToggleNotebook => self.toggle_side(outbound, diffs::SideKind::Notebook),
            Action::ToggleDiff => self.toggle_side(outbound, diffs::SideKind::Diff),
            Action::ToggleTape => self.toggle_side(outbound, diffs::SideKind::Tape),
            Action::OpenEditor => self.open_navigator("e "),
            Action::Find => self.open_navigator("/"),
            Action::Save | Action::Undo | Action::Redo => {
                if !self.editor_action(action) {
                    self.note("no editor pane is focused");
                }
            }
        }
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

    fn select_slot(&mut self, outbound: &Outbound, n: u8) {
        let session = self
            .state
            .visible_rows()
            .get(usize::from(n).saturating_sub(1))
            .and_then(|index| self.state.order.get(*index))
            .cloned();
        let Some(session) = session else {
            return self.note(&format!("no session in slot {n}"));
        };
        if let Some(pane_id) = self.state.focused_pane_id() {
            self.resume_into(outbound, pane_id, &session);
            self.state.zone = Zone::Panes;
        }
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
        let area = ratatui::layout::Rect::new(0, 0, 200, 100);
        if let Some(tab) = self.state.tab_mut() {
            tab.layout.focus_direction(direction, area);
        }
    }

    pub fn handle_event(&mut self, outbound: &Outbound, event: CtEvent) {
        match event {
            CtEvent::Key(key) => {
                if self.selection.take().is_some() {
                    self.dirty = true;
                }
                self.handle_key(outbound, key);
            }
            CtEvent::Resize(cols, rows) => {
                for pane in self.state.panes.values_mut() {
                    if let PaneContent::Session {
                        chat: Some(chat), ..
                    } = &mut pane.content
                    {
                        handle_terminal_event(
                            &mut chat.app,
                            &chat.commands.0,
                            CtEvent::Resize(cols, rows),
                        );
                    }
                }
                self.avatars.forget();
                self.pending_clear = true;
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
                    self.chat_event(CtEvent::Paste(text));
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
            return self.handle_navigator_key(outbound, key);
        }
        if matches!(self.state.mode, Mode::Keys) {
            self.state.mode = Mode::Normal;
            self.dirty = true;
            return;
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
        let on_chat = self.state.zone == Zone::Panes && self.on_chat();
        let multi = self
            .state
            .tab()
            .is_some_and(|tab| tab.layout.pane_ids().len() > 1);
        if let Some(action) = keys::direct(&key) {
            // The chord goes to the pane exactly when the screen says so: a popup
            // owns Tab, a lone chat owns the arrows (solo's child focus).
            let to_pane = match action {
                Action::ToggleZone => on_chat && self.popup_open(),
                Action::FocusLeft | Action::FocusRight | Action::FocusUp | Action::FocusDown => {
                    on_chat && !multi
                }
                Action::PageUp | Action::PageDown => self.on_editor(),
                _ => false,
            };
            if !to_pane {
                return self.apply_action(outbound, action);
            }
            if self.on_editor() {
                return self.editor_key(key);
            }
            return self.chat_event(CtEvent::Key(key));
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            let drafting = self
                .focused_chat()
                .is_some_and(|chat| !chat.app.composer_text().is_empty());
            if on_chat && drafting {
                return self.chat_event(CtEvent::Key(key));
            }
            let now = Instant::now();
            if self
                .quit_at
                .is_some_and(|at| now.saturating_duration_since(at) < QUIT_WINDOW)
            {
                return self.stop_daemon_and_quit(outbound);
            }
            self.quit_at = Some(now);
            self.note("ctrl+c again stops the daemon and quits · ⌥q leaves it running");
            if on_chat {
                self.chat_event(CtEvent::Key(key));
            }
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
                    let text = match self.state.root_filter.as_deref() {
                        Some(root) => format!(
                            "showing {} only · ←/→ walks workspaces",
                            root.rsplit('/').next().unwrap_or(root)
                        ),
                        None => "showing every workspace".to_owned(),
                    };
                    self.note(&text);
                }
                KeyCode::Enter => self.open_selected(outbound),
                _ => {}
            },
            Zone::Panes if self.on_editor() => self.editor_key(key),
            Zone::Panes if self.on_notebook() => match key.code {
                KeyCode::Esc => self.cancel_notebook_cell(outbound),
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
            Zone::Panes if self.on_review().is_some() => self.review_key(key),
            Zone::Panes if self.on_tape() => self.tape_key(key),
            Zone::Panes => self.chat_event(CtEvent::Key(key)),
        }
    }
}

mod chat;
mod diffs;
mod editor;
mod mouse;
mod navigator;
mod notebook;
pub mod port;
mod review;

pub(crate) use chat::orb_ids;
pub use mouse::MouseKind;
pub use navigator::PaletteEntry;
use notebook::apply_notebook;
