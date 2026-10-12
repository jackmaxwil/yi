//! The right-click menu: what was clicked, the verbs it offers, and where each level draws.
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use yi_tui::colors::Theme;

use crate::keys::{Action, chord_for};
use crate::layout::PaneId;

pub enum Target {
    /// Invariant: the source is read when the menu opens, so a verb acts on what was clicked
    /// even when the transcript streams or clears under the open menu.
    Message {
        pane: PaneId,
        source: String,
    },
    Pane(PaneId),
    Session(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Copy,
    CopyPlain,
    Quote,
    Act(Action),
    Open,
    OpenBeside,
}

#[derive(Clone)]
pub struct Item {
    pub label: &'static str,
    pub hint: String,
    pub verb: Option<Verb>,
    /// Why an item that applies to this target cannot run now; one that never applies is absent.
    pub blocked: Option<&'static str>,
    pub children: Vec<Item>,
}

pub struct Level {
    pub items: Vec<Item>,
    pub cursor: usize,
}

pub struct Menu {
    pub target: Target,
    pub at: (u16, u16),
    pub levels: Vec<Level>,
}

fn item(label: &'static str, verb: Verb, hint: &str) -> Item {
    Item {
        label,
        hint: hint.to_owned(),
        verb: Some(verb),
        blocked: None,
        children: Vec::new(),
    }
}

pub struct Facts {
    pub cmd: bool,
    pub last_pane: bool,
    pub zoomed: bool,
}

pub fn items(target: &Target, facts: &Facts) -> Vec<Item> {
    let act = |label, action| item(label, Verb::Act(action), chord_for(action, facts.cmd));
    match target {
        Target::Message { .. } => vec![
            item("Copy", Verb::Copy, ""),
            Item {
                label: "Copy as",
                hint: "▸".to_owned(),
                verb: None,
                blocked: None,
                children: vec![item("Plain text", Verb::CopyPlain, "")],
            },
            item("Quote", Verb::Quote, ""),
        ],
        Target::Pane(_) => vec![
            Item {
                blocked: facts.last_pane.then_some("last pane"),
                ..act("Close pane", Action::ClosePane)
            },
            act(if facts.zoomed { "Unzoom" } else { "Zoom" }, Action::Zoom),
            act("Split right", Action::SplitRight),
            act("Split down", Action::SplitDown),
        ],
        Target::Session(_) => vec![
            item("Open", Verb::Open, "enter"),
            item("Open beside", Verb::OpenBeside, ""),
        ],
    }
}

impl Menu {
    pub fn new(target: Target, at: (u16, u16), facts: &Facts) -> Self {
        let items = items(&target, facts);
        Self {
            target,
            at,
            levels: vec![Level { items, cursor: 0 }],
        }
    }

    pub fn current(&self) -> Option<&Item> {
        let level = self.levels.last()?;
        level.items.get(level.cursor)
    }

    pub fn descend(&mut self) -> bool {
        let Some(items) = self.current().map(|item| item.children.clone()) else {
            return false;
        };
        let opened = !items.is_empty();
        if opened {
            self.levels.push(Level { items, cursor: 0 });
        }
        opened
    }

    pub fn step(&mut self, down: bool) {
        if let Some(level) = self.levels.last_mut() {
            let count = level.items.len().max(1);
            level.cursor = if down {
                level.cursor.saturating_add(1) % count
            } else {
                level
                    .cursor
                    .checked_sub(1)
                    .unwrap_or(count.saturating_sub(1))
            };
        }
    }

