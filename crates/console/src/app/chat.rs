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

use super::port::{
    Config, Decoded, PortRequest, RemotePort, Replay, config_of, decode, option_for,
};
use super::{App, RequestKind};
use crate::client::Outbound;
use crate::layout::PaneId;
use crate::model::{Chat, PaneContent, PendingAsk, SessionId, SessionRow};

const ORB_ID_BASE: u32 = 7800;

fn orb_ids(pane: PaneId) -> [u32; 2] {
    let base = ORB_ID_BASE.saturating_add(pane.raw().saturating_mul(2));
    [base, base.saturating_add(1)]
}

impl App {
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
        Box::new(Chat {
            app,
            port: RemotePort::default(),
            orb: yi_tui::orb::Tick::with_ids(orb_ids(pane_id)),
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
        for pane in self.state.panes.values_mut() {
            if let PaneContent::Session {
                session: Some(bound),
                chat: Some(chat),
            } = &mut pane.content
                && bound == session
            {
                let _ = chat.events.0.send(make());
                pane.scroll_from_bottom = 0;
            }
        }
        self.dirty = true;
    }

    pub(super) fn drop_frame(&mut self) {
        self.state.dropped_frames = self.state.dropped_frames.saturating_add(1);
        self.dirty = true;
    }

    pub(super) fn reduce_extension(
        &mut self,
        outbound: &Outbound,
        id: &SessionId,
        extension: &AcpExtensionUpdate,
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
            Decoded::Config(config) => self.apply_config(id, &config, true),
            Decoded::Child(child) => {
                let rows = self.state.children.entry(id.clone()).or_default();
                match rows.iter().position(|row| row.id == child.id) {
                    Some(at) => {
                        if let Some(slot) = rows.get_mut(at) {
                            *slot = child.clone();
                        }
                    }
                    None if rows.len() < 32 => rows.push(child.clone()),
                    None => {}
                }
                self.fan_out(id, || UiEvent::ChildUpdates(vec![child.clone()]));
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

    pub(super) fn pump_chats(&mut self, outbound: &Outbound) {
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
            let busy = chat.app.is_running() || chat.app.orb_animating();
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
            if busy {
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
                    RequestKind::SetConfig,
                    "session/set_config_option",
                    json!({"sessionId": id, "configId": "model", "value": value}),
                );
                self.send_request(
                    outbound,
                    RequestKind::SetConfig,
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
            handle_terminal_event(&mut chat.app, &chat.commands.0, event);
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
