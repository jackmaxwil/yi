//! The Files button's tree: the workspace's files, folded by folder.

use std::path::Path;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use yi_tui::colors::Theme;

use crate::app::MouseKind;
use crate::client::Outbound;
use crate::model::{Mode, Zone};

/// Invariant: the notice names this cap and the total, so a tree cut here says what cut it.
pub const CAP: usize = 20_000;

pub struct Node {
    pub name: String,
    pub path: String,
    pub dir: bool,
    pub depth: usize,
    pub open: bool,
}

pub struct Files {
    pub root: String,
    pub nodes: Vec<Node>,
    /// An index into `nodes` that [`walk`] shows.
    pub cursor: usize,
    pub scroll: usize,
    pub notice: Option<String>,
}

/// A hidden path the tree never shows: git's own object store would drown everything else.
const NEVER: &str = ".git";
const MAX_DEPTH: usize = 32;

/// One `.gitignore` line, kept with the folder it was read from.
struct Rule {
    base: String,
    pattern: String,
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
            pattern: pattern.to_owned(),
            negate,
            dir_only,
            anchored: line.contains('/'),
        })
    }

    /// A rule only judges paths under the folder that declared it.
    fn matches(&self, path: &str, is_dir: bool) -> bool {
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
            let pattern: Vec<&str> = self.pattern.split('/').collect();
            let parts: Vec<&str> = relative.split('/').collect();
            components(&pattern, &parts)
        } else {
            relative.split('/').any(|name| glob(&self.pattern, name))
        }
    }
}

