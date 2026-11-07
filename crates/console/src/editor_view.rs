//! The editor pane's rows, cursor cell and click mapping, by drawn column rather than by char.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use yi_tui::colors::Theme;
use yi_tui::markdown::{cell_width, expand_tabs_at as expand};

use crate::model::Editor;

use std::ops::{Deref, DerefMut};

/// Rows above the text: the tab strip.
pub(crate) const TAB_ROWS: u16 = 1;

/// The files of one editor panel in tab order, one showing: `before`, `shown`, `after` is the
/// strip left to right. It derefs to the showing file, so what edited the panel's editor still does.
pub struct Tabs {
    before: Vec<Editor>,
    shown: Editor,
    after: Vec<Editor>,
    /// Where the last draw put each tab (or its ✕): the control table reads this.
    drawn: Vec<(Rect, usize, bool)>,
}

impl Deref for Tabs {
    type Target = Editor;
    fn deref(&self) -> &Editor {
        &self.shown
    }
}

impl DerefMut for Tabs {
    fn deref_mut(&mut self) -> &mut Editor {
        &mut self.shown
    }
}

impl Tabs {
    pub fn new(file: Editor) -> Self {
        Self {
            before: Vec::new(),
            shown: file,
            after: Vec::new(),
            drawn: Vec::new(),
        }
    }

    /// The showing file, the others dropped.
    pub fn into_shown(self) -> Editor {
        self.shown
    }

    pub fn count(&self) -> usize {
        self.before
            .len()
            .saturating_add(self.after.len())
            .saturating_add(1)
    }

    /// Every file, in strip order.
    pub fn files(&self) -> impl Iterator<Item = &Editor> {
        self.before
            .iter()
            .chain(std::iter::once(&self.shown))
            .chain(self.after.iter())
    }

    pub fn files_mut(&mut self) -> impl Iterator<Item = &mut Editor> {
        self.before
            .iter_mut()
            .chain(std::iter::once(&mut self.shown))
            .chain(self.after.iter_mut())
    }

    pub fn shown_index(&self) -> usize {
        self.before.len()
    }

    pub fn find(&self, path: &str) -> Option<usize> {
        self.files().position(|file| file.path == path)
    }

    /// Bring tab `index` forward; the order of the strip does not change.
    pub fn show(&mut self, index: usize) {
        let here = self.before.len();
        if index < here {
            let mut tail = self.before.split_off(index);
            let next = tail.remove(0);
            let old = std::mem::replace(&mut self.shown, next);
            tail.push(old);
            tail.append(&mut self.after);
            self.after = tail;
        } else if index > here {
            let k = index.saturating_sub(here).saturating_sub(1);
            if k >= self.after.len() {
                return;
            }
            let mut rest = self.after.split_off(k);
            let next = rest.remove(0);
            let old = std::mem::replace(&mut self.shown, next);
            self.before.push(old);
            self.before.append(&mut self.after);
            self.after = rest;
        }
    }

    /// Step to the next (or previous) tab, wrapping.
    pub fn step(&mut self, delta: isize) {
        let len = isize::try_from(self.count()).unwrap_or(1);
        let at = isize::try_from(self.before.len()).unwrap_or(0);
        let to = at.saturating_add(delta).rem_euclid(len);
        self.show(usize::try_from(to).unwrap_or(0));
    }

    /// A file opens to the right of the showing one and comes forward; one already open just does.
    pub fn open(&mut self, file: Editor) {
        if let Some(index) = self.find(&file.path) {
            return self.show(index);
        }
        let old = std::mem::replace(&mut self.shown, file);
        self.before.push(old);
    }

    /// Take tab `index` out of the strip; `None` when it is the only one.
    pub fn take(&mut self, index: usize) -> Option<Editor> {
        let here = self.before.len();
        if self.count() <= 1 {
            return None;
        }
        if index < here {
            return Some(self.before.remove(index));
        }
        if index > here {
            let k = index.saturating_sub(here).saturating_sub(1);
            return (k < self.after.len()).then(|| self.after.remove(k));
        }
        let next = if self.after.is_empty() {
            self.before.pop()?
        } else {
            self.after.remove(0)
        };
        Some(std::mem::replace(&mut self.shown, next))
    }
}

