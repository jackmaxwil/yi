//! The Files button's tree: the workspace's files, folded by folder.

use std::path::{Path, PathBuf};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use yi_tui::colors::Theme;

use crate::app::MouseKind;
use crate::model::{Mode, Zone};

/// Invariant: the notice names this cap, so a tree cut here says what cut it.
const CAP: usize = 20_000;

struct Node {
    name: String,
    path: String,
    dir: bool,
    depth: usize,
    open: bool,
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

/// One `.gitignore` line, its globs split once, kept with the folder it was read from.
struct Rule {
    base: String,
    segments: Vec<Vec<char>>,
    negate: bool,
    dir_only: bool,
    anchored: bool,
}

impl Rule {
    fn parse(base: &str, line: &str) -> Option<Self> {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let (negate, line) = match line.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, line),
        };
        let (dir_only, line) = match line.strip_suffix('/') {
            Some(rest) => (true, rest),
            None => (false, line),
        };
        let pattern = line.strip_prefix('/').unwrap_or(line);
        (!pattern.is_empty()).then(|| Self {
            base: base.to_owned(),
            segments: pattern
                .split('/')
                .map(|part| part.chars().collect())
                .collect(),
            negate,
            dir_only,
            anchored: line.contains('/'),
        })
    }

    /// A rule only judges paths under the folder that declared it.
    fn matches(&self, path: &str, is_dir: bool, globbed: &mut Vec<char>) -> bool {
        if self.dir_only && !is_dir {
            return false;
        }
        let relative = if self.base.is_empty() {
            path
        } else {
            let Some(rest) = path.strip_prefix(&self.base) else {
                return false;
            };
            let Some(rest) = rest.strip_prefix('/') else {
                return false;
            };
            rest
        };
        if self.anchored {
            let parts: Vec<&str> = relative.split('/').collect();
            components(&self.segments, &parts, globbed)
        } else {
            let Some(segment) = self.segments.first() else {
                return false;
            };
            relative.split('/').any(|name| {
                globbed.clear();
                globbed.extend(name.chars());
                glob(segment, globbed)
            })
        }
    }
}

fn glob(pattern: &[char], name: &[char]) -> bool {
    let (mut at, mut from) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut mark = 0usize;
    while from < name.len() {
        match pattern.get(at) {
            Some('*') => {
                star = Some(at);
                mark = from;
                at = at.saturating_add(1);
            }
            Some('?') => {
                at = at.saturating_add(1);
                from = from.saturating_add(1);
            }
            Some(char) if Some(char) == name.get(from) => {
                at = at.saturating_add(1);
                from = from.saturating_add(1);
            }
            _ => match star {
                Some(star) => {
                    at = star.saturating_add(1);
                    mark = mark.saturating_add(1);
                    from = mark;
                }
                None => return false,
            },
        }
    }
    while pattern.get(at) == Some(&'*') {
        at = at.saturating_add(1);
    }
    at == pattern.len()
}

/// `**` stands for any run of folders, the other segments for one name each.
fn components(pattern: &[Vec<char>], path: &[&str], globbed: &mut Vec<char>) -> bool {
    let Some((head, rest)) = pattern.split_first() else {
        return path.is_empty();
    };
    if head.as_slice() == ['*', '*'] {
        return (0..=path.len()).any(|skip| match path.get(skip..) {
            Some(tail) => components(rest, tail, globbed),
            None => false,
        });
    }
    let Some((name, tail)) = path.split_first() else {
        return false;
    };
    globbed.clear();
    globbed.extend(name.chars());
    glob(head, globbed) && components(rest, tail, globbed)
}

fn ignored(rules: &[Rule], path: &str, is_dir: bool) -> bool {
    let mut globbed: Vec<char> = Vec::new();
    rules.iter().fold(false, |verdict, rule| {
        if rule.matches(path, is_dir, &mut globbed) {
            !rule.negate
        } else {
            verdict
        }
    })
}

struct Walk {
    kept: Vec<String>,
    /// Canonical folders already entered, so a link back up a tree cannot walk forever.
    dirs: Vec<PathBuf>,
    stopped: bool,
    cap: usize,
}

