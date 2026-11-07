//! The Files button's tree: the workspace's files, folded by folder.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use yi_tui::colors::Theme;
use yi_tui::popup::Ignore;

/// Invariant: the notice names this cap, so a tree cut here says what cut it.
const CAP: usize = 20_000;

pub(crate) struct Node {
    name: String,
    path: PathBuf,
    dir: bool,
    depth: usize,
    open: bool,
}

impl Node {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

pub struct Files {
    root: String,
    nodes: Vec<Node>,
    /// An index into `nodes` that [`walk`] shows.
    cursor: usize,
    scroll: usize,
    notice: Option<String>,
    /// Keys go to the tree while true; a press outside it hands them back to the panes.
    pub focused: bool,
    /// The row under the pointer, so a mouse user sees what a click will take.
    hover: Option<usize>,
}

/// A hidden path the tree never shows: git's own object store would drown everything else.
const NEVER: &str = ".git";
const MAX_DEPTH: usize = 32;
/// Rows one wheel notch moves, matching a pane's scroll step.
const WHEEL: usize = 3;

struct Walk {
    kept: Vec<PathBuf>,
    /// Canonical folders already entered, so a link back up a tree cannot walk forever.
    dirs: Vec<PathBuf>,
    /// Entries visited, folders and files alike: the budget bounds the read, not the output.
    visited: usize,
    stopped: bool,
    cap: usize,
}

/// Paths in the order a folded tree draws them: each folder's folders then files, each by name,
/// its subtree right after it.
fn descend(root: &Path, rel: &Path, depth: usize, ignore: &mut Ignore, walk: &mut Walk) {
    if depth > MAX_DEPTH || walk.stopped {
        return;
    }
    let dir = root.join(rel);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let mut names: Vec<(OsString, bool)> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let meta = std::fs::metadata(entry.path()).ok()?;
            Some((entry.file_name(), meta.is_dir()))
        })
        .collect();
    names.sort_by(|a, b| (!a.1).cmp(&!b.1).then_with(|| a.0.cmp(&b.0)));
    ignore.push_dir(&dir);
    for (name, is_dir) in names {
        walk.visited = walk.visited.saturating_add(1);
        if walk.visited > walk.cap {
            walk.stopped = true;
            return;
        }
        let child = rel.join(&name);
        let path = root.join(&child);
        if name == OsStr::new(NEVER) || ignore.ignored(&path, is_dir) {
            continue;
        }
        if !is_dir {
            walk.kept.push(child);
            continue;
        }
        let Ok(canonical) = std::fs::canonicalize(&path) else {
            continue;
        };
        if walk.dirs.contains(&canonical) {
            continue;
        }
        walk.dirs.push(canonical);
        descend(root, &child, depth.saturating_add(1), ignore, walk);
    }
}

/// Sorted paths to pre-order nodes: only the components a path does not share with the one
/// before it are new, so a folder's children sit contiguously after it.
fn nodes_of(root: &Path, paths: &[PathBuf]) -> Vec<Node> {
    let mut nodes: Vec<Node> = Vec::new();
    let mut last: Vec<&OsStr> = Vec::new();
    for path in paths {
        let parts: Vec<&OsStr> = path.iter().collect();
        let shared = last.iter().zip(&parts).take_while(|(a, b)| a == b).count();
        for (depth, name) in parts.iter().enumerate().skip(shared) {
            let rel: PathBuf = parts
                .iter()
                .take(depth.saturating_add(1))
                .copied()
                .collect();
            nodes.push(Node {
                name: name.to_string_lossy().into_owned(),
                path: root.join(rel),
                dir: depth.saturating_add(1) < parts.len(),
                depth,
                open: depth == 0,
            });
        }
        last = parts;
    }
    nodes
}
/// The nodes a drawn frame shows: a folded folder hides every deeper node after it.
fn walk(nodes: &[Node]) -> impl Iterator<Item = (usize, &Node)> {
    let mut hidden: Option<usize> = None;
    nodes.iter().enumerate().filter_map(move |(index, node)| {
        if hidden.is_some_and(|depth| node.depth > depth) {
            return None;
        }
        hidden = (node.dir && !node.open).then_some(node.depth);
        Some((index, node))
    })
}

