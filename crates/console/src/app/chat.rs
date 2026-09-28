//! Solo's chat bound to a pane: the `App` is built at bind time, fed the wire's events,
//! and drained of what it asks its session — one place per direction.

use std::sync::mpsc::TryRecvError;

use ratatui::crossterm::event::{Event as CtEvent, KeyCode, KeyEvent};
use serde_json::{Value, json};
use yi_tui::app::command_channel;
use yi_tui::input::handle_terminal_event;
use yi_tui::{AskChoice, AskRequest, Command, Reply, TuiOptions, UiEvent};
use yi_types::acp::{AcpExtensionUpdate, AcpPermissionParams, AcpSessionResult};
use yi_types::entry::Entry;
use yi_types::subagent::{ChildStatus, ChildUpdate};

use super::port::{
    Config, Decoded, PortRequest, RemotePort, Replay, config_of, decode, option_for,
};
use super::{App, RequestKind};
use crate::client::Outbound;
use crate::layout::PaneId;
use crate::model::{Chat, PaneContent, PendingAsk, SessionId, SessionRow};

const ORB_ID_BASE: u32 = 7800;

const CHILD_ROW_CAP: usize = 32;

/// A child first heard of at its end (retired before this console saw it run) gets no row;
/// a full cache gives up its oldest finished row, never the new one.
fn keep_child_row(rows: &mut Vec<ChildUpdate>, child: &ChildUpdate) {
    if let Some(slot) = rows.iter_mut().find(|row| row.id == child.id) {
        *slot = child.clone();
        return;
    }
    if child.status != ChildStatus::Running {
        return;
    }
    if rows.len() >= CHILD_ROW_CAP {
        let oldest_finished = rows
            .iter()
            .position(|row| row.status != ChildStatus::Running)
            .unwrap_or(0);
        rows.remove(oldest_finished);
    }
    rows.push(child.clone());
}

pub(crate) fn orb_ids(pane: PaneId) -> [u32; 2] {
    let base = ORB_ID_BASE.saturating_add(pane.raw().saturating_mul(2));
    [base, base.saturating_add(1)]
}

impl App {
    pub(super) fn chat_for(&mut self, pane_id: PaneId, session: &SessionId) -> Box<Chat> {
        match self.state.parked.remove(session) {
            Some(mut chat) => {
                chat.orb = yi_tui::orb::Tick::with_ids(orb_ids(pane_id));
                chat.logos = yi_tui::logos::Tick::default();
                chat
            }
            None => self.make_chat(pane_id, session),
        }
    }

    pub(super) fn make_chat(&self, pane_id: PaneId, session: &SessionId) -> Box<Chat> {
        let row = self.state.sessions.get(session);
        let options = TuiOptions {
            model: yi_tui::model::model_or_stub("", ""),
            session_name: row.map_or_else(|| session.0.clone(), SessionRow::label),
            cwd: row.map_or_else(|| self.state.root.clone(), |row| row.root.clone()),
            lane: None,
            context_window: 0,
            session_dir: String::new(),
            keys: Vec::new(),
            initial_prompt: None,
            pace: 0,
        };
        let mut app =
            yi_tui::app::App::new(options, self.theme, yi_tui::keymap::default_keymap(), 80);
        let _ = app.take_title();
        app.set_status_name_shown(false);
        app.set_pane();
        Box::new(Chat {
            app,
            port: RemotePort::default(),
            orb: yi_tui::orb::Tick::with_ids(orb_ids(pane_id)),
            phase: 0,
            logos: yi_tui::logos::Tick::default(),
            ask: None,
            events: std::sync::mpsc::channel(),
            commands: command_channel(),
        })
    }

    pub(super) fn focused_chat(&mut self) -> Option<&mut Chat> {
        match &mut self.state.focused_pane_mut()?.content {
            PaneContent::Session {
                chat: Some(chat), ..
            } => Some(chat.as_mut()),
            _ => None,
        }
    }