/// Paths in the order a folded tree draws them: each folder's entries sorted, its subtree after.
fn descend(root: &Path, rel: &str, depth: usize, rules: &mut Vec<Rule>, walk: &mut Walk) {
    if depth > MAX_DEPTH || walk.kept.len() >= walk.cap {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root.join(rel)) else {
        return;
    };
    let mut names: Vec<(String, bool)> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let meta = std::fs::metadata(entry.path()).ok()?;
            Some((
                entry.file_name().to_string_lossy().into_owned(),
                meta.is_dir(),
            ))
        })
        .collect();
    names.sort();
    let mark = rules.len();
    if let Ok(text) = std::fs::read_to_string(root.join(rel).join(".gitignore")) {
        rules.extend(text.lines().filter_map(|line| Rule::parse(rel, line)));
    }
    for (name, dir) in names {
        if walk.kept.len() >= walk.cap {
            walk.stopped = true;
            break;
        }
        let child = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        if name == NEVER || ignored(rules, &child, dir) {
            continue;
        }
        if !dir {
            walk.kept.push(child);
            continue;
        }
        let Ok(canonical) = std::fs::canonicalize(root.join(&child)) else {
            continue;
        };
        if walk.dirs.contains(&canonical) {
            continue;
        }
        walk.dirs.push(canonical);
        descend(root, &child, depth.saturating_add(1), rules, walk);
    }
    rules.truncate(mark);
}