/// The tab strip as one line, and where each tab and its ✕ landed. Tabs that do not fit are cut
/// from the far side of the showing one and named in a trailing `+N more`.
fn strip(tabs: &mut Tabs, area: Rect, theme: &Theme) -> Line<'static> {
    const CLOSE: &str = " ✕ ";
    tabs.drawn.clear();
    let items: Vec<String> = tabs
        .files()
        .map(|file| {
            let mark = if file.dirty { " ●" } else { "" };
            format!(
                " {}{mark} ",
                crate::render::short_path(&file.path)
                    .rsplit('/')
                    .next()
                    .unwrap_or("")
            )
        })
        .collect();
    let width = |i: usize| {
        items
            .get(i)
            .map_or(0, |label| label.chars().count().saturating_add(3))
    };
    let (here, total) = (tabs.shown_index(), items.len());
    let room = usize::from(area.width);
    let all: usize = (0..total).map(width).sum();
    let budget = if all <= room {
        room
    } else {
        room.saturating_sub(10)
    };
    let (mut lo, mut hi, mut used) = (here, here, width(here));
    loop {
        let right = (hi.saturating_add(1) < total).then(|| width(hi.saturating_add(1)));
        let left = lo.checked_sub(1).map(width);
        match (right, left) {
            (Some(w), _) if used.saturating_add(w) <= budget => hi = hi.saturating_add(1),
            (_, Some(w)) if used.saturating_add(w) <= budget => lo = lo.saturating_sub(1),
            _ => break,
        }
        used = (lo..=hi).map(width).sum();
    }
    let on = Style::default()
        .fg(theme.text)
        .bg(theme.selection_bg())
        .add_modifier(Modifier::BOLD);
    let off = theme.muted_style();
    let mut spans = Vec::new();
    let mut x = area.x;
    for index in lo..=hi {
        let Some(label) = items.get(index) else {
            continue;
        };
        let style = if index == here { on } else { off };
        let (name_w, close_w) = (label.chars().count(), CLOSE.chars().count());
        let (name_w16, close_w16) = (
            u16::try_from(name_w).unwrap_or(0),
            u16::try_from(close_w).unwrap_or(0),
        );
        tabs.drawn
            .push((Rect::new(x, area.y, name_w16, 1), index, false));
        tabs.drawn.push((
            Rect::new(x.saturating_add(name_w16), area.y, close_w16, 1),
            index,
            true,
        ));
        spans.push(Span::styled(label.clone(), style));
        spans.push(Span::styled(CLOSE, style));
        x = x.saturating_add(name_w16).saturating_add(close_w16);
    }
    let hidden = total.saturating_sub(hi.saturating_sub(lo).saturating_add(1));
    if hidden > 0 {
        spans.push(Span::styled(
            format!(" +{hidden} more"),
            Style::default().fg(theme.warning),
        ));
    }
    Line::from(spans)
}

/// Each tab and ✕ the last draw laid out is a control the press and the hover walk.
pub(crate) fn offer_tabs(app: &mut crate::app::App) {
    let mut found = Vec::new();
    for (id, pane) in &app.state.panes {
        if let crate::model::PaneContent::Editor(tabs) = &pane.content {
            for (rect, index, close) in &tabs.drawn {
                let hit = if *close {
                    crate::hit::Hit::EditorClose(*id, *index)
                } else {
                    crate::hit::Hit::EditorTab(*id, *index)
                };
                found.push((*rect, hit));
            }
        }
    }
    if let Some(hits) = app.hits.as_mut() {
        hits.controls.extend(found);
    }
}

fn drawn_col(line: &str, index: usize) -> usize {
    line.chars()
        .take(index)
        .fold(0, |at, c| at.saturating_add(cell_width(c, at)))
}

/// The char drawn under column `col` of `line`, or the line's end past its last char.
/// ponytail: by char width, so a VS16 or ZWJ emoji before `col` shifts it; walk graphemes then.
pub(crate) fn char_at(line: &str, col: usize) -> usize {
    let mut at: usize = 0;
    for (index, c) in line.chars().enumerate() {
        at = at.saturating_add(cell_width(c, at));
        if col < at {
            return index;
        }
    }
    line.chars().count()
}