    pub(super) fn on_chat(&self) -> bool {
        self.state
            .focused_pane()
            .is_some_and(|pane| matches!(pane.content, PaneContent::Session { chat: Some(_), .. }))
    }

    pub(super) fn popup_open(&self) -> bool {
        self.state
            .focused_pane()
            .is_some_and(|pane| match &pane.content {
                PaneContent::Session {
                    chat: Some(chat), ..
                } => chat.app.bottom_open(),
                _ => false,
            })
    }

    pub fn chat_running(&self) -> bool {
        self.state
            .focused_pane()
            .is_some_and(|pane| match &pane.content {
                PaneContent::Session {
                    chat: Some(chat), ..
                } => chat.app.is_running(),
                _ => false,
            })
    }

    pub(super) fn fan_out(&mut self, session: &SessionId, make: impl Fn() -> UiEvent) {
        if let Some(chat) = self.state.parked.get_mut(session) {
            let _ = chat.events.0.send(make());
        }
        for pane in self.state.panes.values_mut() {
            if let PaneContent::Session {
                session: Some(bound),
                chat: Some(chat),
            } = &mut pane.content
                && bound == session
            {
                // Incident: resetting the scroll here pinned every reader to the bottom mid-turn.
                let _ = chat.events.0.send(make());
                self.dirty = true;
            }
        }
    }

    pub(super) fn drop_frame(&mut self) {
        self.state.dropped_frames = self.state.dropped_frames.saturating_add(1);
        self.dirty = true;
    }

    pub(super) fn reduce_extension(
        &mut self,
        outbound: &Outbound,
        id: &SessionId,
        extension: AcpExtensionUpdate,
    ) {
        let Ok(decoded) = decode(extension) else {
            return self.drop_frame();
        };
        match decoded {
            Decoded::Event { seq, child, event } => {
                let last = self.seq.get(id).copied().flatten();
                if !seq.follows(last) {
                    return self.heal(outbound, id);
                }
                self.seq.insert(id.clone(), Some(seq));
                self.fan_out(id, || match &child {
                    Some(child_id) => UiEvent::Child {
                        child_id: child_id.clone(),
                        event: (*event).clone(),
                    },
                    None => UiEvent::Agent((*event).clone()),
                });
            }
            Decoded::Gap => self.heal(outbound, id),
            Decoded::Replay(replay, entries) => self.absorb_replay(id, &replay, entries),
            Decoded::Goal(goal) => {
                for chat in self.state.chats_mut(id) {
                    chat.port.set_goal(goal.clone());
                }
                self.dirty = true;
            }
            Decoded::Workdir { cwd, lane } => {
                for chat in self.state.chats_mut(id) {
                    chat.app.set_workdir(cwd.clone(), lane.clone());
                }
                self.dirty = true;
            }
            Decoded::Todo(todos) => {
                for chat in self.state.chats_mut(id) {
                    chat.port.set_todos(todos.clone());
                }
                self.dirty = true;
            }
            Decoded::Name(name) => {
                if let Some(row) = self.state.sessions.get_mut(id) {
                    row.name = Some(name.clone());
                }
                for chat in self.state.chats_mut(id) {
                    chat.app.set_session_name(name.clone());
                }
                self.dirty = true;
            }
            Decoded::Claims(claims) => {
                for chat in self.state.chats_mut(id) {
                    chat.port.set_claims(claims.clone());
                }
                self.dirty = true;
            }
            Decoded::Plan(plan) => {
                for chat in self.state.chats_mut(id) {
                    chat.port.set_plan(plan.clone());
                }
                self.dirty = true;
            }
            Decoded::Config(config) => {
                // A pick's two requests each echo a frame before their answers; only the last announces.
                let own = RequestKind::SetConfig(id.clone());
                let in_flight = self.pending.values().filter(|p| p.kind == own).count();
                self.apply_config(id, &config, in_flight <= 1);
            }
            Decoded::Child(child) => {
                keep_child_row(self.state.children.entry(id.clone()).or_default(), &child);
                self.fan_out(id, || UiEvent::ChildUpdates(vec![child.clone()]));
            }
            Decoded::Notice(text) => {
                self.fan_out(id, || UiEvent::Reply(Reply::Notice(text.clone())))
            }
            Decoded::Other => {}
        }
    }