impl Files {
    pub(crate) fn open(root: &str) -> Self {
        Self::opened(root, CAP)
    }

    fn opened(root: &str, cap: usize) -> Self {
        let base = Path::new(root);
        let mut walked = Walk {
            kept: Vec::new(),
            dirs: Vec::new(),
            visited: 0,
            stopped: false,
            cap,
        };
        let reachable = std::fs::read_dir(base);
        let notice = match reachable {
            Ok(_) => {
                if let Ok(canonical) = std::fs::canonicalize(base) {
                    walked.dirs.push(canonical);
                }
                descend(base, Path::new(""), 0, &mut Ignore::default(), &mut walked);
                walked.stopped.then(|| {
                    format!(
                        "[the first {} files kept in name order · cap {cap} entries visited · \
                         the walk stopped there · `git ls-files` in a terminal lists the rest]",
                        walked.kept.len()
                    )
                })
            }
            Err(error) => Some(format!("[no tree: {root}: {error}]")),
        };
        Self {
            root: root.to_owned(),
            nodes: nodes_of(base, &walked.kept),
            cursor: 0,
            scroll: 0,
            notice,
            focused: true,
            hover: None,
        }
    }

    pub(crate) fn current(&self) -> Option<&Node> {
        self.nodes.get(self.cursor)
    }

    pub(crate) fn step(&mut self, down: bool) {
        let mut before: Option<usize> = None;
        let mut after: Option<usize> = None;
        for (index, _) in walk(&self.nodes) {
            if index < self.cursor {
                before = Some(index);
            } else if index > self.cursor {
                after = Some(index);
                break;
            }
        }
        if let Some(index) = if down { after } else { before } {
            self.cursor = index;
        }
    }

    /// A whole page in one pass over the drawn rows, so a held key is not a page of walks.
    pub(crate) fn page(&mut self, rows: usize, down: bool) {
        let shown: Vec<usize> = walk(&self.nodes).map(|(index, _)| index).collect();
        let Some(at) = shown.iter().position(|index| *index == self.cursor) else {
            return;
        };
        let last = shown.len().saturating_sub(1);
        let to = if down {
            at.saturating_add(rows).min(last)
        } else {
            at.saturating_sub(rows)
        };
        if let Some(index) = shown.get(to) {
            self.cursor = *index;
        }
    }

    pub(crate) fn edge(&mut self, end: bool) {
        let found = if end {
            walk(&self.nodes).map(|(index, _)| index).last()
        } else {
            walk(&self.nodes).map(|(index, _)| index).next()
        };
        if let Some(index) = found {
            self.cursor = index;
        }
    }

    /// True when the row is a folder and folded or unfolded; a file is the caller's to open.
    pub(crate) fn toggle(&mut self) -> bool {
        let Some(node) = self.nodes.get_mut(self.cursor) else {
            return false;
        };
        if !node.dir {
            return false;
        }
        node.open = !node.open;
        true
    }

    pub(crate) fn expand(&mut self) {
        if let Some(node) = self.nodes.get_mut(self.cursor)
            && node.dir
        {
            node.open = true;
        }
    }

    pub(crate) fn root(&self) -> &str {
        &self.root
    }

    pub(crate) fn select(&mut self, index: usize) {
        self.cursor = index;
    }

    /// A folder closes; a closed folder and a file both walk out to their parent row.
    pub(crate) fn collapse(&mut self) {
        let Some(node) = self.nodes.get(self.cursor) else {
            return;
        };
        let (depth, folded) = (node.depth, node.dir && node.open);
        if folded {
            if let Some(node) = self.nodes.get_mut(self.cursor) {
                node.open = false;
            }
            return;
        }
        let parent = (0..self.cursor).rev().find(|index| {
            self.nodes
                .get(*index)
                .is_some_and(|node| node.depth < depth)
        });
        if let Some(index) = parent {
            self.cursor = index;
        }
    }

