use serde_json::{Value, json};
use yi_tui::{Reply, UiEvent};
use yi_types::config::RewriteLength;

use super::{App, RequestKind};
use crate::client::Outbound;
use crate::layout::PaneId;
use crate::model::{PaneContent, SessionId};
use std::time::Duration;

/// A rewrite may take the rewriter's own 20 s deadline, so the console waits past it.
const REWRITE_DEADLINE: Duration = Duration::from_secs(30);

pub(super) fn deadline_of(kind: &RequestKind) -> Duration {
    match kind {
        RequestKind::Rewrite(..) => REWRITE_DEADLINE,
        _ => super::REQUEST_DEADLINE,
    }
}

impl App {
    /// A press on a position of a chat's improve control.
    pub(super) fn pick_rewrite(&mut self, pane: PaneId, position: Option<RewriteLength>) {
        if let Some(PaneContent::Session {
            chat: Some(chat), ..
        }) = self
            .state
            .panes
            .get_mut(&pane)
            .map(|pane| &mut pane.content)
        {
            chat.app.pick_rewrite(position);
        }
        self.dirty = true;
    }

    pub(super) fn send_rewrite(
        &mut self,
        outbound: &Outbound,
        session: &SessionId,
        (ask, length, draft): (u64, RewriteLength, String),
    ) {
        if !self.connected() {
            let outcome = Err("not connected to the daemon".to_owned());
            return self.answer_rewrite(session, ask, outcome);
        }
        let params = json!({"sessionId": session.0, "length": length, "draft": draft});
        let kind = RequestKind::Rewrite(session.clone(), ask);
        self.send_request(outbound, kind, "_yi/rewrite", params);
    }

    pub(super) fn absorb_rewrite(&mut self, session: &SessionId, ask: u64, reply: &Value) {
        let text = |key: &str| {
            reply
                .pointer(&format!("/result/{key}"))
                .and_then(Value::as_str)
        };
        let outcome = match (text("text"), text("reason")) {
            (Some(rewrite), _) => Ok(rewrite.to_owned()),
            (None, Some(reason)) => Err(reason.to_owned()),
            (None, None) => Err(reply
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("the daemon sent no rewrite")
                .to_owned()),
        };
        self.answer_rewrite(session, ask, outcome);
    }

    pub(super) fn answer_rewrite(
        &mut self,
        session: &SessionId,
        ask: u64,
        outcome: Result<String, String>,
    ) {
        self.fan_out(session, || {
            UiEvent::Reply(Reply::Rewritten {
                ask,
                outcome: outcome.clone(),
            })
        });
    }

    /// The session's chats learn their composer control from its open result; a chat built
    /// after that is told by [`App::make_chat`].
    pub(super) fn learn_rewriter(&mut self, session: &SessionId, start: RewriteLength) {
        self.state.rewriters.insert(session.clone(), start);
        self.fan_out(session, || UiEvent::Reply(Reply::Rewriter(start)));
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use yi_tui::colors::{ColorTier, Theme};

    use super::super::{App, MouseKind};
    use crate::present;

    fn painted(terminal: &Terminal<ratatui::backend::TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        let width = usize::from(buffer.area.width);
        let cells: Vec<&str> = buffer.content().iter().map(|cell| cell.symbol()).collect();
        cells.chunks(width).map(|row| row.concat() + "\n").collect()
    }

    /// The control is drawn by the chat and clicked through the console's hit table: a press
    /// on a position's rectangle picks it, and the next paint marks that one.
    #[test]
    fn a_press_on_a_position_of_the_improve_control_picks_it()
    -> Result<(), Box<dyn std::error::Error>> {
        use yi_types::config::RewriteLength;
        let theme = Theme::new(ColorTier::TrueColor, true);
        let mut app = App::new("/repo".to_owned(), theme);
        let session = crate::model::SessionId("s-improve".to_owned());
        let pane = app.state.focused_pane_id().ok_or("a pane")?;
        let mut chat = app.make_chat(&session);
        chat.app
            .apply(yi_tui::Reply::Rewriter(RewriteLength::Medium));
        if let Some(slot) = app.state.panes.get_mut(&pane) {
            slot.content = crate::model::PaneContent::Session {
                session: Some(session),
                chat: Some(chat),
            };
        }
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(100, 30))?;
        let mut paint = |app: &mut App| -> Result<String, Box<dyn std::error::Error>> {
            let mut out = Vec::new();
            let mut cache = std::collections::HashMap::new();
            present(app, &mut terminal, &theme, &mut cache, &mut out)?;
            Ok(painted(&terminal))
        };
        let before = paint(&mut app)?;
        assert!(before.contains("improve: off \u{b7} short \u{b7} [medium] \u{b7} long"));

        let hits = app.hits.as_ref().ok_or("no hits")?;
        let long = hits.controls.iter().find_map(|(rect, hit)| {
            matches!(
                hit,
                crate::render::Hit::Improve(_, Some(RewriteLength::Long))
            )
            .then_some(*rect)
        });
        let long = long.ok_or("the long position is not a control")?;
        let (events, _received) = crate::client::events();
        let socket = std::env::temp_dir().join("yi-improve-click-no-daemon.sock");
        let (outbound, _threads) = crate::client::spawn(socket, events);
        app.handle_mouse(&outbound, MouseKind::Down, long.x, long.y);
        outbound.shutdown();
        let after = paint(&mut app)?;
        assert!(
            after.contains("improve: off \u{b7} short \u{b7} medium \u{b7} [long]"),
            "{after}"
        );
        Ok(())
    }
}
