use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, Sender};

use ratatui::layout::Direction;
use yi_tui::app::{CommandReceiver, CommandSender};
use yi_tui::{AskChoice, UiEvent};
use yi_types::acp::{AcpPermissionOption, AcpPermissionParams, AcpState};

use crate::app::port::RemotePort;
use crate::layout::{PaneId, PaneIds, TileLayout};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(pub String);

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

    pub fn need(self) -> usize {
        match self {
            Self::Blocked => 0,
            Self::DoneUnseen => 1,
            Self::Working => 2,
            Self::Idle => 3,
            Self::Unknown => 4,
        }
    }

    pub fn section(self) -> &'static str {
        match self {
            Self::Blocked | Self::DoneUnseen => "needs you",
            Self::Working => "working",
            Self::Idle | Self::Unknown => "idle",
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

#[derive(Debug, Clone)]
pub struct SessionRow {
    pub id: SessionId,
    pub root: String,
    pub status: SessionStatus,
    pub attached: bool,
    pub name: Option<String>,
    pub created_ms: u64,
    pub last_ms: u64,
}

impl SessionRow {
    /// A session is named by its first prompt; until then it is what it is.
    pub fn label(&self) -> String {
        self.name.clone().unwrap_or_else(|| "untitled".to_owned())
    }

    pub fn seed(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id.0)
    }

    pub fn recency(&self) -> u64 {
        self.last_ms.max(self.created_ms)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarMode {
    Rail,
    Full,
}

impl SidebarMode {
    pub fn next(self) -> Self {
        match self {
            Self::Rail => Self::Full,
            Self::Full => Self::Rail,
        }
    }
}

#[expect(
    clippy::disallowed_methods,
    reason = "row ages and a fresh session's birth are read against the wall clock"
)]
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    Sidebar,
    Panes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    Connecting,
    Connected,
    Disconnected { reason: String },
}

/// Solo's chat bound to one pane: the same `App`, fed over the daemon link.
pub struct Chat {
    pub app: yi_tui::app::App,
    pub port: RemotePort,
    pub orb: yi_tui::orb::Tick,
    pub phase: usize,
    pub logos: yi_tui::logos::Tick,
    pub ask: Option<PendingAsk>,
    pub events: (Sender<UiEvent>, Receiver<UiEvent>),
    pub commands: (CommandSender, CommandReceiver),
}

/// A permission the worker asked, answered by the chat's own popup.
pub struct PendingAsk {
    /// Wire id of the worker-originated request, echoed back in the response.
    pub request_id: String,
    pub reply: Receiver<AskChoice>,
    pub options: Vec<AcpPermissionOption>,
}

#[derive(Debug, Clone, Default)]
pub struct NbCell {
    pub call_id: String,
    pub code: String,
    pub stdout: String,
    pub result: Option<String>,
    pub error: Option<String>,
    pub images: Vec<String>,
    pub running: bool,
}

/// What a pane shows: the provider seam — a new kind is a new variant plus
/// its render arm, nothing else.
pub enum PaneContent {
    Session {
        session: Option<SessionId>,
        chat: Option<Box<Chat>>,
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
        input: Box<tui_textarea::TextArea<'static>>,
    },
    SessionDiff {
        session: SessionId,
        scope: ReviewScope,
    },
    Tape {
        session: SessionId,
        tape: Option<yi_types::tape::Tape>,
        cursor: usize,
    },
    Editor(Editor),
}

pub struct Editor {
    pub path: String,
    pub text: Box<tui_textarea::TextArea<'static>>,
    pub dirty: bool,
    pub mtime: Option<std::time::SystemTime>,
    pub stale: bool,
    pub scroll_top: usize,
    pub lang: Option<String>,
    pub primed: Option<(usize, yi_tui::highlight::Lang)>,
    pub last_cursor: (usize, usize),
}

impl Editor {
    pub fn primed_lang(&mut self) -> Option<yi_tui::highlight::Lang> {
        let fresh = || self.lang.as_deref().and_then(yi_tui::highlight::lang_for);
        let (from, mut lang) = match self.primed.take() {
            Some((at, lang)) if at <= self.scroll_top => (at, lang),
            _ => (0, fresh()?),
        };
        for line in self
            .text
            .lines()
            .get(from..self.scroll_top)
            .unwrap_or_default()
        {
            yi_tui::highlight::tokens(line, &mut lang);
        }
        self.primed = Some((self.scroll_top, lang.clone()));
        Some(lang)
    }

