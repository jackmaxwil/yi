//! The chat half of the reducer: the composer's submit, the slash verbs, the popups
//! that stand in for the composer, and ctrl+c.

use std::time::Instant;

use ratatui::crossterm::event::KeyEvent;
use serde_json::json;
use yi_tui::keymap::SingleKey;
use yi_tui::popup::{BottomView, PopupResult};

use super::{App, QUIT_WINDOW, RequestKind};
use crate::client::Outbound;
use crate::model::{Bottom, PaneContent, SessionId};

#[rustfmt::skip]
pub(super) const CONSOLE_VERBS: [&str; 9] = [
    "new", "quit", "undo", "advisor", "plan", "goal", "permissions", "compact", "sessions",
];

impl App {
    pub(super) fn note_transcript(&mut self, session: &SessionId, text: &str) {
        for pane in self.state.panes.values_mut() {
            if pane.session() != Some(session) {
                continue;
            }
            if let PaneContent::Session { transcript, .. } = &mut pane.content {
                transcript.note(text.to_owned());
                pane.scroll_from_bottom = 0;
            }
        }
        self.dirty = true;
    }

    fn run_slash(&mut self, outbound: &Outbound, line: &str) {
        let line = line.trim();
        match line.split_whitespace().next().unwrap_or("") {
            "new" => self.new_session(outbound),
            "quit" => self.state.quit = true,
            _ => {
                if !self.connected() {
                    return self.note("not connected — command not sent");
                }
                let Some(session) = self.state.focused_session() else {
                    return self.note("no session in this pane — enter on a sidebar row first");
                };
                self.send_request(
                    outbound,
                    RequestKind::Slash(session.clone()),
                    "_yi/slash",
                    json!({"sessionId": session.0, "line": line}),
                );
            }
        }
        self.dirty = true;
    }

    pub(super) fn handle_bottom_key(
        &mut self,
        outbound: &Outbound,
        mut bottom: Bottom,
        key: KeyEvent,
    ) {
        let Some(single) = SingleKey::from_event(&key) else {
            self.bottom = Some(bottom);
            return;
        };
        match bottom.popup_mut().handle_key(&single) {
            PopupResult::Open => self.bottom = Some(bottom),
            PopupResult::Close => {}
            PopupResult::Insert(text) => match bottom {
                Bottom::Command(_) => self.run_slash(outbound, text.trim_start_matches('/')),
                Bottom::File(_) => {
                    let path = format!("{} ", text.trim_start_matches('@'));
                    let _ = self.composer.textarea.insert_str(path);
                }
            },
        }
        self.dirty = true;
    }

    pub(super) fn submit_prompt(&mut self, outbound: &Outbound) {
        if self.composer.is_empty() {
            return;
        }
        if let Some(command) = slash_line(&self.composer.text()) {
            self.composer.set_text("");
            return self.run_slash(outbound, &command);
        }
        if !self.connected() {
            self.note("not connected — prompt not sent");
            return;
        }
        let Some(focused) = self.state.focused_session() else {
            self.note("no session in this pane — enter on a sidebar row first");
            return;
        };
        let Some(text) = self.composer.take_submission() else {
            return;
        };
        self.send_request(
            outbound,
            RequestKind::Prompt,
            "session/prompt",
            json!({
                "sessionId": focused.0,
                "prompt": [{"type": "text", "text": text}],
            }),
        );
    }

    pub(super) fn cancel_turn(&mut self, outbound: &Outbound) {
        if self.connected()
            && let Some(session) = self.state.focused_session()
        {
            self.send_request(
                outbound,
                RequestKind::Cancel,
                "session/cancel",
                json!({"sessionId": session.0}),
            );
        }
    }

    pub(super) fn interrupt(&mut self, outbound: &Outbound) {
        if !self.composer.is_empty() {
            self.composer.set_text("");
            self.dirty = true;
            return;
        }
        let now = Instant::now();
        if self
            .interrupt_at
            .is_some_and(|at| now.saturating_duration_since(at) < QUIT_WINDOW)
        {
            self.state.quit = true;
            return;
        }
        self.interrupt_at = Some(now);
        self.cancel_turn(outbound);
        self.note("interrupted — ctrl+c again quits");
    }
}

fn slash_line(text: &str) -> Option<String> {
    let rest = text.trim().strip_prefix('/')?;
    let head = rest.split_whitespace().next()?;
    CONSOLE_VERBS
        .contains(&head)
        .then(|| rest.trim().to_owned())
}