    fn heal(&mut self, outbound: &Outbound, id: &SessionId) {
        self.seq.insert(id.clone(), None);
        self.resume_offsets.remove(id);
        let pane = self
            .state
            .panes
            .iter()
            .find(|(_, pane)| pane.session() == Some(id))
            .map(|(pane_id, _)| *pane_id);
        if let Some(pane_id) = pane {
            self.resume_into(outbound, pane_id, id);
        }
    }

    fn absorb_replay(&mut self, id: &SessionId, replay: &Replay, entries: Vec<Entry>) {
        if replay.leaf.is_some() && yi_types::trace::enabled() {
            let mut args = serde_json::Map::new();
            args.insert("session".to_owned(), json!(id.0));
            args.insert(
                "entries".to_owned(),
                json!(replay.from.saturating_add(entries.len() as u64)),
            );
            yi_types::trace::instant("console.replayed", args);
        }
        self.seq.insert(id.clone(), None);
        self.replayed.insert(id.clone());
        if let (Some(name), Some(row)) = (&replay.name, self.state.sessions.get_mut(id)) {
            row.name = Some(name.clone());
        }
        for chat in self.state.chats_mut(id) {
            if replay.child.is_none() {
                chat.port.absorb_replay(replay, entries.clone());
            }
            if let Some(name) = &replay.name {
                chat.app.set_session_name(name.clone());
            }
            if let Some(window) = replay.context_window {
                chat.app.set_context_window(window);
            }
        }
        let (child, from) = (replay.child.clone(), replay.from);
        self.fan_out(id, || {
            UiEvent::Reply(match &child {
                Some(child_id) => Reply::ChildHistory {
                    child_id: child_id.clone(),
                    entries: entries.clone(),
                },
                // A replay from the root is the branch as it now stands: the
                // rewound shape resets before it replays.
                None if from == 0 => Reply::Rewound {
                    entries: entries.clone(),
                    unsent: None,
                    abandoned: None,
                },
                None => Reply::History(entries.clone()),
            })
        });
    }

    pub(super) fn apply_config(&mut self, id: &SessionId, config: &Config, announce: bool) {
        let Some((provider, model_id)) = &config.model else {
            return;
        };
        let model = yi_tui::model::model_or_stub(provider, model_id);
        let effort = config.effort.unwrap_or_default();
        let changed = self.state.chats_mut(id).iter().any(|chat| {
            let shown = &chat.app.selection;
            !shown.model.provider.is_empty()
                && ((&shown.model.provider, &shown.model.id) != (provider, model_id)
                    || shown.effort != effort)
        });
        let announce = announce && changed;
        for chat in self.state.chats_mut(id) {
            if let Some(window) = config.context_window {
                chat.app.set_context_window(window);
            }
            chat.app.set_model_selector(model.clone(), effort);
        }
        if announce {
            self.fan_out(id, || {
                UiEvent::Reply(Reply::Selected {
                    model: model.clone(),
                    effort,
                })
            });
        }
        self.dirty = true;
    }

    pub(super) fn sync_chat_names(&mut self) {
        let labels: Vec<(SessionId, String)> = self
            .state
            .sessions
            .values()
            .map(|row| (row.id.clone(), row.label()))
            .collect();
        for (id, label) in labels {
            for chat in self.state.chats_mut(&id) {
                chat.app.set_session_name(label.clone());
            }
        }
    }

    pub(super) fn absorb_result(&mut self, id: &SessionId, result: &Value) {
        let Ok(result) = serde_json::from_value::<AcpSessionResult>(result.clone()) else {
            return;
        };
        if let Some(name) = &result.name {
            if let Some(row) = self.state.sessions.get_mut(id) {
                row.name = Some(name.clone());
            }
            for chat in self.state.chats_mut(id) {
                chat.app.set_session_name(name.clone());
            }
        }
        let config = config_of(&result.config_options);
        self.apply_config(id, &config, false);
    }

