//! The Files button's tree: the workspace's files, folded by folder.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
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

/// Paths in the order a folded tree draws them: each folder's entries sorted, its subtree after.
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
    names.sort();
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

    /// The node drawn at `row` of the tree's own rows, below the popup's border.
    fn node_at(&self, row: usize, rows: usize) -> Option<usize> {
        if row >= rows {
            return None;
        }
        walk(&self.nodes)
            .map(|(index, _)| index)
            .nth(self.scroll.saturating_add(row))
    }

    pub(crate) fn pointed(&self, popup: Rect, x: u16, y: u16, height: u16) -> Option<usize> {
        let rows = tree_rows(self, height);
        self.node_at(row_at(popup, x, y, rows)?, rows)
    }
}

fn popup(area: Rect) -> Rect {
    let width = area.width.saturating_sub(8).clamp(40, 64).min(area.width);
    let height = area.height.saturating_sub(4).clamp(5, 30).min(area.height);
    crate::palette::popup_in(area, width, height)
}

/// The tree's own rows above the notice row, for a popup `height` cells tall.
pub(crate) fn tree_rows(files: &Files, height: u16) -> usize {
    usize::from(height.saturating_sub(2)).saturating_sub(usize::from(files.notice.is_some()))
}

pub(crate) fn render(files: &Files, frame: &mut Frame<'_>, area: Rect, theme: &Theme) -> Rect {
    let popup = popup(area);
    frame.render_widget(Clear, popup);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme.muted_style())
        .title(format!(
            " Files · {} ",
            crate::render::short_path(&files.root)
        ))
        .title_top(Line::from(crate::palette::DISMISS).right_aligned());
    let rows = tree_rows(files, popup.height);
    let width = usize::from(popup.width.saturating_sub(2));
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (index, node) in walk(&files.nodes).skip(files.scroll).take(rows) {
        let picked = index == files.cursor;
        let indent = "  ".repeat(node.depth);
        let glyph = match (node.dir, node.open) {
            (true, true) => "▾ ",
            (true, false) => "▸ ",
            (false, _) => "  ",
        };
        let base = if node.dir {
            Style::default().fg(theme.muted)
        } else {
            Style::default().fg(theme.text)
        };
        let style = if picked {
            base.bg(theme.selection_bg()).add_modifier(Modifier::BOLD)
        } else {
            base
        };
        let text = format!(" {indent}{glyph}{}", node.name);
        let pad = width.saturating_sub(text.chars().count());
        lines.push(Line::from(vec![
            Span::styled(text, style),
            Span::styled(" ".repeat(pad), style),
        ]));
    }
    if files.nodes.is_empty() {
        lines.push(Line::styled(
            "  nothing to list — r reloads",
            theme.dim_style(),
        ));
    }
    if let Some(notice) = &files.notice {
        lines.truncate(rows);
        lines.push(Line::styled(
            notice.clone(),
            Style::default().fg(theme.warning),
        ));
    }
    frame.render_widget(Paragraph::new(lines).block(block), popup);
    popup
}

/// Invariant: a drawn row is the node under it, so a click and the frame agree.
fn row_at(popup: Rect, x: u16, y: u16, rows: usize) -> Option<usize> {
    let inside = popup.contains(Position::new(x, y));
    let row = y.checked_sub(popup.y.saturating_add(1)).map(usize::from)?;
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
}
