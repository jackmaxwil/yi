//! Desktop notifications over terminal escapes (OSC 9, or kitty's OSC 99), plus the delayed
//! re-validated queue that keeps agent status flicker from reaching the user.

use std::time::{Duration, Instant};

use crate::model::{SessionId, SessionStatus};

/// How long a status change must hold before it notifies.
pub const NOTIFY_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OscFlavor {
    /// OSC 9 — iTerm2, WezTerm, Ghostty and most others.
    Osc9,
    /// Kitty's structured OSC 99.
    Kitty,
    /// Terminal gave no signal it renders notifications.
    None,
}

pub fn detect_flavor(term_program: Option<&str>, kitty_id: Option<&str>) -> OscFlavor {
    if kitty_id.is_some() {
        return OscFlavor::Kitty;
    }
    match term_program {
        Some("iTerm.app" | "WezTerm" | "ghostty") => OscFlavor::Osc9,
        _ => OscFlavor::None,
    }
}

/// Control bytes never reach the escape payload.
fn sanitize(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(200).collect()
}

/// The escape string for one notification, or None when the terminal has no
/// notification support.
pub fn escape(flavor: OscFlavor, title: &str, body: &str) -> Option<String> {
    let title = sanitize(title);
    let body = sanitize(body);
    match flavor {
        OscFlavor::Osc9 => Some(format!("\u{1b}]9;{title}: {body}\u{7}")),
        OscFlavor::Kitty => Some(format!(
            "\u{1b}]99;i=yi:d=0;{title}\u{7}\u{1b}]99;i=yi:d=1:p=body;{body}\u{7}"
        )),
        OscFlavor::None => None,
    }
}

/// A status change waiting out its delay before it may notify.
pub struct PendingNote {
    pub session: SessionId,
    pub status: SessionStatus,
    pub due: Instant,
}

/// Queue with one slot per session: a newer transition replaces the old one,
/// so flicker re-arms the delay instead of stacking toasts.
#[derive(Default)]
pub struct NoteQueue {
    pending: Vec<PendingNote>,
}

impl NoteQueue {
    pub fn arm(&mut self, session: &SessionId, status: SessionStatus, now: Instant) {
        self.pending.retain(|note| note.session != *session);
        if matches!(status, SessionStatus::Blocked | SessionStatus::DoneUnseen) {
            self.pending.push(PendingNote {
                session: session.clone(),
                status,
                due: now + NOTIFY_DELAY,
            });
        }
    }

    pub fn disarm(&mut self, session: &SessionId) {
        self.pending.retain(|note| note.session != *session);
    }

    /// Notes whose delay elapsed; the caller re-validates against live state
    /// before emitting anything.
    pub fn due(&mut self, now: Instant) -> Vec<PendingNote> {
        let mut fired = Vec::new();
        let mut index = 0;
        while index < self.pending.len() {
            if self.pending.get(index).is_some_and(|note| note.due <= now) {
                fired.push(self.pending.remove(index));
            } else {
                index = index.saturating_add(1);
            }
        }
        fired
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flicker_rearms_instead_of_stacking() {
        let mut queue = NoteQueue::default();
        let session = SessionId("s".to_owned());
        let start = Instant::now();
        queue.arm(&session, SessionStatus::DoneUnseen, start);
        queue.arm(
            &session,
            SessionStatus::Blocked,
            start + Duration::from_millis(500),
        );
        assert!(queue.due(start + NOTIFY_DELAY).is_empty());
        let fired = queue.due(start + Duration::from_millis(500) + NOTIFY_DELAY);
        assert_eq!(fired.len(), 1);
        assert!(
            fired
                .iter()
                .all(|note| note.status == SessionStatus::Blocked)
        );
        assert!(queue.is_empty());
    }

    #[test]
    fn working_transition_disarms() {
        let mut queue = NoteQueue::default();
        let session = SessionId("s".to_owned());
        let start = Instant::now();
        queue.arm(&session, SessionStatus::Blocked, start);
        queue.arm(&session, SessionStatus::Working, start);
        assert!(queue.is_empty());
    }

    #[test]
    fn escapes_are_sanitized() {
        let escaped = escape(OscFlavor::Osc9, "ti\u{7}tle", "bo\u{1b}dy");
        assert_eq!(escaped.as_deref(), Some("\u{1b}]9;title: body\u{7}"));
        assert!(escape(OscFlavor::None, "t", "b").is_none());
    }
}
