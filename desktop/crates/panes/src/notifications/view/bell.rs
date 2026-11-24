//! What the bell keeps: the attention list as rows that outlive the moment, and which are read.

use yd_core::ids::{Millis, ThreadId};
use yd_core::reduce::state::Wire;
use yd_core::reduce::threads::{Alert, Want};

/// Rows kept at most; the oldest read row goes first.
const KEPT: usize = 64;

/// One row of the panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub thread: ThreadId,
    pub want: Want,
    pub title: String,
    pub since: Millis,
    pub detail: String,
    pub read: bool,
}

impl Item {
    /// A key part that names the row for good: the thread and what it wants.
    pub fn id(&self) -> String {
        let want = match &self.want {
            Want::Approval(ask) => format!("approval:{ask}"),
            Want::Child(agent) => format!("child:{agent}"),
            Want::NeedsYou => "needs".to_owned(),
            Want::Failed => "failed".to_owned(),
            Want::Done => "done".to_owned(),
        };
        format!("{}:{want}", self.thread).replace('/', ":")
    }

    /// Whether the thread is waiting on the user, as against having finished or failed.
    pub fn needs_you(&self) -> bool {
        match self.want {
            Want::Approval(_) | Want::NeedsYou | Want::Child(_) => true,
            Want::Failed | Want::Done => false,
        }
    }

    fn rank(&self) -> u8 {
        match self.want {
            Want::Approval(_) => 0,
            Want::NeedsYou | Want::Child(_) => 1,
            Want::Failed => 2,
            Want::Done => 3,
        }
    }
}

/// The panel's own list. It is not a view of findings, and it is not the wire's list: it
/// remembers a finished thread after the user has seen it, as a read row.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Bell {
    pub open: bool,
    items: Vec<Item>,
}

fn title(wire: &Wire, thread: &ThreadId) -> String {
    let named = wire
        .threads
        .get(thread)
        .and_then(|thread| thread.title.clone());
    named.unwrap_or_else(|| thread.to_string())
}

/// What the row says under its title, in the server's own words.
fn detail(wire: &Wire, alert: &Alert) -> String {
    let words = || wire.threads.get(&alert.thread)?.transcript.last_words();
    let asks = |note: Option<String>| match note.filter(|note| !note.is_empty()) {
        Some(note) => format!("Asks: {note}"),
        None => "Waiting for you".to_owned(),
    };
    match &alert.want {
        Want::Approval(_) => alert.detail.clone(),
        Want::NeedsYou => asks(words()),
        Want::Child(_) => asks(Some(alert.detail.clone())),
        Want::Failed => words().unwrap_or_else(|| "Failed".to_owned()),
        Want::Done => words().unwrap_or_else(|| "Done".to_owned()),
    }
}

impl Bell {
    /// Folds the wire's attention list in: a new alert is a new unread row, and a row whose
    /// alert is gone leaves if it waited on the user, else is read. Returns the news.
    pub fn sync(&mut self, wire: &Wire) -> Vec<Item> {
        let mut news = Vec::new();
        let alerts = wire.attention().alerts;
        let live = |item: &Item| {
            let same = |alert: &&Alert| alert.thread == item.thread && alert.want == item.want;
            alerts.iter().any(|alert| same(&alert))
        };
        self.items.retain_mut(|item| {
            if live(item) {
                return true;
            }
            item.read = true;
            !item.needs_you()
        });
        for alert in &alerts {
            let held = self
                .items
                .iter_mut()
                .find(|item| item.thread == alert.thread && item.want == alert.want);
            let (title, detail) = (title(wire, &alert.thread), detail(wire, alert));
            match held {
                Some(item) => {
                    // Invariant: a thread that finishes again is news again.
                    let again = item.read && alert.since > item.since;
                    item.read = item.read && !again;
                    (item.title, item.since, item.detail) = (title, alert.since, detail);
                    if again {
                        news.push(item.clone());
                    }
                }
                None => {
                    let item = Item {
                        thread: alert.thread.clone(),
                        want: alert.want.clone(),
                        title,
                        since: alert.since,
                        detail,
                        read: false,
                    };
                    news.push(item.clone());
                    self.items.push(item);
                }
            }
        }
        while self.items.len() > KEPT {
            let oldest = self.items.iter().position(|item| item.read).unwrap_or(0);
            self.items.remove(oldest);
        }
        news
    }

    pub fn mark_all_read(&mut self) {
        self.items.iter_mut().for_each(|item| item.read = true);
    }

    pub fn unread(&self) -> usize {
        self.news().count()
    }

    /// The thread of each row the user has not read.
    pub fn news(&self) -> impl Iterator<Item = &ThreadId> {
        let news = self.items.iter().filter(|item| !item.read);
        news.map(|item| &item.thread)
    }

    /// The rows as drawn: unread before read, then what waits on the user (approvals first),
    /// then what failed, then what finished; newest first inside each.
    pub fn shown(&self) -> Vec<&Item> {
        let mut rows: Vec<&Item> = self.items.iter().collect();
        rows.sort_by_key(|item| (item.read, item.rank(), std::cmp::Reverse(item.since)));
        rows
    }
}
