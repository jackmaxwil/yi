//! What a terminal pane keeps: its shells, each with the screen its source fills.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use yd_core::feed::TERMINAL;
use yd_core::ids::FeedId;

use crate::term::screen::{Cell, Rows, Selection};

/// The feed every shell is an instance of; `yd_sources::pty` fills it.
pub const FEED: FeedId = TERMINAL;

/// Instance numbers are app-wide: two terminal panes never share a shell.
static NEXT: AtomicU32 = AtomicU32::new(1);

/// One shell's screen as this app draws it. The app hands `parser` and `lines` to the source,
/// whose reader thread is their only writer apart from a resize.
pub struct Surface {
    pub parser: Arc<Mutex<vt100::Parser>>,
    /// Lines that scrolled off, as the source formats them; it caps them.
    pub lines: Arc<Mutex<VecDeque<Vec<u8>>>>,
    /// Lines scrolled back from the live screen.
    pub back: usize,
    pub(super) wheel: f32,
    pub(super) anchor: Option<Cell>,
    pub selection: Option<Selection>,
    /// The size last sent to the source; none until the first layout.
    pub(super) told: Option<(u16, u16)>,
    pub(super) rows: RefCell<Rows>,
}

impl Surface {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 2 * 64))),
            lines: Arc::new(Mutex::new(VecDeque::new())),
            back: 0,
            wheel: 0.0,
            anchor: None,
            selection: None,
            told: None,
            rows: RefCell::new(Rows::default()),
        }
    }
}

/// A shell tab: its feed instance, the program, the folder it runs in and its screen.
pub struct Shell {
    pub feed: FeedId,
    /// The shell's path; the tab shows its file name.
    pub program: String,
    /// The working folder as shown; empty means the thread's own.
    pub cwd: String,
    pub surface: Surface,
}

impl Shell {
    /// A new shell tab running the user's login shell in `cwd`.
    pub fn new(cwd: String) -> Self {
        let program = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_owned());
        Self {
            feed: FEED.instance(NEXT.fetch_add(1, Ordering::Relaxed)),
            program,
            cwd,
            surface: Surface::new(24, 80),
        }
    }

    pub fn name(&self) -> &str {
        self.program.rsplit('/').next().unwrap_or(&self.program)
    }
}

/// Which tab the pane shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Shell(FeedId),
    /// The read-only list of the commands the agent ran.
    Agent,
}

/// The pane's state: its shell tabs and the one shown.
pub struct Shells {
    pub tabs: Vec<Shell>,
    pub active: Tab,
}

impl std::fmt::Debug for Shells {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Shells({} tabs)", self.tabs.len())
    }
}

impl Default for Shells {
    fn default() -> Self {
        let first = Shell::new(String::new());
        Self {
            active: Tab::Shell(first.feed),
            tabs: vec![first],
        }
    }
}

impl Shells {
    pub fn shell(&self, feed: FeedId) -> Option<&Shell> {
        self.tabs.iter().find(|shell| shell.feed == feed)
    }

    pub fn shell_mut(&mut self, feed: FeedId) -> Option<&mut Shell> {
        self.tabs.iter_mut().find(|shell| shell.feed == feed)
    }

    /// Opens a shell in the shown shell's folder and shows it.
    pub fn open(&mut self) -> FeedId {
        let cwd = match self.active {
            Tab::Shell(feed) => self.shell(feed).map(|shell| shell.cwd.clone()),
            Tab::Agent => None,
        };
        let shell = Shell::new(cwd.unwrap_or_default());
        let feed = shell.feed;
        self.tabs.push(shell);
        self.active = Tab::Shell(feed);
        feed
    }

    /// Drops a shell tab and shows its neighbour; the last shell cannot close.
    pub fn close(&mut self, feed: FeedId) -> bool {
        let Some(at) = self.tabs.iter().position(|shell| shell.feed == feed) else {
            return false;
        };
        if self.tabs.len() < 2 {
            return false;
        }
        self.tabs.remove(at);
        if self.active == Tab::Shell(feed) {
            let next = self.tabs.get(at.saturating_sub(1)).map(|shell| shell.feed);
            self.active = next.map_or(Tab::Agent, Tab::Shell);
        }
        true
    }
}

pub(super) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