    pub fn gutter(&self) -> usize {
        self.text
            .lines()
            .len()
            .max(1)
            .to_string()
            .len()
            .saturating_add(1)
    }
}

pub struct FileDiff {
    pub patch: String,
    pub added: u64,
    pub removed: u64,
    pub tracked: bool,
    pub serving: Option<String>,
    pub turn: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReviewScope {
    #[default]
    Branch,
    Turn,
    Session,
}

impl ReviewScope {
    pub fn next(self) -> Self {
        match self {
            Self::Branch => Self::Turn,
            Self::Turn => Self::Session,
            Self::Session => Self::Branch,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Branch => "branch",
            Self::Turn => "turn",
            Self::Session => "session",
        }
    }
}

#[derive(Default)]
pub struct SessionDiff {
    pub files: Vec<(String, FileDiff)>,
    pub turn: u64,
    pub reads: BTreeMap<String, u32>,
    pub branch: Option<yi_types::lane::BranchDiff>,
    pub branch_due: bool,
    pub tape_due: bool,
    /// The title a second `l` lands under; any other key clears it.
    pub land_armed: Option<String>,
}

const MAX_DIFF_FILES: usize = 512;
const MAX_DIFF_BYTES: usize = 4 * 1024 * 1024;

impl SessionDiff {
    pub fn insert(&mut self, path: String, file: FileDiff) {
        self.files.retain(|(existing, _)| *existing != path);
        self.files.push((path, file));
        while self.files.len() > MAX_DIFF_FILES || self.bytes() > MAX_DIFF_BYTES {
            if self.files.len() <= 1 {
                break;
            }
            self.files.remove(0);
        }
    }

    pub fn file_mut(&mut self, path: &str) -> Option<&mut FileDiff> {
        self.files
            .iter_mut()
            .find(|(existing, _)| existing == path)
            .map(|(_, file)| file)
    }

    fn bytes(&self) -> usize {
        self.files.iter().map(|(_, file)| file.patch.len()).sum()
    }

    pub fn totals(&self) -> (u64, u64) {
        self.files.iter().fold((0, 0), |(a, r), (_, file)| {
            (a.saturating_add(file.added), r.saturating_add(file.removed))
        })
    }
}

pub fn notebook_input() -> Box<tui_textarea::TextArea<'static>> {
    let mut input = tui_textarea::TextArea::default();
    input.set_placeholder_text("python · ⇧↩ runs on this session's kernel · esc cancels");
    Box::new(input)
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
                chat: None,
            },
            scroll_from_bottom: 0,
        }
    }

    pub fn session(&self) -> Option<&SessionId> {
        match &self.content {
            PaneContent::Session { session, .. } | PaneContent::Notebook { session, .. } => {
                session.as_ref()
            }
            PaneContent::Markdown { .. }
            | PaneContent::Diff { .. }
            | PaneContent::SessionDiff { .. }
            | PaneContent::Tape { .. }
            | PaneContent::Editor(_) => None,
        }
    }
}

pub struct Tab {
    pub layout: TileLayout,
    pub zoomed: bool,
}