    /// The drawn row of the cursor, counting from the tree's first visible row.
    fn row_of_cursor(&self) -> usize {
        walk(&self.nodes)
            .take_while(|(index, _)| *index != self.cursor)
            .count()
    }

    fn visible_len(&self) -> usize {
        walk(&self.nodes).count()
    }

    pub(crate) fn scrolled(&mut self, rows: usize) {
        let rows = rows.max(1);
        let row = self.row_of_cursor();
        if row < self.scroll {
            self.scroll = row;
        } else if row >= self.scroll.saturating_add(rows) {
            self.scroll = row.saturating_add(1).saturating_sub(rows);
        }
        self.scroll = self.scroll.min(self.visible_len().saturating_sub(rows));
    }

    /// A wheel notch moves the window, leaving the cursor where the user left it.
    pub(crate) fn wheel(&mut self, down: bool, rows: usize) {
        let most = self.visible_len().saturating_sub(rows.max(1));
        self.scroll = if down {
            self.scroll.saturating_add(WHEEL).min(most)
        } else {
            self.scroll.saturating_sub(WHEEL)
        };
    }

    /// The node drawn at `row` of the tree's own rows, below the sidebar's border.
    fn node_at(&self, row: usize, rows: usize) -> Option<usize> {
        if row >= rows {
            return None;
        }
        walk(&self.nodes)
            .map(|(index, _)| index)
            .nth(self.scroll.saturating_add(row))
    }

    pub(crate) fn pointed(&self, side: Rect, x: u16, y: u16) -> Option<usize> {
        let rows = tree_rows(self, side);
        self.node_at(row_at(side, x, y, rows)?, rows)
    }

    /// A fresh walk that keeps which folders were open and the row the cursor was on.
    pub(crate) fn refresh(&mut self) {
        let was: HashMap<PathBuf, bool> = (self.nodes.iter())
            .filter(|node| node.dir)
            .map(|node| (node.path.clone(), node.open))
            .collect();
        let at = self.current().map(|node| node.path.clone());
        let (root, focused) = (self.root.clone(), self.focused);
        *self = Self::open(&root);
        for node in &mut self.nodes {
            node.open = was.get(&node.path).copied().unwrap_or(node.open);
        }
        self.cursor = at
            .and_then(|path| self.nodes.iter().position(|node| node.path == path))
            .unwrap_or(0);
        self.focused = focused;
    }

    pub(crate) fn collapse_all(&mut self) {
        self.nodes.iter_mut().for_each(|node| node.open = false);
        (self.cursor, self.scroll) = (0, 0);
    }

    pub(crate) fn hover(&mut self, to: Option<usize>) -> bool {
        std::mem::replace(&mut self.hover, to) != to
    }
}

/// The sidebar's rectangle on `area`'s right edge; empty when closed or where the panes would starve.
pub(crate) fn side(area: Rect, open: bool) -> Rect {
    let width = if open && area.width >= 60 {
        (area.width / 4).clamp(24, 44)
    } else {
        0
    };
    Rect {
        x: area.right().saturating_sub(width),
        width,
        ..area
    }
}

pub(crate) fn draw(app: &mut crate::app::App, frame: &mut Frame<'_>, theme: &Theme) {
    let side = app.hits.as_ref().and_then(|hits| hits.files);
    let controls = if let (Some(files), Some(side)) = (&app.state.files, side) {
        render(files, frame, side, theme)
    } else {
        Vec::new()
    };
    if let Some(hits) = app.hits.as_mut() {
        hits.controls.extend(controls);
    }
}

/// Invariant: the notice wraps rather than clips, so a cap's name and value are never cut off.
pub(crate) fn tree_rows(files: &Files, side: Rect) -> usize {
    let width = usize::from(side.width.saturating_sub(1)).max(1);
    let notice = files
        .notice
        .as_ref()
        .map_or(0, |notice| notice.chars().count().div_ceil(width));
    usize::from(side.height.saturating_sub(1)).saturating_sub(notice)
}

