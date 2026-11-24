//! A feed is a typed stream from a source: reset, upsert, remove, status. Its rows fold into
//! a table that every list and every empty, loading, offline or error state reads.

use std::collections::{BTreeMap, HashMap};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use crate::command::table::Run;
use crate::ids::{FeedId, RowId};
use crate::inbox::{Closed, FeedTx};
use crate::row::{Row, Rows, Value};

/// The working folder's tree and the text of files read from it; the `fs` source fills it.
pub const FILES: FeedId = FeedId::new("files");
/// The working folder's branch and uncommitted changes; the `git` source fills it.
pub const CHANGES: FeedId = FeedId::new("changes");
/// A shell on a PTY; each terminal tab is one instance.
pub const TERMINAL: FeedId = FeedId::new("terminal");
/// The code viewer's open files, one row each; `code.open` fills it, no source does.
pub const OPEN_FILES: FeedId = FeedId::new("code");

/// Language servers, their problems and answers; the `lsp` source fills it.
pub const LSP: FeedId = FeedId::new("lsp");
/// One debugging session; the `dap` source fills it.
pub const DEBUGGER: FeedId = FeedId::new("debugger");
/// Containers on this Mac; the `docker` source fills it.
pub const CONTAINERS: FeedId = FeedId::new("containers");
/// Services on this Mac; the `launchd` source fills it.
pub const DAEMONS: FeedId = FeedId::new("daemons");
/// Pages of the built-in browser; the `browser` source fills it.
pub const WEB: FeedId = FeedId::new("web");
/// The page the Report pane shows: the `browser` source's second page.
pub const REPORT: FeedId = WEB.instance(1);

/// The browser source's page row for `feed`.
pub fn page_row_id(feed: FeedId) -> RowId {
    RowId::new(format!("page:{}", feed.instance))
}
/// Answers to a script pane's web requests; the `fetch` source fills it.
pub const FETCH: FeedId = FeedId::new("fetch");
/// Virtual machines; the `machines` source fills it.
pub const VMS: FeedId = FeedId::new("vms");
/// Hosts and other assets; the `machines` source fills it.
pub const ASSETS: FeedId = FeedId::new("assets");
/// The screen of one machine, its windows and rows; the `machines` source fills it.
pub const REMOTE: FeedId = FeedId::new("remote");

/// Rows one table keeps; past it the oldest go, counted in [`Table::dropped`].
pub const MAX_ROWS: usize = 200_000;

/// Where a feed stands. A feed that goes offline or fails keeps the rows it had.
#[derive(Debug, Clone, PartialEq, Hash, Default)]
pub enum FeedStatus {
    #[default]
    Loading,
    Live,
    Empty,
    Offline,
    Error(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum FeedEvent {
    Reset(Vec<Row>),
    Upsert(Row),
    Remove(RowId),
    Status(FeedStatus),
}

/// A key a screen passes on that types nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenKey {
    Enter,
    Backspace,
    Delete,
    Tab,
    Escape,
    Left,
    Right,
    Up,
    Down,
}

/// What the pointer did on a screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pointer {
    Press,
    Move,
    Release,
}

/// Input on a screen surface; every point is in the picture's own pixels from its top left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenInput {
    Pointer {
        phase: Pointer,
        x: u32,
        y: u32,
    },
    /// Points to scroll by where the pointer last was, positive moving the content right and
    /// down.
    Wheel {
        dx: i32,
        dy: i32,
    },
    Key(ScreenKey),
    Text(String),
}