pub enum Mode {
    Normal,
    Prefix,
    Navigator { query: String, selected: usize },
    Keys,
}

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
    pub banner: Option<String>,
    pub orphan_asks: Vec<(String, AcpPermissionParams)>,
    pub dropped_frames: u64,
    pub quit: bool,
    pub sidebar: SidebarMode,
    pub sidebar_cols: usize,
    pub cursor_moved: bool,
    pub diffs: BTreeMap<SessionId, SessionDiff>,
    pub side_opened: std::collections::BTreeSet<SessionId>,
    pub auto_side: bool,
    pub root_filter: Option<String>,
    pub children: BTreeMap<SessionId, Vec<yi_types::subagent::ChildUpdate>>,
    pub parked: BTreeMap<SessionId, Box<Chat>>,
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
            banner: None,
            orphan_asks: Vec::new(),
            dropped_frames: 0,
            sidebar: SidebarMode::Rail,
            sidebar_cols: 0,
            cursor_moved: false,
            diffs: BTreeMap::new(),
            side_opened: std::collections::BTreeSet::new(),
            auto_side: true,
            root_filter: None,
            children: BTreeMap::new(),
            parked: BTreeMap::new(),
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

    /// Every chat showing the session, across every tab.
    pub fn chat(&self, id: &SessionId) -> Option<&Chat> {
        self.panes
            .values()
            .find_map(|pane| match &pane.content {
                PaneContent::Session {
                    session: Some(bound),
                    chat: Some(chat),
                } if bound == id => Some(chat.as_ref()),
                _ => None,
            })
            .or_else(|| self.parked.get(id).map(Box::as_ref))
    }

    pub fn chats_mut(&mut self, id: &SessionId) -> Vec<&mut Chat> {
        let parked = self.parked.get_mut(id).map(Box::as_mut);
        self.panes
            .values_mut()
            .filter_map(|pane| match &mut pane.content {
                PaneContent::Session {
                    session: Some(bound),
                    chat: Some(chat),
                } if bound == id => Some(chat.as_mut()),
                _ => None,
            })
            .chain(parked)
            .collect()
    }

    /// ponytail: evicts the lowest session id, not the least recent; order it if 8 is tight.
    pub fn park(&mut self, id: SessionId, chat: Box<Chat>) {
        if !self.parked.contains_key(&id) && self.parked.len() >= 8 {
            self.parked.pop_first();
        }
        self.parked.insert(id, chat);
    }

    pub fn selected_id(&self) -> Option<&SessionId> {
        self.order.get(self.selected)
    }

    pub fn upsert_row(&mut self, row: SessionRow) {
        match self.sessions.get_mut(&row.id) {
            Some(existing) => {
                existing.root = row.root;
                // Invariant: done stays loud until a focus clears it; no poll can.
                if !(existing.status == SessionStatus::DoneUnseen
                    && row.status == SessionStatus::Idle)
                {
                    existing.status = row.status;
                }
                existing.attached = row.attached;
                existing.name = row.name.or(existing.name.take());
                existing.created_ms = existing.created_ms.max(row.created_ms);
                existing.last_ms = existing.last_ms.max(row.last_ms);
            }
            None => {
                self.order.push(row.id.clone());
                self.sessions.insert(row.id.clone(), row);
            }
        }
        self.settle_cursor();
    }

    pub fn settle_cursor(&mut self) {
        if !self.cursor_moved
            && let Some(first) = self.visible_rows().first()
        {
            self.selected = *first;
        } else if self.selected >= self.order.len() {
            self.selected = self.order.len().saturating_sub(1);
        }
    }

    pub fn set_session_status(&mut self, id: &SessionId, status: SessionStatus) {
        if let Some(row) = self.sessions.get_mut(id) {
            row.status = status;
        }
    }

    pub fn visible_rows(&self) -> Vec<usize> {
        let mut rows: Vec<usize> = self
            .order
            .iter()
            .enumerate()
            .filter(|(_, id)| {
                self.root_filter
                    .as_deref()
                    .is_none_or(|root| self.sessions.get(*id).is_some_and(|row| row.root == root))
            })
            .map(|(index, _)| index)
            .collect();
        rows.sort_by_key(|index| {
            let row = self.order.get(*index).and_then(|id| self.sessions.get(id));
            (
                row.map_or(usize::MAX, |row| row.status.need()),
                std::cmp::Reverse(row.map_or(0, SessionRow::recency)),
            )
        });
        rows
    }

    pub fn set_root_filter(&mut self, root: Option<String>) {
        self.root_filter = root;
        if let Some(first) = self.visible_rows().first() {
            self.selected = *first;
        }
    }

    pub fn cycle_root_filter(&mut self, forward: bool) {
        let roots = self.roots();
        let at = self
            .root_filter
            .as_ref()
            .and_then(|root| roots.iter().position(|r| r == root));
        let next = match (at, forward) {
            (None, true) => roots.first().cloned(),
            (None, false) => roots.last().cloned(),
            (Some(i), true) => roots.get(i.saturating_add(1)).cloned(),
            (Some(0), false) => None,
            (Some(i), false) => roots.get(i.saturating_sub(1)).cloned(),
        };
        self.set_root_filter(next);
    }

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
