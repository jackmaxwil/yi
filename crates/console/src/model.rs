use std::collections::BTreeMap;

use ratatui::layout::Direction;
use yi_types::acp::{AcpPermissionParams, AcpState};

use crate::layout::{PaneId, PaneIds, TileLayout};
use crate::transcript::Transcript;

/// Session identity as the wire carries it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(pub String);

/// Outgoing JSON-RPC request id (rewritten by the daemon in flight).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestId(pub u64);

/// Invariant: status crosses agent state with the seen bit — "done,
/// unlooked-at" must render louder than idle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Blocked,
    Working,
    DoneUnseen,
    Idle,
    Unknown,
}

impl SessionStatus {
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Blocked => "✕",
            Self::Working => "◐",
            Self::DoneUnseen => "●",
            Self::Idle => "○",
            Self::Unknown => "·",
        }
    }

    pub fn from_state(state: &AcpState, seen: bool) -> Self {
        match state {
            AcpState::Running => Self::Working,
            AcpState::RequiresAction => Self::Blocked,
            AcpState::Idle { .. } => {
                if seen {
                    Self::Idle
                } else {
                    Self::DoneUnseen
                }
            }
        }
    }

    /// Unrecognized ledger strings are Unknown, never an error.
    pub fn from_ledger(state: &str, unseen: u64) -> Self {
        match state {
            "running" => Self::Working,
            "requires_action" => Self::Blocked,
            "idle" => {
                if unseen > 0 {
                    Self::DoneUnseen
                } else {
                    Self::Idle
                }
            }
            _ => Self::Unknown,
        }
    }
}

/// One sidebar row, merged from the worker and daemon lists.
#[derive(Debug, Clone)]
pub struct SessionRow {
    pub id: SessionId,
    pub root: String,
    pub status: SessionStatus,
    pub attached: bool,
}

/// Which zone owns plain keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    Sidebar,
    Panes,
}

/// Connection state as the status line reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    Connecting,
    Connected,
    Disconnected { reason: String },
}

/// A pending `session/request_permission`, shown until answered.
#[derive(Debug, Clone)]
pub struct ActiveAsk {
    /// Wire id of the worker-originated request, echoed back in the response.
    pub request_id: String,
    pub params: AcpPermissionParams,
}

/// One executed kernel cell as the notebook pane retains it.
#[derive(Debug, Clone, Default)]
pub struct NbCell {
    pub call_id: String,
    pub code: String,
    pub stdout: String,
    pub result: Option<String>,
    pub error: Option<String>,
    /// base64 PNG payloads, ready for a kitty `f=100` transmit.
    pub images: Vec<String>,
    pub running: bool,
}

/// What a pane shows: the provider seam — a new kind is a new variant plus
/// its render arm, nothing else.
pub enum PaneContent {
    Session {
        session: Option<SessionId>,
        transcript: Transcript,
    },
    Markdown {
        path: String,
        source: String,
    },
    Diff {
        path: String,
        source: String,
    },
    Notebook {
        session: Option<SessionId>,
        cells: Vec<NbCell>,
    },
}

pub struct Pane {
    pub content: PaneContent,
    pub scroll_from_bottom: usize,
}

impl Pane {
    pub fn empty_session() -> Self {
        Self {
            content: PaneContent::Session {
                session: None,
                transcript: Transcript::new(),
            },
            scroll_from_bottom: 0,
        }
    }

    pub fn session(&self) -> Option<&SessionId> {
        match &self.content {
            PaneContent::Session { session, .. } | PaneContent::Notebook { session, .. } => {
                session.as_ref()
            }
            PaneContent::Markdown { .. } | PaneContent::Diff { .. } => None,
        }
    }
}

pub struct Tab {
    pub layout: TileLayout,
    pub zoomed: bool,
}

/// Modal input state; one overlay at a time.
pub enum Mode {
    Normal,
    /// ctrl+b armed, next key resolves from the prefix table.
    Prefix,
    Navigator {
        query: String,
        selected: usize,
    },
}