/// Header buttons ↻ ⊟ ✕, each a three-cell chip with a gap between and one cell in from the edge.
const BUTTONS: [&str; 3] = [" ↻ ", " ⊟ ", " ✕ "];
pub(crate) const BUTTONS_WIDTH: usize = 12;

pub(crate) fn render(
    files: &Files,
    frame: &mut Frame<'_>,
    side: Rect,
    theme: &Theme,
) -> Vec<(Rect, crate::hit::Hit)> {
    let width = usize::from(side.width.saturating_sub(1));
    let bar = if files.focused {
        Style::default().fg(theme.text)
    } else {
        theme.muted_style()
    };
    let leaf = Path::new(&files.root)
        .file_name()
        .map_or_else(
            || files.root.clone(),
            |name| name.to_string_lossy().into_owned(),
        )
        .to_uppercase();
    let room = width.saturating_sub(" EXPLORER: ".len() + BUTTONS_WIDTH + 1);
    let leaf = if leaf.chars().count() > room {
        let head: String = leaf.chars().take(room.saturating_sub(1)).collect();
        format!("{head}…")
    } else {
        leaf
    };
    let title = format!(" EXPLORER: {leaf}");
    let gap = width.saturating_sub(title.chars().count() + BUTTONS_WIDTH);
    let mut header = vec![
        Span::styled(
            title.clone(),
            theme.muted_style().add_modifier(Modifier::BOLD),
        ),
        Span::raw(" ".repeat(gap)),
    ];
    let chip = Style::default().fg(theme.text).bg(theme.selection_bg());
    let mut controls = Vec::new();
    let mut x = side
        .x
        .saturating_add(1)
        .saturating_add(u16::try_from(title.chars().count() + gap).unwrap_or(0));
    for (index, glyph) in BUTTONS.into_iter().enumerate() {
        let hit = match index {
            0 => crate::hit::Hit::FilesRefresh,
            1 => crate::hit::Hit::FilesCollapse,
            _ => crate::hit::Hit::FilesClose,
        };
        let width = u16::try_from(glyph.chars().count()).unwrap_or(0);
        controls.push((Rect::new(x, side.y, width, 1), hit));
        header.push(Span::styled(glyph, chip));
        header.push(Span::raw(" "));
        x = x.saturating_add(width).saturating_add(1);
    }
    let mut lines: Vec<Line<'static>> = vec![Line::from(header)];
    let rows = tree_rows(files, side);
    for (index, node) in walk(&files.nodes).skip(files.scroll).take(rows) {
        let tint = if index == files.cursor || files.hover == Some(index) {
            Style::default().bg(theme.selection_bg())
        } else {
            Style::default()
        };
        let name_style = if index == files.cursor {
            tint.fg(theme.text).add_modifier(Modifier::BOLD)
        } else {
            tint.fg(theme.text)
        };
        let glyph = match (node.dir, node.open) {
            (true, true) => "▾ ",
            (true, false) => "▸ ",
            (false, _) => "  ",
        };
        let used = 1 + 2 * node.depth + 2;
        let name: String = node.name.chars().take(width.saturating_sub(used)).collect();
        let pad = width.saturating_sub(used + name.chars().count());
        let mut spans = vec![Span::styled(" ", tint)];
        spans.extend((0..node.depth).map(|_| Span::styled("│ ", tint.fg(theme.dim))));
        spans.push(Span::styled(glyph, tint.fg(theme.muted)));
        spans.push(Span::styled(name, name_style));
        spans.push(Span::styled(" ".repeat(pad), tint));
        lines.push(Line::from(spans));
    }
    if files.nodes.is_empty() {
        lines.push(Line::styled(
            " nothing to list · r reloads",
            theme.dim_style(),
        ));
    }
    if let Some(notice) = &files.notice {
        lines.truncate(rows.saturating_add(1));
        let chars: Vec<char> = notice.chars().collect();
        for chunk in chars.chunks(width.max(1)) {
            lines.push(Line::styled(
                chunk.iter().collect::<String>(),
                Style::default().fg(theme.warning),
            ));
        }
    }
    let block = Block::new().borders(Borders::LEFT).border_style(bar);
    frame.render_widget(Paragraph::new(lines).block(block), side);
    controls
}