/// The spans of an already tab-expanded line from drawn column `skip` on.
fn crop(spans: Vec<Span<'static>>, skip: usize) -> Vec<Span<'static>> {
    if skip == 0 {
        return spans;
    }
    let mut left = skip;
    let mut out = Vec::new();
    for span in spans {
        let mut kept = String::new();
        for c in span.content.chars() {
            if left > 0 {
                let width = c.width().unwrap_or(0);
                if left < width {
                    kept.push(' ');
                }
                left = left.saturating_sub(width);
            } else {
                kept.push(c);
            }
        }
        if !kept.is_empty() {
            out.push(Span::styled(kept, span.style));
        }
    }
    out
}

pub(crate) fn widest(editor: &mut Editor) -> usize {
    if let Some(widest) = editor.widest {
        return widest;
    }
    let widest = editor
        .text
        .lines()
        .iter()
        .map(|line| expand(line, 0).width())
        .max()
        .unwrap_or(0);
    editor.widest = Some(widest);
    widest
}

pub(crate) fn cursor_cell(tabs: &Tabs, area: Rect) -> Option<(u16, u16)> {
    let editor = &tabs.shown;
    let inner = Rect {
        y: area.y.saturating_add(TAB_ROWS),
        height: area.height.saturating_sub(TAB_ROWS),
        ..area
    };
    let (row, col) = editor.text.cursor();
    let line = editor.text.lines().get(row)?;
    // Incident: an emoji with VS16 draws two cells where its chars sum to one; take the drawn
    // prefix's width, as the buffer does.
    let prefix: String = line.chars().take(col).collect();
    let x = inner
        .x
        .checked_add(u16::try_from(editor.gutter()).ok()?)?
        .checked_add(
            u16::try_from(expand(&prefix, 0).width().checked_sub(editor.scroll_left)?).ok()?,
        )?;
    let y = inner
        .y
        .checked_add(u16::try_from(row.checked_sub(editor.scroll_top)?).ok()?)?;
    (x < inner.x.saturating_add(inner.width) && y < inner.y.saturating_add(inner.height))
        .then_some((x, y))
}

pub(crate) fn view(
    tabs: &mut Tabs,
    area: Rect,
    theme: &Theme,
) -> (String, Vec<Line<'static>>, Option<(usize, usize)>) {
    let bar = strip(tabs, area, theme);
    let inner = Rect {
        y: area.y.saturating_add(TAB_ROWS),
        height: area.height.saturating_sub(TAB_ROWS),
        ..area
    };
    let editor = &mut tabs.shown;
    let height = usize::from(inner.height).max(1);
    let total = editor.text.lines().len();
    let cursor = editor.text.cursor();
    let moved = cursor != editor.last_cursor;
    if moved {
        editor.last_cursor = cursor;
        if cursor.0 < editor.scroll_top {
            editor.scroll_top = cursor.0;
        } else if cursor.0 >= editor.scroll_top.saturating_add(height) {
            editor.scroll_top = cursor.0.saturating_add(1).saturating_sub(height);
        }
    }
    editor.scroll_top = editor.scroll_top.min(total.saturating_sub(1));
    let room = usize::from(inner.width)
        .saturating_sub(editor.gutter())
        .max(1);
    if moved {
        let line = editor.text.lines().get(cursor.0).map_or("", String::as_str);
        let at = expand(&line.chars().take(cursor.1).collect::<String>(), 0).width();
        if at < editor.scroll_left {
            editor.scroll_left = at;
        } else if at >= editor.scroll_left.saturating_add(room) {
            editor.scroll_left = at.saturating_add(1).saturating_sub(room);
        }
    }
    let gutter = editor.gutter();
    let mut lang = editor.primed_lang();
    let selection = editor.text.selection_range();
    let base = Style::default().fg(theme.text);
    let mut out = Vec::new();
    for (index, line) in editor
        .text
        .lines()
        .iter()
        .enumerate()
        .skip(editor.scroll_top)
        .take(height)
    {
        let number = format!(
            "{:>width$} ",
            index.saturating_add(1),
            width = gutter.saturating_sub(1)
        );
        let mut spans = vec![Span::styled(number, theme.dim_style())];
        let selected = selection.and_then(|(start, end)| {
            let from = if index == start.0 { start.1 } else { 0 };
            let to = if index == end.0 {
                end.1
            } else {
                line.chars().count()
            };
            (start.0 <= index && index <= end.0).then_some((from, to))
        });
        match (selected, lang.as_mut()) {
            (Some((from, to)), _) => {
                let head: String = line.chars().take(from).collect();
                let mid: String = line
                    .chars()
                    .skip(from)
                    .take(to.saturating_sub(from))
                    .collect();
                let tail: String = line.chars().skip(to).collect();
                let head = expand(&head, 0);
                let mid = expand(&mid, drawn_col(line, from));
                let tail = expand(&tail, drawn_col(line, to));
                spans.push(Span::styled(head, base));
                spans.push(Span::styled(mid, base.add_modifier(Modifier::REVERSED)));
                spans.push(Span::styled(tail, base));
            }
            (None, Some(lang)) => {
                let drawn = expand(line, 0);
                spans.extend(yi_tui::highlight::spans(&drawn, lang, theme, base));
            }
            (None, None) => spans.push(Span::styled(expand(line, 0), base)),
        }
        let content = spans.split_off(1);
        spans.extend(crop(content, editor.scroll_left));
        out.push(Line::from(spans));
    }
    if editor.stale {
        out.truncate(height.saturating_sub(1));
        out.push(Line::styled(
            "file changed on disk · r reload · k keep",
            Style::default().fg(theme.warning),
        ));
    }
    let mark = if editor.dirty { " ●" } else { "" };
    let extent = (total > height).then_some((total, editor.scroll_top));
    let title = format!("✎ {}{mark}", crate::render::short_path(&editor.path));
    out.insert(0, bar);
    (title, out, extent)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str) -> Editor {
        Editor {
            path: path.to_owned(),
            text: Box::new(tui_textarea::TextArea::default()),
            dirty: false,
            mtime: None,
            stale: false,
            scroll_top: 0,
            widest: None,
            scroll_left: 0,
            lang: None,
            primed: None,
            last_cursor: (0, 0),
            crlf: false,
            final_newline: true,
            workdir: "/repo".to_owned(),
        }
    }

    fn order(tabs: &Tabs) -> Vec<&str> {
        tabs.files().map(|file| file.path.as_str()).collect()
    }

    fn three() -> Tabs {
        let mut tabs = Tabs::new(file("a"));
        tabs.open(file("b"));
        tabs.open(file("c"));
        tabs
    }

    #[test]
    fn a_file_opens_right_of_the_showing_one_and_an_open_file_only_comes_forward() {
        let mut tabs = three();
        tabs.show(0);
        tabs.open(file("d"));
        assert_eq!(
            order(&tabs),
            ["a", "d", "b", "c"],
            "d opens beside a, not at the end"
        );
        assert_eq!(tabs.path, "d");
        tabs.open(file("c"));
        assert_eq!(
            order(&tabs),
            ["a", "d", "b", "c"],
            "an open file was added twice"
        );
        assert_eq!(tabs.path, "c");
    }

    #[test]
    fn showing_a_tab_never_reorders_the_strip_and_stepping_wraps() {
        let mut tabs = three();
        for index in [0, 2, 1, 0] {
            tabs.show(index);
            assert_eq!(order(&tabs), ["a", "b", "c"], "show({index}) reordered");
            assert_eq!(tabs.shown_index(), index);
        }
        tabs.step(-1);
        assert_eq!(tabs.path, "c", "stepping back from the first did not wrap");
        tabs.step(1);
        assert_eq!(tabs.path, "a");
    }

    #[test]
    fn taking_the_showing_tab_shows_its_right_neighbour_and_the_last_tab_stays() {
        let mut tabs = three();
        tabs.show(1);
        let gone = tabs.take(1).map(|file| file.path);
        assert_eq!(gone.as_deref(), Some("b"));
        assert_eq!((order(&tabs), tabs.path.as_str()), (vec!["a", "c"], "c"));
        tabs.take(1);
        assert_eq!(
            tabs.path, "a",
            "with nothing to its right, the left one shows"
        );
        assert!(tabs.take(0).is_none(), "the only tab was taken");
        assert_eq!(tabs.count(), 1);
    }

    #[test]
    fn cropping_into_a_wide_character_keeps_the_rest_of_the_row_aligned() {
        let spans = crop(vec![Span::raw("界x")], 1);
        let text: String = spans.iter().map(|span| span.content.as_ref()).collect();
        assert_eq!(text, " x");
    }
}