    /// The ask lands in one pane per session: the focused one if it shows the session,
    /// else the lowest; none yet, and it waits for a pane to open.
    pub(super) fn offer_ask(&mut self, request_id: String, params: AcpPermissionParams) {
        let session = SessionId(params.session_id.clone());
        let focused = self.state.focused_pane_id();
        let mut targets: Vec<PaneId> = self
            .state
            .panes
            .iter()
            .filter(|(_, pane)| {
                pane.session() == Some(&session)
                    && matches!(pane.content, PaneContent::Session { chat: Some(_), .. })
            })
            .map(|(pane_id, _)| *pane_id)
            .collect();
        targets.sort();
        let target = targets
            .iter()
            .find(|pane_id| Some(**pane_id) == focused)
            .or_else(|| targets.first())
            .copied();
        let Some(pane_id) = target else {
            if self.state.orphan_asks.len() < 32 {
                self.state.orphan_asks.push((request_id, params));
            }
            return;
        };
        let Some(PaneContent::Session {
            chat: Some(chat), ..
        }) = self
            .state
            .panes
            .get_mut(&pane_id)
            .map(|pane| &mut pane.content)
        else {
            return;
        };
        let grants = crate::app::port::grant_labels(&params.options);
        let (tx, rx) = std::sync::mpsc::channel();
        chat.ask = Some(PendingAsk {
            request_id,
            reply: rx,
            options: params.options,
        });
        let _ = chat.events.0.send(UiEvent::Ask(AskRequest {
            title: params.title,
            description: params
                .description
                .unwrap_or_else(|| "the agent asks for permission".to_owned()),
            grants,
            reply: tx,
            tool_call_id: None,
        }));
        self.dirty = true;
    }