/// What the app asks of a running source.
#[derive(Debug, Clone, PartialEq)]
pub enum SourceCmd {
    /// A command aimed at something this source owns.
    Run(Run),
    /// Bytes for the far side: terminal keys, a paste.
    Input(Vec<u8>),
    /// The surface's new size, in cells for a terminal and points for a screen.
    Resize {
        cols: u16,
        rows: u16,
    },
    Refresh,
    /// The far side is done with: a shell tab closed, so its shell is hung up.
    Close,
    /// The folder to read from now on, absolute: the shown thread's working folder.
    Root(PathBuf),
    /// Read the file at this path under the root into the row `file:<path>`.
    Read(PathBuf),
    /// Put one hunk of `file` back as it was. `hunk` is its lines as shown, so a hunk that
    /// changed since is refused and no other is reverted.
    Revert {
        file: PathBuf,
        hunk: String,
    },
    /// Pointer, wheel, keys and typed text on the screen this source draws.
    Screen(ScreenInput),
    /// Whether some window shows a pane of this feed: a source reads on its timer only while
    /// one does.
    Seen(bool),
    /// The extra names this source lists, replacing those it had: the `daemons.labels` setting.
    Labels(Vec<String>),
    /// A second folder files may be read from, never written to: the app's own panes folder.
    Readable(PathBuf),
}

/// A source's thread body: it sends rows until `cmds` closes, or fails when the app is gone.
pub type SourceFn = fn(FeedTx, Receiver<SourceCmd>) -> Result<(), Closed>;

/// One row of the feed registry: a feed and the function that runs its source.
#[derive(Debug, Clone, Copy)]
pub struct FeedDef {
    pub id: FeedId,
    pub run: SourceFn,
}

/// The rows of one feed, in arrival order, found by id.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Table {
    status: FeedStatus,
    rows: Vec<Row>,
    index: HashMap<RowId, usize>,
    version: u64,
    keys: u64,
    dropped: u64,
}

impl Table {
    pub fn status(&self) -> &FeedStatus {
        &self.status
    }

    /// Rows this table let go of to stay under [`MAX_ROWS`].
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    pub fn find(&self, id: &RowId) -> Option<&Row> {
        self.rows.get(*self.index.get(id)?)
    }

    /// The rows whose id starts with `prefix`, as their own table with this one's status.
    // ponytail: copies the matching rows on each call; index by prefix if a feed grows large.
    pub fn prefixed(&self, prefix: &str) -> Table {
        let mut part = Table::default();
        part.apply(FeedEvent::Reset(
            self.rows
                .iter()
                .filter(|row| row.id.as_str().starts_with(prefix))
                .cloned()
                .collect(),
        ));
        part.status = self.status.clone();
        part
    }

    /// The rows `keep` gives back, as their own table with this one's status: a pane's view of
    /// a feed that carries more than it lists. Copies; for small feeds.
    pub fn kept(&self, keep: impl FnMut(&Row) -> Option<Row>) -> Table {
        let mut part = Table::default();
        part.apply(FeedEvent::Reset(
            self.rows.iter().filter_map(keep).collect(),
        ));
        part.status = self.status.clone();
        part
    }

    /// Moves when rows are added, removed or reordered, or a row changes its parent; an
    /// update in place keeps it. A list passes it to `virtual_rows_version`.
    pub fn keys_version(&self) -> u64 {
        self.keys
    }