/// Everything the render pass reads; `order` fixes the sidebar sequence.
pub struct ConsoleState {
    pub root: String,
    pub link: Link,
    pub sessions: BTreeMap<SessionId, SessionRow>,
    pub order: Vec<SessionId>,
    pub selected: usize,
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    pub panes: BTreeMap<PaneId, Pane>,
    pub pane_ids: PaneIds,
    pub zone: Zone,
    pub mode: Mode,
    pub ask: Option<ActiveAsk>,
    pub status_note: Option<String>,
    pub dropped_frames: u64,
    pub tokens_used: Option<(u64, u64)>,
    pub quit: bool,
}

impl ConsoleState {
    pub fn new(root: String) -> Self {
        let mut pane_ids = PaneIds::default();
        let (layout, first) = TileLayout::new(&mut pane_ids);
        let mut panes = BTreeMap::new();
        panes.insert(first, Pane::empty_session());
        Self {
            root,
            link: Link::Connecting,
            sessions: BTreeMap::new(),
            order: Vec::new(),
            selected: 0,
            tabs: vec![Tab {
                layout,
                zoomed: false,
            }],
            active_tab: 0,
            panes,
            pane_ids,
            zone: Zone::Sidebar,
            mode: Mode::Normal,
            ask: None,
            status_note: None,
            dropped_frames: 0,
            tokens_used: None,
            quit: false,
        }
    }

    pub fn tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active_tab)
    }

    pub fn tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active_tab)
    }

    pub fn focused_pane_id(&self) -> Option<PaneId> {
        self.tab().map(|tab| tab.layout.focused())
    }

    pub fn focused_pane(&self) -> Option<&Pane> {
        self.panes.get(&self.focused_pane_id()?)
    }

    pub fn focused_pane_mut(&mut self) -> Option<&mut Pane> {
        let id = self.focused_pane_id()?;
        self.panes.get_mut(&id)
    }

    pub fn focused_session(&self) -> Option<SessionId> {
        self.focused_pane()?.session().cloned()
    }

    /// True when any pane in any tab shows the session (attachment must
    /// survive that pane losing focus).
    pub fn session_visible(&self, id: &SessionId) -> bool {
        self.panes.values().any(|pane| pane.session() == Some(id))
    }

    pub fn selected_id(&self) -> Option<&SessionId> {
        self.order.get(self.selected)
    }

    /// Insert or update a row, keeping first-seen order stable.
    pub fn upsert_row(&mut self, row: SessionRow) {
        if !self.sessions.contains_key(&row.id) {
            self.order.push(row.id.clone());
        }
        self.sessions.insert(row.id.clone(), row);
        if self.selected >= self.order.len() {
            self.selected = self.order.len().saturating_sub(1);
        }
    }

    pub fn set_session_status(&mut self, id: &SessionId, status: SessionStatus) {
        if let Some(row) = self.sessions.get_mut(id) {
            row.status = status;
        }
    }

    /// Roots with at least one known session, launch root first.
    pub fn roots(&self) -> Vec<String> {
        let mut roots = vec![self.root.clone()];
        for id in &self.order {
            if let Some(row) = self.sessions.get(id)
                && !roots.contains(&row.root)
            {
                roots.push(row.root.clone());
            }
        }
        roots
    }

    pub fn split_focused(&mut self, direction: Direction) -> Option<PaneId> {
        let ids = &mut self.pane_ids;
        let tab = self.tabs.get_mut(self.active_tab)?;
        let new_id = tab.layout.split_focused(ids, direction);
        tab.zoomed = false;
        self.panes.insert(new_id, Pane::empty_session());
        Some(new_id)
    }

    pub fn close_focused_pane(&mut self) -> Option<PaneId> {
        let tab = self.tabs.get_mut(self.active_tab)?;
        let closed = tab.layout.close_focused()?;
        tab.zoomed = false;
        self.panes.remove(&closed);
        Some(closed)
    }

    pub fn new_tab(&mut self) {
        let (layout, first) = TileLayout::new(&mut self.pane_ids);
        self.panes.insert(first, Pane::empty_session());
        self.tabs.push(Tab {
            layout,
            zoomed: false,
        });
        self.active_tab = self.tabs.len().saturating_sub(1);
    }

    pub fn select_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active_tab = index;
        }
    }
}