/// Invariant: a drawn row is the node under it, so a click and the frame agree.
fn row_at(side: Rect, x: u16, y: u16, rows: usize) -> Option<usize> {
    let inside = side.contains(Position::new(x, y));
    let row = y.checked_sub(side.y.saturating_add(1)).map(usize::from)?;
    (inside && row < rows).then_some(row)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nodes(paths: &[&str]) -> Vec<Node> {
        let owned: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        nodes_of(Path::new("/repo"), &owned)
    }

    fn tree(paths: &[&str]) -> Files {
        Files {
            root: "/repo".to_owned(),
            nodes: nodes(paths),
            cursor: 0,
            scroll: 0,
            notice: None,
            focused: true,
            hover: None,
        }
    }

    fn scratch(tag: &str) -> std::io::Result<PathBuf> {
        let root = std::env::temp_dir().join(format!("yi-console-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&root)?;
        Ok(root)
    }

    #[test]
    fn a_gitignore_hides_what_it_names_and_a_negation_puts_one_back() -> std::io::Result<()> {
        let root = scratch("ignore")?;
        for dir in ["build", "a/deep", "a/build"] {
            std::fs::create_dir_all(root.join(dir))?;
        }
        for name in [
            "debug.log",
            "keep.log",
            "build/x.rs",
            "a/build/y.rs",
            "a/deep/trace.log",
            "a/deep/keep.log",
        ] {
            std::fs::write(root.join(name), "")?;
        }
        std::fs::write(root.join(".gitignore"), "*.log\n!keep.log\n/build/\n")?;
        let files = Files::opened(&root.display().to_string(), 100);
        let listed = |name: &str| files.nodes.iter().any(|node| node.path == root.join(name));
        assert!(listed("keep.log"), "the negation put nothing back");
        assert!(
            listed("a/deep/keep.log"),
            "the negation stopped at the root"
        );
        assert!(!listed("debug.log"), "an ignored file reached the tree");
        assert!(!listed("a/deep/trace.log"), "the rule stopped at the root");
        assert!(!listed("build"), "an anchored rule reached the root folder");
        assert!(
            listed("a/build"),
            "a leading-slash rule hid a folder at depth"
        );
        std::fs::remove_dir_all(&root)?;
        Ok(())
    }

    #[test]
    fn a_paths_own_components_are_new_after_the_shared_prefix() {
        let nodes = nodes(&["crates/a/src/lib.rs", "crates/a/src/main.rs", "docs/x.md"]);
        let seen: Vec<(&str, usize, bool)> = nodes
            .iter()
            .map(|node| (node.name.as_str(), node.depth, node.dir))
            .collect();
        assert_eq!(
            seen,
            [
                ("crates", 0, true),
                ("a", 1, true),
                ("src", 2, true),
                ("lib.rs", 3, false),
                ("main.rs", 3, false),
                ("docs", 0, true),
                ("x.md", 1, false),
            ]
        );
        assert_eq!(nodes[3].path.to_string_lossy(), "/repo/crates/a/src/lib.rs");
    }

    #[test]
    fn a_folded_folder_hides_every_row_below_it() {
        let mut files = tree(&["src/lib.rs", "src/tool.rs", "top.rs"]);
        let shown = |files: &Files| -> Vec<String> {
            walk(&files.nodes)
                .map(|(_, node)| node.name.clone())
                .collect()
        };
        assert_eq!(
            shown(&files),
            ["src", "lib.rs", "tool.rs", "top.rs"],
            "the workspace's first level starts unfolded"
        );
        assert!(files.toggle(), "the folder row folded");
        assert_eq!(shown(&files), ["src", "top.rs"]);
    }

    #[test]
    fn the_cursor_steps_over_rows_folded_away() {
        let mut files = tree(&["src/lib.rs", "top.rs"]);
        assert!(files.toggle(), "the folder row folded");
        files.step(true);
        assert_eq!(
            files.current().map(|node| node.name.as_str()),
            Some("top.rs"),
            "down from a folded folder skips its children"
        );
        files.step(false);
        assert_eq!(files.current().map(|node| node.name.as_str()), Some("src"));
    }

    #[test]
    fn a_file_row_walks_out_to_the_folder_that_holds_it() {
        let mut files = tree(&["src/deep/lib.rs"]);
        files.cursor = 1;
        files.expand();
        files.cursor = 2;
        files.collapse();
        assert_eq!(
            files.current().map(|node| node.name.as_str()),
            Some("deep"),
            "left on a file steps to its folder"
        );
        files.collapse();
        assert_eq!(
            files.current().map(|node| node.name.as_str()),
            Some("deep"),
            "left on an unfolded folder folds it"
        );
        files.collapse();
        assert_eq!(
            files.current().map(|node| node.name.as_str()),
            Some("src"),
            "left on a folded folder steps to its parent"
        );
    }

    #[test]
    fn a_page_moves_the_cursor_a_window_at_a_time_and_stops_at_the_edges() {
        let names: Vec<String> = (0..40).map(|index| format!("f{index:02}.rs")).collect();
        let mut files = tree(&names.iter().map(String::as_str).collect::<Vec<_>>());
        files.page(10, true);
        assert_eq!(
            files.current().map(|node| node.name.as_str()),
            Some("f10.rs"),
            "one page down is one window of rows"
        );
        files.page(10, true);
        files.page(10, true);
        files.page(10, true);
        assert_eq!(
            files.current().map(|node| node.name.as_str()),
            Some("f39.rs"),
            "the last page lands on the last row, not past it"
        );
        files.page(10, false);
        assert_eq!(
            files.current().map(|node| node.name.as_str()),
            Some("f29.rs")
        );
    }

    #[test]
    fn a_wheel_notch_moves_the_window_and_leaves_the_cursor() {
        let names: Vec<String> = (0..40).map(|index| format!("f{index:02}.rs")).collect();
        let mut files = tree(&names.iter().map(String::as_str).collect::<Vec<_>>());
        files.wheel(true, 10);
        assert_eq!(files.scroll, WHEEL, "a notch moves the window");
        assert_eq!(
            files.current().map(|node| node.name.as_str()),
            Some("f00.rs"),
            "the wheel moved the cursor"
        );
        for _ in 0..20 {
            files.wheel(true, 10);
        }
        assert_eq!(
            files.scroll, 30,
            "the window stops at the last full page of rows"
        );
        files.wheel(false, 10);
        assert_eq!(files.scroll, 30 - WHEEL);
    }

    #[test]
    fn a_walk_past_the_cap_stops_there_and_says_what_it_kept() -> std::io::Result<()> {
        let root = scratch("cap")?;
        std::fs::create_dir_all(root.join("src"))?;
        for name in ["src/a.rs", "src/b.rs", "top.rs"] {
            std::fs::write(root.join(name), "")?;
        }
        let files = Files::opened(&root.display().to_string(), 3);
        let notice = files.notice.clone().unwrap_or_default();
        assert!(
            notice.contains("the first 2 files kept in name order"),
            "{notice}"
        );
        assert!(
            notice.contains("cap 3"),
            "the notice names the cap: {notice}"
        );
        assert!(notice.contains("lists the rest"), "{notice}");
        assert_eq!(
            files.nodes.len(),
            3,
            "two files and the folder that holds them"
        );
        std::fs::remove_dir_all(&root)?;
        Ok(())
    }

    #[test]
    fn a_tree_of_folders_spends_the_cap_too() -> std::io::Result<()> {
        let root = scratch("dirs")?;
        for name in ["d1", "d2", "d3", "d4"] {
            std::fs::create_dir_all(root.join(name))?;
        }
        let files = Files::opened(&root.display().to_string(), 3);
        let notice = files.notice.clone().unwrap_or_default();
        assert!(
            notice.contains("cap 3"),
            "a folder-heavy tree walked past the cap: {notice}"
        );
        assert!(files.nodes.is_empty(), "nothing under the cap was kept");
        std::fs::remove_dir_all(&root)?;
        Ok(())
    }

    // APFS refuses a name that is not UTF-8, so this runs where a filesystem carries one.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn a_name_that_is_not_utf8_is_still_a_path_the_tree_keeps() -> std::io::Result<()> {
        use std::os::unix::ffi::OsStringExt;
        let root = scratch("nonutf8")?;
        let name = OsString::from_vec(b"bad\xffname.rs".to_vec());
        std::fs::create_dir_all(root.join("d"))?;
        std::fs::write(root.join(&name), "")?;
        std::fs::write(root.join("d").join(&name), "")?;
        let files = Files::opened(&root.display().to_string(), 10);
        let listed: Vec<&PathBuf> = files
            .nodes
            .iter()
            .filter(|node| !node.dir)
            .map(|node| &node.path)
            .collect();
        assert_eq!(listed.len(), 2, "a non-UTF-8 name vanished: {listed:?}");
        assert!(
            listed.iter().all(|path| path.exists()),
            "a listed path does not exist: {listed:?}"
        );
        std::fs::remove_dir_all(&root)?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn a_link_to_a_folder_is_a_folder_the_tree_can_unfold() -> std::io::Result<()> {
        let root = std::env::temp_dir().join(format!("yi-console-link-{}", std::process::id()));
        std::fs::create_dir_all(root.join("real"))?;
        std::fs::write(root.join("real/leaf.rs"), "")?;
        std::os::unix::fs::symlink(root.join("real"), root.join("link"))?;
        let files = Files::opened(&root.display().to_string(), 10);
        let link = files
            .nodes
            .iter()
            .find(|node| node.name == "link")
            .ok_or_else(|| std::io::Error::other("no link row"))?;
        assert!(link.dir, "a link to a folder drew as a file row");
        let leaf = files
            .nodes
            .iter()
            .any(|node| node.path.ends_with("link/leaf.rs"));
        assert!(leaf, "the linked folder's subtree is missing the tree");
        std::fs::remove_dir_all(&root)?;
        Ok(())
    }

    #[test]
    fn a_folders_entries_list_its_folders_before_its_files() -> std::io::Result<()> {
        let root = scratch("order")?;
        std::fs::create_dir_all(root.join("zeta"))?;
        std::fs::write(root.join("zeta/inner.rs"), "")?;
        std::fs::write(root.join("alpha.rs"), "")?;
        std::fs::write(root.join(".hidden"), "")?;
        let files = Files::opened(&root.display().to_string(), 100);
        let names: Vec<&str> = files.nodes.iter().map(|node| node.name.as_str()).collect();
        assert_eq!(
            names,
            ["zeta", "inner.rs", ".hidden", "alpha.rs"],
            "a folder sorted among the files by name"
        );
        std::fs::remove_dir_all(&root)?;
        Ok(())
    }

    #[test]
    fn a_refresh_lists_a_new_file_and_keeps_what_the_user_unfolded() -> std::io::Result<()> {
        let root = scratch("refresh")?;
        std::fs::create_dir_all(root.join("a/b"))?;
        std::fs::write(root.join("a/b/y.rs"), "")?;
        let mut files = Files::opened(&root.display().to_string(), 100);
        files.cursor = 1;
        files.expand();
        std::fs::write(root.join("a/new.rs"), "")?;
        files.refresh();
        let shown: Vec<&str> = walk(&files.nodes)
            .map(|(_, node)| node.name.as_str())
            .collect();
        assert_eq!(shown, ["a", "b", "y.rs", "new.rs"], "{shown:?}");
        assert_eq!(files.current().map(|node| node.name.as_str()), Some("b"));
        std::fs::remove_dir_all(&root)?;
        Ok(())
    }

    #[test]
    fn collapse_all_folds_every_folder_and_returns_to_the_top() {
        let mut files = tree(&["a/x.rs", "a/b/y.rs", "top.rs"]);
        files.cursor = 3;
        files.collapse_all();
        let shown: Vec<&str> = walk(&files.nodes)
            .map(|(_, node)| node.name.as_str())
            .collect();
        assert_eq!(shown, ["a", "top.rs"]);
        assert_eq!(files.cursor, 0);
    }
}