    pub(super) fn deliver_orphan_asks(&mut self, session: &SessionId) {
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.state.orphan_asks)
            .into_iter()
            .partition(|(_, params)| params.session_id == session.0);
        self.state.orphan_asks = rest;
        for (request_id, params) in mine {
            self.offer_ask(request_id, params);
        }
    }

    fn answer_ask(&mut self, outbound: &Outbound, ask: PendingAsk, choice: Option<AskChoice>) {
        // A popup that closed without a verdict is a rejection, never a hung worker.
        let option = option_for(choice.unwrap_or(AskChoice::Reject), &ask.options);
        let response = json!({
            "jsonrpc": "2.0",
            "id": ask.request_id,
            "result": {"outcome": {"outcome": "selected", "optionId": option}},
        });
        if !outbound.send(&response) {
            self.note("send failed: permission answer not delivered");
        }
        self.dirty = true;
    }

    /// A chat already on the session keeps its draft; the replay that follows resets the
    /// transcript. Any other content gives way to a fresh chat.
    pub(super) fn bind_pane(&mut self, pane_id: PaneId, session: &SessionId) {
        let keep = matches!(
            self.state.panes.get(&pane_id).map(|pane| &pane.content),
            Some(PaneContent::Session { session: Some(bound), chat: Some(_) }) if bound == session
        );
        let fresh = (!keep).then(|| self.chat_for(pane_id, session));
        let Some(pane) = self.state.panes.get_mut(&pane_id) else {
            return;
        };
        let mut left = None;
        match (&mut pane.content, fresh) {
            (PaneContent::Notebook { session: slot, .. }, _) => *slot = Some(session.clone()),
            (_, Some(chat)) => {
                left = Some(std::mem::replace(
                    &mut pane.content,
                    PaneContent::Session {
                        session: Some(session.clone()),
                        chat: Some(chat),
                    },
                ));
            }
            (_, None) => {}
        }
        pane.scroll_from_bottom = 0;
        if let Some(PaneContent::Session {
            session: Some(old),
            chat: Some(chat),
        }) = left
            && old != *session
            && !self.state.session_visible(&old)
        {
            self.state.park(old, chat);
        }
        self.state.banner = None;
        self.deliver_orphan_asks(session);
    }

    pub(super) fn resume_into(
        &mut self,
        outbound: &Outbound,
        pane_id: PaneId,
        session: &SessionId,
    ) {
        if yi_types::trace::enabled() {
            let mut args = serde_json::Map::new();
            args.insert("session".to_owned(), json!(session.0));
            yi_types::trace::instant("console.switch", args);
        }
        // Invariant: replay streams ahead of the resume response, so wipe and bind happen at
        // send time; an offset skips the wipe only for this pane's session or its parked chat.
        let continuous = self
            .state
            .panes
            .get(&pane_id)
            .is_some_and(|pane| pane.session() == Some(session));
        let parked = self.state.parked.contains_key(session);
        let offset = if continuous || parked {
            self.resume_offsets.get(session).copied()
        } else {
            None
        };
        if parked && offset.is_some() {
            self.bind_pane(pane_id, session);
        }
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
                "replayUpdates": false,
            }),
        );
    }

    pub(super) fn pump_chats(&mut self, outbound: &Outbound) {
        self.ask_branches(outbound);
        let ids: Vec<PaneId> = self.state.panes.keys().copied().collect();
        for pane_id in ids {
            let Some(session) = self
                .state
                .panes
                .get(&pane_id)
                .and_then(|pane| pane.session().cloned())
            else {
                continue;
            };
            let Some(PaneContent::Session {
                chat: Some(chat), ..
            }) = self
                .state
                .panes
                .get_mut(&pane_id)
                .map(|pane| &mut pane.content)
            else {
                continue;
            };
            let _span = yi_types::trace::span("console.chat_tick");
            yi_tui::tick(
                &mut chat.app,
                &mut chat.port,
                &chat.events.1,
                &chat.commands.0,
            );
            if chat.app.take_pending_editor() {
                chat.app
                    .notice("the editor is a pane here: ⌥e opens a file beside the chat");
            }
            let requests = std::mem::take(&mut chat.port.queue);
            let mut commands = Vec::new();
            while let Ok(command) = chat.commands.1.try_recv() {
                commands.push(command);
            }
            let quit = chat.app.take_quit();
            let redraw = chat.app.take_redraw(&mut chat.phase);
            let verdict = match &chat.ask {
                Some(ask) => match ask.reply.try_recv() {
                    Ok(choice) => Some(Some(choice)),
                    Err(TryRecvError::Disconnected) => Some(None),
                    Err(TryRecvError::Empty) => None,
                },
                None => None,
            };
            let answered = verdict.and_then(|choice| chat.ask.take().map(|ask| (ask, choice)));
            if quit {
                self.state.quit = true;
            }
            if redraw {
                self.dirty = true;
            }
            if let Some((ask, choice)) = answered {
                self.answer_ask(outbound, ask, choice);
            }
            for request in requests {
                self.send_port_request(outbound, pane_id, &session, request);
            }
            for command in commands {
                self.send_command(outbound, &session, command);
            }
        }
    }

    fn send_command(&mut self, outbound: &Outbound, session: &SessionId, command: Command) {
        let id = session.0.as_str();
        match command {
            Command::Prompt(text) => {
                if !self.connected() {
                    return self.note("not connected — prompt not sent");
                }
                self.send_request(
                    outbound,
                    RequestKind::Prompt,
                    "session/prompt",
                    json!({"sessionId": id, "prompt": [{"type": "text", "text": text}]}),
                );
            }
            Command::Steer(text) => self.send_request(
                outbound,
                RequestKind::Steer,
                "_yi/steer",
                json!({"sessionId": id, "text": text}),
            ),
            Command::Abort => self.send_request(
                outbound,
                RequestKind::Cancel,
                "session/cancel",
                json!({"sessionId": id}),
            ),
            Command::StopChild(child) => self.send_request(
                outbound,
                RequestKind::Steer,
                "_yi/child_abort",
                json!({"sessionId": id, "childId": child}),
            ),
            Command::ChildHistory(child) => self.send_request(
                outbound,
                RequestKind::Steer,
                "_yi/child_replay",
                json!({"sessionId": id, "childId": child}),
            ),
            Command::Slash(line) => self.send_request(
                outbound,
                RequestKind::Slash(session.clone()),
                "_yi/slash",
                json!({"sessionId": id, "line": line}),
            ),
            Command::Answer {
                child_id,
                question,
                text,
            } => self.send_request(
                outbound,
                RequestKind::Slash(session.clone()),
                "_yi/child_answer",
                json!({"sessionId": id, "childId": child_id, "questionId": question, "text": text}),
            ),
            // The worker summarises what it rewound; shutdown is the console's own.
            Command::SummarizeBranch(_) | Command::Shutdown => {}
        }
    }

    fn send_port_request(
        &mut self,
        outbound: &Outbound,
        pane_id: PaneId,
        session: &SessionId,
        request: PortRequest,
    ) {
        if !self.connected() {
            return self.note("not connected — command not sent");
        }
        let id = session.0.as_str();
        match request {
            PortRequest::New => self.new_session_into(outbound, pane_id),
            PortRequest::Rewind(entry_id) => self.send_request(
                outbound,
                RequestKind::Rewind(session.clone()),
                "_yi/rewind",
                json!({"sessionId": id, "entryId": entry_id}),
            ),
            PortRequest::RewindFiles(entry_id) => self.send_request(
                outbound,
                RequestKind::Rewind(session.clone()),
                "_yi/rewind",
                json!({"sessionId": id, "entryId": entry_id, "files": true}),
            ),
            PortRequest::Undo => self.send_request(
                outbound,
                RequestKind::Slash(session.clone()),
                "_yi/slash",
                json!({"sessionId": id, "line": "undo"}),
            ),
            PortRequest::Slash(line) => self.send_request(
                outbound,
                RequestKind::Slash(session.clone()),
                "_yi/slash",
                json!({"sessionId": id, "line": line}),
            ),
            PortRequest::Select(model, effort) => {
                let value = format!("{}/{}", model.provider, model.id);
                drop(model);
                self.send_request(
                    outbound,
                    RequestKind::SetConfig(session.clone()),
                    "session/set_config_option",
                    json!({"sessionId": id, "configId": "model", "value": value}),
                );
                self.send_request(
                    outbound,
                    RequestKind::SetConfig(session.clone()),
                    "session/set_config_option",
                    json!({"sessionId": id, "configId": "thought_level", "value": effort.to_string()}),
                );
            }
            PortRequest::Plan => self.send_request(
                outbound,
                RequestKind::Plan(session.clone()),
                "_yi/plan",
                json!({"sessionId": id}),
            ),
        }
    }

    pub(super) fn chat_event(&mut self, event: CtEvent) {
        self.dirty = true;
        if let Some(chat) = self.focused_chat() {
            let enter = matches!(event, CtEvent::Key(key) if key.code == KeyCode::Enter);
            let drafted = enter && !chat.app.composer_text().is_empty();
            handle_terminal_event(&mut chat.app, &chat.commands.0, event);
            let sent = drafted && chat.app.composer_text().is_empty();
            if let Some(pane) = self.state.focused_pane_mut()
                && sent
            {
                pane.scroll_from_bottom = 0;
            }
            return;
        }
        if let CtEvent::Key(KeyEvent {
            code: KeyCode::Enter,
            ..
        }) = event
        {
            if self.connected() {
                self.note("no session in this pane — enter on a sidebar row first");
            } else {
                self.note("not connected — prompt not sent");
            }
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_file_restore_from_the_tape_waits_for_a_second_press() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use yi_types::tape::{Mark, MarkKind, Tape};
        let theme = yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::TrueColor, true);
        let mut app = App::new("/r".to_owned(), theme);
        let session = SessionId("s-a".to_owned());
        let pane = app.state.focused_pane_id().expect("a pane");
        let chat = app.make_chat(pane, &session);
        app.state.parked.insert(session.clone(), chat);
        if let Some(focused) = app.state.focused_pane_mut() {
            focused.content = PaneContent::Tape {
                session: session.clone(),
                tape: Some(Tape {
                    start: 0,
                    end: 10,
                    marks: vec![Mark {
                        at: 0,
                        kind: MarkKind::User,
                        entry: "u1".to_owned(),
                        label: "first".to_owned(),
                    }],
                    ..Tape::default()
                }),
                cursor: 0,
                armed: false,
            };
        }
        let press = |app: &mut App| {
            app.tape_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));
        };
        let queued = |app: &mut App| {
            app.state
                .chats_mut(&session)
                .into_iter()
                .map(|chat| chat.port.queue.len())
                .sum::<usize>()
        };
        press(&mut app);
        assert_eq!(queued(&mut app), 0, "the first press only arms");
        press(&mut app);
        assert!(matches!(
            app.state.chats_mut(&session).into_iter().next().and_then(|chat| chat.port.queue.first()),
            Some(super::super::port::PortRequest::RewindFiles(entry)) if entry == "u1"
        ));
    }

    /// Dies with `w` doing nothing after ↓ ran past the last file or a scope switch shortened
    /// the list, while the `▸` still sat on a file; then with every blame error read as uncommitted.
    #[test]
    fn why_asks_for_the_marked_file_and_its_rows_say_why_a_chain_is_missing() {
        use crate::model::{FileDiff, ReviewScope};
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let theme = yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::TrueColor, true);
        let mut app = App::new("/r".to_owned(), theme);
        let session = SessionId("s-a".to_owned());
        if let Some(focused) = app.state.focused_pane_mut() {
            focused.content = PaneContent::SessionDiff {
                session: session.clone(),
                scope: ReviewScope::Session,
            };
        }
        let diff = app.state.diffs.entry(session.clone()).or_default();
        for path in ["src/a.rs", "src/b.rs"] {
            let patch = format!("--- a/{path}\n+++ b/{path}\n@@ -1 +1,2 @@\n line\n+added\n");
            let file = FileDiff {
                patch,
                added: 1,
                removed: 0,
                tracked: true,
                serving: None,
                turn: 0,
            };
            diff.files.push((path.to_owned(), file));
        }
        let key = |app: &mut App, code: KeyCode| {
            app.review_key(KeyEvent::new(code, KeyModifiers::NONE));
        };
        let asked = |app: &mut App| {
            app.state
                .diffs
                .get_mut(&session)
                .and_then(|diff| diff.why_due.take())
                .map(|(path, _)| path)
        };
        for _ in 0..3 {
            key(&mut app, KeyCode::Down);
        }
        key(&mut app, KeyCode::Char('w'));
        assert_eq!(asked(&mut app).as_deref(), Some("src/b.rs"), "past the end");
        for _ in 0..3 {
            key(&mut app, KeyCode::Char('s'));
        }
        key(&mut app, KeyCode::Char('w'));
        assert_eq!(
            asked(&mut app).as_deref(),
            Some("src/a.rs"),
            "a scope switch"
        );

        let mut answers = vec![
            serde_json::json!({"line": 2, "uncommitted": true}),
            serde_json::json!({"line": 40, "error": "git blame failed: fatal: file src/a.rs has only 3 lines"}),
        ];
        answers.extend((3..9).map(
            |n| serde_json::json!({"line": n * 10, "commit": "1a2b3c4d5e6f", "subject": "Add a"}),
        ));
        let reply = serde_json::json!({"path": "src/a.rs", "cap": 8, "unasked": [90, 120], "answers": answers});
        app.absorb_why(&session, &reply);
        let (_, lines) = crate::render::review_view(
            app.state.diffs.get(&session),
            ReviewScope::Session,
            "",
            200,
            &theme,
        );
        let rows: Vec<String> = lines.iter().map(ToString::to_string).collect();
        for row in [
            "  ↳ L2 not committed yet, so no chain",
            "  ↳ L40 no chain: git blame failed: fatal: file src/a.rs has only 3 lines",
            "  […] 8 of 10 hunks asked (_yi/why answers 8 a call); the next: yi why src/a.rs:90",
        ] {
            assert!(rows.iter().any(|drawn| drawn == row), "{row}\n{rows:#?}");
        }
    }

    #[test]
    fn a_red_gate_ranks_between_done_and_working() {
        use crate::model::{SessionRow, SessionStatus};
        use yi_types::lane::{JobState, Landing, LandingJob, PrNumber};
        let theme = yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::TrueColor, true);
        let mut app = App::new("/r".to_owned(), theme);
        for (id, status, last_ms) in [
            ("s-work", SessionStatus::Working, 30),
            ("s-red", SessionStatus::Working, 10),
            ("s-done", SessionStatus::DoneUnseen, 5),
        ] {
            app.state.upsert_row(SessionRow {
                id: SessionId(id.to_owned()),
                root: "/r".to_owned(),
                status,
                attached: true,
                name: Some(id.to_owned()),
                created_ms: 0,
                last_ms,
            });
        }
        let red = SessionId("s-red".to_owned());
        let pane = app.state.focused_pane_id().expect("a pane");
        let mut chat = app.make_chat(pane, &red);
        chat.app
            .reduce_agent(yi_types::event::AgentEvent::LandingState {
                landing: Landing::Open {
                    pr: PrNumber(612),
                    jobs: vec![LandingJob {
                        name: "gate (test)".to_owned(),
                        state: JobState::Red,
                    }],
                    behind: 0,
                },
            });
        app.state.parked.insert(red, chat);
        let order: Vec<String> = app
            .state
            .visible_rows()
            .into_iter()
            .filter_map(|index| app.state.order.get(index).map(|id| id.0.clone()))
            .collect();
        assert_eq!(order, ["s-done", "s-red", "s-work"]);
    }
    use super::*;
    use yi_types::subagent::{ChildActivity, ChildId};

    fn update(id: usize, status: ChildStatus) -> ChildUpdate {
        ChildUpdate {
            id: ChildId(format!("sub-{id}")),
            name: format!("child {id}"),
            status,
            activity: ChildActivity::Waiting,
            tool_use_count: 0,
            token_count: 0,
            answer_preview: None,
            error: None,
            exit: None,
            flag: None,
        }
    }

    /// Guards the row cache: row thirty-three was dropped without a word, and a child retired
    /// before the console saw it run was given a row nothing would ever update again.
    #[test]
    fn a_child_first_seen_at_its_end_gets_no_row_and_row_thirty_three_is_kept() {
        let mut rows = Vec::new();
        keep_child_row(&mut rows, &update(99, ChildStatus::Error));
        assert!(
            rows.is_empty(),
            "a retired stranger leaves no row: {rows:?}"
        );
        for id in 0..CHILD_ROW_CAP {
            keep_child_row(&mut rows, &update(id, ChildStatus::Running));
        }
        keep_child_row(&mut rows, &update(5, ChildStatus::Completed));
        assert_eq!(rows.len(), CHILD_ROW_CAP, "an update replaces its own row");
        keep_child_row(&mut rows, &update(32, ChildStatus::Running));
        assert_eq!(rows.len(), CHILD_ROW_CAP);
        assert!(
            rows.iter().any(|row| row.id.as_str() == "sub-32"),
            "{rows:?}"
        );
        assert!(
            !rows.iter().any(|row| row.id.as_str() == "sub-5"),
            "the finished row made way, not a running one: {rows:?}"
        );
        assert!(rows.iter().any(|row| row.id.as_str() == "sub-0"));
    }
}
