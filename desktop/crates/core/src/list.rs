//! A list pane is data: its columns, whether it is a tree, and what an opened row shows.

use crate::address::uri::Address;
use crate::ids::CommandId;
use crate::kind::Kind;
use crate::row::{Row, Value};

/// How wide a column is: fixed points, or a share of what the fixed ones leave.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Width {
    Px(f32),
    Fr(f32),
}

/// How a column's words are set. Dots, meters, bars and tags draw as their value says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Text {
    /// The thing itself: bright and bold.
    Name,
    Plain,
    Mono,
    /// Right-aligned with tabular figures.
    Number,
    Faint,
    /// A coverage check: a flag is a mark, filled when covered, a ring for a gap; any other
    /// value is a check that does not apply.
    Mark,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Col {
    /// The key of the cell this column draws.
    pub field: &'static str,
    /// The header label, drawn uppercase.
    pub title: &'static str,
    pub width: Width,
    pub text: Text,
}

/// The height of one row: a table row or a line of log or code (record 1.4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowHeight {
    Row,
    Line,
}

/// What an opened row shows under itself (record 5.2.1).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Detail {
    /// Label and value pairs, flowed `facts_per_line` to a line.
    pub facts: Vec<(String, Value)>,
    /// A block of raw text under the facts.
    pub text: Option<String>,
    /// Buttons, drawn in this order; each runs on the row as its target.
    pub commands: Vec<CommandId>,
    /// Buttons aimed at something other than the row: the thing each runs on.
    pub aims: Vec<(CommandId, Address)>,
    /// Buttons that say words of their own, such as `Only docker:db`.
    pub titles: Vec<(CommandId, String)>,
    /// Buttons that show a key their command does not bind, written as a command's key is.
    pub hints: Vec<(CommandId, &'static str)>,
    /// A faint sentence after the buttons.
    pub note: Option<String>,
    /// Which button is the main one, drawn filled.
    pub main: Main,
    /// Buttons drawn quiet, as ghosts: no fill and no ring.
    pub quiet: Vec<CommandId>,
    /// The label column of its facts: wider where a label would be cut.
    pub label: LabelWidth,
}

/// The width of a detail's label column in points; record 5.2.1 gives 56.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LabelWidth(pub f32);

impl Default for LabelWidth {
    fn default() -> Self {
        LabelWidth(56.0)
    }
}

/// The main button of a detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Main {
    /// The first button, as most details have it.
    #[default]
    First,
    Command(CommandId),
    /// No button is the main one.
    Neither,
}

/// Everything the list engine needs to draw one list.
#[derive(Debug, Clone, Copy)]
pub struct ListDesc {
    /// What a row is: its address, its right-click items.
    pub kind: Kind,
    pub columns: &'static [Col],
    /// Rows name parents; a child indents one nesting step.
    pub tree: bool,
    pub height: RowHeight,
    /// 3 for the key and value grid, 1 for Agents and the Findings list form.
    pub facts_per_line: u8,
    pub detail: fn(&Row) -> Detail,
    /// The plain sentence a pane ends with, stating a rule.
    pub footer: Option<&'static str>,
}