    pub fn point(&mut self, depth: usize, row: usize) -> bool {
        let deeper = self.levels.len() != depth.saturating_add(1);
        let Some(level) = self.levels.get_mut(depth) else {
            return false;
        };
        if row >= level.items.len() {
            return false;
        }
        let moved = level.cursor != row || deeper;
        level.cursor = row;
        self.levels.truncate(depth.saturating_add(1));
        moved
    }
}

fn shown_hint(item: &Item) -> &str {
    item.blocked.unwrap_or(&item.hint)
}

fn size(items: &[Item]) -> (u16, u16) {
    let label = items.iter().map(|item| item.label.chars().count()).max();
    let hint = items
        .iter()
        .map(|item| shown_hint(item).chars().count())
        .max();
    let width = label
        .unwrap_or(0)
        .saturating_add(hint.unwrap_or(0))
        .saturating_add(6);
    let height = items.len().saturating_add(2);
    (
        u16::try_from(width).unwrap_or(u16::MAX),
        u16::try_from(height).unwrap_or(u16::MAX),
    )
}

/// Each level's box on `screen`: the first at the click, a submenu beside its parent's row,
/// flipped left where the right edge would cut it.
pub fn rects(menu: &Menu, screen: Rect) -> Vec<Rect> {
    let mut out: Vec<Rect> = Vec::new();
    for (depth, level) in menu.levels.iter().enumerate() {
        let (width, height) = size(&level.items);
        let (width, height) = (width.min(screen.width), height.min(screen.height));
        let (x, y) = match (out.last(), depth.checked_sub(1)) {
            (Some(parent), Some(up)) => {
                let cursor = menu.levels.get(up).map_or(0, |level| level.cursor);
                let row = parent.y.saturating_add(u16::try_from(cursor).unwrap_or(0));
                if parent.right().saturating_add(width) <= screen.right() {
                    (parent.right(), row)
                } else {
                    (parent.x.saturating_sub(width), row)
                }
            }
            _ => menu.at,
        };
        let x = x.min(screen.right().saturating_sub(width)).max(screen.x);
        let y = y.min(screen.bottom().saturating_sub(height)).max(screen.y);
        out.push(Rect::new(x, y, width, height));
    }
    out
}

pub fn hit(menu: &Menu, rects: &[Rect], x: u16, y: u16) -> Option<(usize, Option<usize>)> {
    let (depth, rect) = rects
        .iter()
        .enumerate()
        .rev()
        .find(|(_, rect)| rect.contains(Position::new(x, y)))?;
    let row = y.checked_sub(rect.y.saturating_add(1)).map(usize::from);
    let count = menu.levels.get(depth).map_or(0, |level| level.items.len());
    Some((depth, row.filter(|row| *row < count)))
}

pub fn render(menu: &Menu, rects: &[Rect], frame: &mut Frame<'_>, theme: &Theme) {
    for (level, rect) in menu.levels.iter().zip(rects) {
        frame.render_widget(Clear, *rect);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(theme.accent_style())
            .style(Style::default().bg(theme.selection_bg()));
        let label_width = level
            .items
            .iter()
            .map(|item| item.label.chars().count())
            .max()
            .unwrap_or(0);
        let inner = usize::from(rect.width.saturating_sub(2));
        let lines: Vec<Line<'static>> = level
            .items
            .iter()
            .enumerate()
            .map(|(row, item)| {
                let hint = shown_hint(item);
                let pad = inner
                    .saturating_sub(label_width)
                    .saturating_sub(hint.chars().count())
                    .saturating_sub(2);
                let text = format!(" {:<label_width$}{} {hint}", item.label, " ".repeat(pad));
                let base = if item.blocked.is_some() {
                    theme.dim_style()
                } else {
                    Style::default().fg(theme.text)
                };
                let style = if row == level.cursor {
                    base.add_modifier(Modifier::REVERSED)
                } else {
                    base
                };
                Line::from(Span::styled(text, style))
            })
            .collect();
        frame.render_widget(Paragraph::new(lines).block(block), *rect);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts {
            cmd: false,
            last_pane: false,
            zoomed: false,
        }
    }

    #[test]
    fn a_submenu_flips_left_where_the_right_edge_would_cut_it() -> Result<(), String> {
        let target = Target::Message {
            pane: crate::layout::PaneIds::default().allocate(),
            source: "hi".to_owned(),
        };
        let mut menu = Menu::new(target, (70, 3), &facts());
        menu.point(0, 1);
        assert!(menu.descend());
        let screen = Rect::new(0, 0, 80, 24);
        let boxes = rects(&menu, screen);
        let (Some(root), Some(sub)) = (boxes.first(), boxes.get(1)) else {
            return Err(format!("two levels drew {boxes:?}"));
        };
        assert!(root.right() <= screen.right());
        assert_eq!(
            sub.right(),
            root.x,
            "the submenu sits flush left of its parent"
        );
        assert_eq!(
            sub.y,
            root.y.saturating_add(1),
            "its first row beside Copy as"
        );
        Ok(())
    }
}