/// Sorted paths to pre-order nodes: only the components a path does not share with the one
/// before it are new, so a folder's children sit contiguously after it.
fn nodes_of(root: &str, paths: &[String]) -> Vec<Node> {
    let base = root.trim_end_matches('/');
    let mut nodes: Vec<Node> = Vec::new();
    let mut last: Vec<&str> = Vec::new();
    for path in paths {
        let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
        let shared = last.iter().zip(&parts).take_while(|(a, b)| a == b).count();
        for (depth, name) in parts.iter().enumerate().skip(shared) {
            let joined = parts.get(..=depth).map(|tail| tail.join("/"));
            nodes.push(Node {
                name: (*name).to_owned(),
                path: format!("{base}/{}", joined.unwrap_or_default()),
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
    fn open(root: &str) -> Self {
        Self::opened(root, CAP)
    }

    fn opened(root: &str, cap: usize) -> Self {
        let base = Path::new(root);
        let mut walked = Walk {
            kept: Vec::new(),
            dirs: Vec::new(),
            stopped: false,
            cap,
        };
        let reachable = std::fs::read_dir(base);
        let notice = match reachable {
            Ok(_) => {
                if let Ok(canonical) = std::fs::canonicalize(base) {
                    walked.dirs.push(canonical);
                }
                descend(base, "", 0, &mut Vec::new(), &mut walked);
                walked.stopped.then(|| {
                    format!(
                        "[the first {} files kept in name order · cap {cap} · the walk stopped \
                         there · `git ls-files` in a terminal lists the rest]",
                        walked.kept.len()
                    )
                })
            }
            Err(error) => Some(format!("[no tree: {root}: {error}]")),
        };
        Self {
            root: root.to_owned(),
            nodes: nodes_of(root, &walked.kept),
            cursor: 0,
            scroll: 0,
            notice,
        }
    }

    fn current(&self) -> Option<&Node> {
        self.nodes.get(self.cursor)
    }

    fn step(&mut self, down: bool) {
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
    fn page(&mut self, rows: usize, down: bool) {
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

    fn edge(&mut self, end: bool) {
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
    fn toggle(&mut self) -> bool {
        let Some(node) = self.nodes.get_mut(self.cursor) else {
            return false;
        };
        if !node.dir {
            return false;
        }
        node.open = !node.open;
        true
    }

    fn expand(&mut self) {
        if let Some(node) = self.nodes.get_mut(self.cursor)
            && node.dir
        {
            node.open = true;
        }
    }

    /// A folder closes; a closed folder and a file both walk out to their parent row.
    fn collapse(&mut self) {
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

    fn scrolled(&mut self, rows: usize) {
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
    fn wheel(&mut self, down: bool, rows: usize) {
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

    fn pointed(&self, popup: Rect, x: u16, y: u16, height: u16) -> Option<usize> {
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
fn tree_rows(files: &Files, height: u16) -> usize {
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

impl crate::app::App {
    pub(crate) fn open_files(&mut self) {
        let root = self.session_dir();
        self.state.mode = Mode::Files(Files::open(&root));
        self.dirty = true;
    }

    fn overlay_height(&self) -> u16 {
        self.hits
            .as_ref()
            .and_then(|hits| hits.overlays.first())
            .map_or(20, |rect| rect.height)
    }

    fn close_files(&mut self) {
        self.state.mode = Mode::Normal;
        self.dirty = true;
    }

    /// A folder row folds; a file hands its path to the editor and closes the tree.
    fn files_activate(&mut self) {
        let height = self.overlay_height();
        let mut opening = None;
        if let Mode::Files(files) = &mut self.state.mode {
            if files.toggle() {
                let rows = tree_rows(files, height);
                files.scrolled(rows);
                self.dirty = true;
                return;
            }
            opening = files
                .current()
                .map(|node| (node.path.clone(), files.root.clone()));
        }
        let Some((path, workdir)) = opening else {
            return;
        };
        self.state.mode = Mode::Normal;
        self.state.zone = Zone::Panes;
        self.open_editor(&path, workdir);
    }

    pub(crate) fn files_key(&mut self, key: KeyEvent) {
        let height = self.overlay_height();
        let Mode::Files(files) = &mut self.state.mode else {
            return;
        };
        let rows = tree_rows(files, height);
        match key.code {
            KeyCode::Esc => return self.close_files(),
            KeyCode::Up => files.step(false),
            KeyCode::Down => files.step(true),
            KeyCode::Left => files.collapse(),
            KeyCode::Right => files.expand(),
            KeyCode::PageUp => files.page(rows, false),
            KeyCode::PageDown => files.page(rows, true),
            KeyCode::Home => files.edge(false),
            KeyCode::End => files.edge(true),
            KeyCode::Char('r') => {
                let root = files.root.clone();
                *files = Files::open(&root);
            }
            _ => {}
        }
        files.scrolled(rows);
        self.dirty = true;
    }

    pub(crate) fn files_mouse(&mut self, kind: MouseKind, x: u16, y: u16) {
        let Some(hits) = &self.hits else { return };
        let Some(popup) = hits.overlays.first().copied() else {
            return;
        };
        let at = Position::new(x, y);
        // A popup takes every press: one outside it, or on its close mark, only dismisses it.
        if !popup.contains(at) || hits.dismiss.is_some_and(|mark| mark.contains(at)) {
            if matches!(kind, MouseKind::Down | MouseKind::RightDown) {
                self.close_files();
            }
            return;
        }
        let height = self.overlay_height();
        let clicked = match &self.state.mode {
            Mode::Files(files) if kind == MouseKind::Down => files.pointed(popup, x, y, height),
            Mode::Normal
            | Mode::Prefix
            | Mode::Navigator { .. }
            | Mode::Files(_)
            | Mode::Menu(_) => None,
        };
        match kind {
            MouseKind::ScrollUp | MouseKind::ScrollDown => {
                let down = kind == MouseKind::ScrollDown;
                if let Mode::Files(files) = &mut self.state.mode {
                    let rows = tree_rows(files, height);
                    files.wheel(down, rows);
                }
                self.dirty = true;
            }
            MouseKind::Down => {
                let Some(index) = clicked else { return };
                if let Mode::Files(files) = &mut self.state.mode {
                    files.cursor = index;
                }
                self.dirty = true;
                self.files_activate();
            }
            MouseKind::Moved | MouseKind::Up | MouseKind::Drag | MouseKind::RightDown => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nodes(paths: &[&str]) -> Vec<Node> {
        let owned: Vec<String> = paths.iter().map(|path| (*path).to_owned()).collect();
        nodes_of("/repo", &owned)
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

    fn rule(line: &str) -> Option<Rule> {
        Rule::parse("", line)
    }

    #[test]
    fn a_gitignore_line_judges_the_paths_a_repository_ignores() {
        let hide = |line: &str, path: &str, dir: bool| -> bool {
            rule(line).is_some_and(|rule| rule.matches(path, dir, &mut Vec::new()))
        };
        assert!(
            hide("*.log", "debug.log", false),
            "a bare glob hides a name"
        );
        assert!(hide("*.log", "a/deep/debug.log", false), "at any depth");
        assert!(hide("target/", "target", true), "a slash hides a folder");
        assert!(
            !hide("target/", "target", false),
            "and never a file of that name"
        );
        assert!(
            hide("src/gen/x.rs", "src/gen/x.rs", false),
            "a path anchors"
        );
        assert!(
            !hide("src/gen/x.rs", "gen/x.rs", false),
            "to where it was read"
        );
        assert!(
            hide("**/keep.rs", "a/b/keep.rs", false),
            "** crosses folders"
        );
        assert!(!hide("#*.rs", "a.rs", false), "a comment hides nothing");
    }

    #[test]
    fn a_negated_line_puts_back_what_an_earlier_line_hid() {
        let rules: Vec<Rule> = ["*.log", "!keep.log"]
            .iter()
            .filter_map(|line| Rule::parse("", line))
            .collect();
        assert!(ignored(&rules, "debug.log", false));
        assert!(!ignored(&rules, "keep.log", false), "the last line wins");
        assert!(!ignored(&rules, "keep.rs", false));
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
        assert_eq!(nodes[3].path, "/repo/crates/a/src/lib.rs");
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
        let root = std::env::temp_dir().join(format!("yi-console-cap-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src"))?;
        for name in ["src/a.rs", "src/b.rs", "top.rs"] {
            std::fs::write(root.join(name), "")?;
        }
        let files = Files::opened(&root.display().to_string(), 2);
        let notice = files.notice.clone().unwrap_or_default();
        assert!(
            notice.contains("the first 2 files kept in name order"),
            "{notice}"
        );
        assert!(
            notice.contains("cap 2"),
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