fn glob(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
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
fn components(pattern: &[&str], path: &[&str]) -> bool {
    let Some((head, rest)) = pattern.split_first() else {
        return path.is_empty();
    };
    if *head == "**" {
        return (0..=path.len()).any(|skip| match path.get(skip..) {
            Some(tail) => components(rest, tail),
            None => false,
        });
    }
    let Some((name, tail)) = path.split_first() else {
        return false;
    };
    glob(head, name) && components(rest, tail)
}

fn ignored(rules: &[Rule], path: &str, is_dir: bool) -> bool {
    rules.iter().fold(false, |verdict, rule| {
        if rule.matches(path, is_dir) {
            !rule.negate
        } else {
            verdict
        }
    })
}

struct Walk {
    kept: Vec<String>,
    seen: usize,
    cap: usize,
}

/// Paths in the order a folded tree draws them: each folder's entries sorted, its subtree after.
fn descend(root: &Path, rel: &str, depth: usize, rules: &mut Vec<Rule>, walk: &mut Walk) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root.join(rel)) else {
        return;
    };
    let mut names: Vec<(String, bool)> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| {
            let dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
            (entry.file_name().to_string_lossy().into_owned(), dir)
        })
        .collect();
    names.sort();
    let mark = rules.len();
    if let Ok(text) = std::fs::read_to_string(root.join(rel).join(".gitignore")) {
        rules.extend(text.lines().filter_map(|line| Rule::parse(rel, line)));
    }
    for (name, dir) in names {
        let child = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        if name == NEVER || ignored(rules, &child, dir) {
            continue;
        }
        if dir {
            descend(root, &child, depth.saturating_add(1), rules, walk);
        } else {
            walk.seen = walk.seen.saturating_add(1);
            if walk.kept.len() < walk.cap {
                walk.kept.push(child);
            }
        }
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
    pub fn open(root: &str) -> Self {
        Self::opened(root, CAP)
    }

    fn opened(root: &str, cap: usize) -> Self {
        let base = Path::new(root);
        let mut walk = Walk {
            kept: Vec::new(),
            seen: 0,
            cap,
        };
        let reachable = std::fs::read_dir(base);
        let notice = match reachable {
            Ok(_) => {
                descend(base, "", 0, &mut Vec::new(), &mut walk);
                (walk.seen > cap).then(|| {
                    format!(
                        "[{} of {} files kept · cap {cap} · `git ls-files` in a terminal lists \
                         the rest]",
                        walk.kept.len(),
                        walk.seen
                    )
                })
            }
            Err(error) => Some(format!("[no tree: {root}: {error}]")),
        };
        Self {
            root: root.to_owned(),
            nodes: nodes_of(root, &walk.kept),
            cursor: 0,
            scroll: 0,
            notice,
        }
    }

    pub fn current(&self) -> Option<&Node> {
        self.nodes.get(self.cursor)
    }

    pub fn step(&mut self, down: bool) {
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

    pub fn to_edge(&mut self, end: bool) {
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
    pub fn toggle(&mut self) -> bool {
        let Some(node) = self.nodes.get_mut(self.cursor) else {
            return false;
        };
        if !node.dir {
            return false;
        }
        node.open = !node.open;
        true
    }

    pub fn expand(&mut self) {
        if let Some(node) = self.nodes.get_mut(self.cursor)
            && node.dir
        {
            node.open = true;
        }
    }

    /// A folder closes; a closed folder and a file both walk out to their parent row.
    pub fn collapse(&mut self) {
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
    pub fn row_of_cursor(&self) -> usize {
        walk(&self.nodes)
            .take_while(|(index, _)| *index != self.cursor)
            .count()
    }

    pub fn scrolled(&mut self, rows: usize) {
        let rows = rows.max(1);
        let row = self.row_of_cursor();
        if row < self.scroll {
            self.scroll = row;
        } else if row >= self.scroll.saturating_add(rows) {
            self.scroll = row.saturating_add(1).saturating_sub(rows);
        }
    }

    /// The node drawn at `row` of the tree's own rows, below the popup's border.
    pub fn node_at(&self, row: usize, rows: usize) -> Option<usize> {
        if row >= rows {
            return None;
        }
        walk(&self.nodes)
            .map(|(index, _)| index)
            .nth(self.scroll.saturating_add(row))
    }
}

pub fn popup(area: Rect) -> Rect {
    let width = area.width.saturating_sub(8).clamp(40, 64).min(area.width);
    let height = area.height.saturating_sub(4).clamp(5, 30).min(area.height);
    Rect {
        x: area.x.saturating_add(area.width.saturating_sub(width) / 2),
        y: area.y.saturating_add(1),
        width,
        height,
    }
}

/// The tree's own rows above the notice row, for a popup `height` cells tall.
pub fn tree_rows(files: &Files, height: u16) -> usize {
    usize::from(height.saturating_sub(2)).saturating_sub(usize::from(files.notice.is_some()))
}

pub fn render(files: &Files, frame: &mut Frame<'_>, area: Rect, theme: &Theme) -> Rect {
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
pub fn row_at(popup: Rect, x: u16, y: u16, rows: usize) -> Option<usize> {
    let inside = popup.contains(Position::new(x, y));
    let row = y.checked_sub(popup.y.saturating_add(1)).map(usize::from)?;
    (inside && row < rows).then_some(row)
}

impl crate::app::App {
    pub(crate) fn open_files(&mut self) {
        let root = self.state.root.clone();
        self.state.mode = Mode::Files(Files::open(&root));
        self.dirty = true;
    }

    /// The two popups that take typed keys; the tree's rows are not a query.
    pub(crate) fn navigator_key(&mut self, outbound: &Outbound, key: KeyEvent) {
        if matches!(self.state.mode, Mode::Files(_)) {
            return self.files_key(key);
        }
        self.handle_navigator_key(outbound, key);
    }

    /// The tree's own rows in the last drawn popup, or what a fresh one would draw.
    fn files_rows(&self) -> usize {
        let height = self
            .hits
            .as_ref()
            .and_then(|hits| hits.overlays.first())
            .map_or(20, |rect| rect.height);
        match &self.state.mode {
            Mode::Files(files) => tree_rows(files, height),
            Mode::Normal | Mode::Prefix | Mode::Navigator { .. } | Mode::Menu(_) => {
                usize::from(height)
            }
        }
    }

    fn close_files(&mut self) {
        self.state.mode = Mode::Normal;
        self.dirty = true;
    }

    /// A folder row folds; a file hands its path to the editor and closes the tree.
    fn files_activate(&mut self) {
        let rows = self.files_rows();
        let mut opening = None;
        let mut folded = false;
        if let Mode::Files(files) = &mut self.state.mode {
            if files.toggle() {
                files.scrolled(rows);
                folded = true;
            } else {
                opening = files.current().map(|node| node.path.clone());
            }
        }
        if folded {
            self.dirty = true;
            return;
        }
        let Some(path) = opening else { return };
        let workdir = self.state.root.clone();
        self.state.mode = Mode::Normal;
        self.state.zone = Zone::Panes;
        self.open_editor(&path, workdir);
    }

    pub(crate) fn files_key(&mut self, key: KeyEvent) {
        let rows = self.files_rows();
        let Mode::Files(files) = &mut self.state.mode else {
            return;
        };
        match key.code {
            KeyCode::Esc => return self.close_files(),
            KeyCode::Up => files.step(false),
            KeyCode::Down => files.step(true),
            KeyCode::Left => files.collapse(),
            KeyCode::Right => files.expand(),
            KeyCode::PageUp => (0..rows).for_each(|_| files.step(false)),
            KeyCode::PageDown => (0..rows).for_each(|_| files.step(true)),
            KeyCode::Home => files.to_edge(false),
            KeyCode::End => files.to_edge(true),
            KeyCode::Char('r') => {
                let root = files.root.clone();
                *files = Files::open(&root);
            }
            KeyCode::Enter | KeyCode::Char(' ') => return self.files_activate(),
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
        let rows = self.files_rows();
        let Some(row) = row_at(popup, x, y, rows) else {
            return;
        };
        let mut pointed = false;
        if let Mode::Files(files) = &mut self.state.mode
            && let Some(index) = files.node_at(row, rows)
        {
            files.cursor = index;
            pointed = true;
        }
        self.dirty |= pointed;
        if pointed && matches!(kind, MouseKind::Down) {
            self.files_activate();
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

    fn rule(line: &str) -> Option<Rule> {
        Rule::parse("", line)
    }

    #[test]
    fn a_gitignore_line_judges_the_paths_a_repository_ignores() {
        let hide = |line: &str, path: &str, dir: bool| -> bool {
            rule(line).is_some_and(|rule| rule.matches(path, dir))
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
        let mut files = Files {
            root: "/repo".to_owned(),
            nodes: nodes(&["src/lib.rs", "src/tool.rs", "top.rs"]),
            cursor: 0,
            scroll: 0,
            notice: None,
        };
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
        let mut files = Files {
            root: "/repo".to_owned(),
            nodes: nodes(&["src/lib.rs", "top.rs"]),
            cursor: 0,
            scroll: 0,
            notice: None,
        };
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
        let mut files = Files {
            root: "/repo".to_owned(),
            nodes: nodes(&["src/deep/lib.rs"]),
            cursor: 1,
            scroll: 0,
            notice: None,
        };
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
    fn a_walk_past_the_cap_keeps_the_cap_and_names_the_total() -> std::io::Result<()> {
        let root = std::env::temp_dir().join(format!("yi-console-cap-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src"))?;
        for name in ["src/a.rs", "src/b.rs", "top.rs"] {
            std::fs::write(root.join(name), "")?;
        }
        let files = Files::opened(&root.display().to_string(), 2);
        let notice = files.notice.clone().unwrap_or_default();
        assert!(notice.contains("2 of 3 files kept"), "{notice}");
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
}