    /// Folds one event in. Both versions are digests of the events, so a table a view builds
    /// afresh from the same rows each run keeps its versions; any change moves `version`.
    pub fn apply(&mut self, event: FeedEvent) {
        let mut content = DefaultHasher::new();
        let mut keys = DefaultHasher::new();
        (self.version, self.keys).hash(&mut content);
        self.keys.hash(&mut keys);
        let mut moved = false;
        match event {
            FeedEvent::Reset(rows) => {
                for row in &rows {
                    digest(row, &mut content);
                    (&row.id, &row.parent).hash(&mut keys);
                }
                moved = true;
                self.rows = rows;
                self.trim();
                self.reindex();
                self.status = self.live_or_empty();
            }
            FeedEvent::Upsert(row) => {
                digest(&row, &mut content);
                match self
                    .index
                    .get(&row.id)
                    .and_then(|at| self.rows.get_mut(*at))
                {
                    Some(old) => {
                        if old.parent != row.parent {
                            (&row.id, &row.parent).hash(&mut keys);
                            moved = true;
                        }
                        *old = row;
                    }
                    None => {
                        (&row.id, &row.parent).hash(&mut keys);
                        moved = true;
                        self.index.insert(row.id.clone(), self.rows.len());
                        self.rows.push(row);
                        if self.trim() {
                            self.reindex();
                        }
                    }
                }
                self.status = FeedStatus::Live;
            }
            FeedEvent::Remove(id) => {
                let Some(at) = self.index.get(&id).copied() else {
                    return;
                };
                id.hash(&mut content);
                id.hash(&mut keys);
                moved = true;
                if at < self.rows.len() {
                    self.rows.remove(at);
                }
                self.reindex();
                if self.status == FeedStatus::Live {
                    self.status = self.live_or_empty();
                }
            }
            FeedEvent::Status(status) => {
                status.hash(&mut content);
                self.status = status;
            }
        }
        if moved {
            self.keys = keys.finish();
        }
        self.version = content.finish();
    }

    fn live_or_empty(&self) -> FeedStatus {
        if self.rows.is_empty() {
            FeedStatus::Empty
        } else {
            FeedStatus::Live
        }
    }

    /// Lets the oldest rows go once the table is over its cap, an eighth at a time; true
    /// when any went.
    fn trim(&mut self) -> bool {
        let over = self.rows.len().saturating_sub(MAX_ROWS);
        if over == 0 {
            return false;
        }
        let gone = over.max(MAX_ROWS / 8).min(self.rows.len());
        self.rows.drain(..gone);
        self.dropped = self
            .dropped
            .saturating_add(u64::try_from(gone).unwrap_or(u64::MAX));
        true
    }

    fn reindex(&mut self) {
        self.index = self
            .rows
            .iter()
            .enumerate()
            .map(|(at, row)| (row.id.clone(), at))
            .collect();
    }
}

impl Rows for Table {
    fn version(&self) -> u64 {
        self.version
    }

    fn len(&self) -> usize {
        self.rows.len()
    }

    fn get(&self, index: usize) -> Option<&Row> {
        self.rows.get(index)
    }
}

/// Feeds a row's whole content to `state`, floats by their bits.
fn digest(row: &Row, state: &mut DefaultHasher) {
    (&row.id, &row.parent, &row.group, row.dim, row.tone).hash(state);
    for (field, value) in &row.cells {
        field.hash(state);
        std::mem::discriminant(value).hash(state);
        match value {
            Value::Text(words) => words.hash(state),
            Value::Badge(words, tone) | Value::Pill(words, tone) => (words, tone).hash(state),
            Value::Dot(dot) => dot.hash(state),
            Value::Meter { ratio, text } => (ratio.to_bits(), text).hash(state),
            Value::Tags(tags) => tags.hash(state),
            Value::Bar { start, len, tone } => (start.to_bits(), len.to_bits(), tone).hash(state),
            Value::Flag(on) => on.hash(state),
            Value::Series(values) => values.iter().for_each(|v| v.to_bits().hash(state)),
        }
    }
}

/// Every feed's table.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Tables(BTreeMap<FeedId, Table>);

impl Tables {
    pub fn get(&self, id: FeedId) -> Option<&Table> {
        self.0.get(&id)
    }

    pub fn apply(&mut self, id: FeedId, event: FeedEvent) {
        self.0.entry(id).or_default().apply(event);
    }

    /// The feed whose table `table` is, found by address: a list handed a table from the
    /// model finds it again when its rows are built.
    pub fn feed_of(&self, table: &Table) -> Option<FeedId> {
        let mine = |(id, own): (&FeedId, &Table)| std::ptr::eq(own, table).then_some(*id);
        self.0.iter().find_map(mine)
    }
}
