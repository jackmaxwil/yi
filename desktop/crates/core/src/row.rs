//! One row of a feed's table, its typed cell values, and the trait a list reads rows through.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ids::RowId;

/// The state a status dot shows (record 1.1.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dot {
    Need,
    Ask,
    Work,
    Done,
    Idle,
    Run,
    Stop,
    Fail,
    Pause,
}

/// A colour role: what a colour means, never which colour it is (record 1.1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    Fg,
    Dim,
    Faint,
    Accent,
    Need,
    Ok,
    Bad,
    Warn,
    Model,
    Context,
}

/// What one cell holds. The variant picks the drawing; a column picks the text style.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Value {
    Text(String),
    Dot(Dot),
    /// A severity badge or a tag with a meaning: its words and its role.
    Badge(String, Tone),
    /// A fully round state pill.
    Pill(String, Tone),
    /// A meter filled to `ratio` (0 to 1) with its reading beside it.
    Meter {
        ratio: f32,
        text: String,
    },
    Tags(Vec<String>),
    /// A bar inside a time track: where it starts and how long it is, as shares of the track.
    Bar {
        start: f32,
        len: f32,
        tone: Tone,
    },
    Flag(bool),
    Series(Vec<f32>),
}

/// One thing in a list. Cells are keyed by the field names the list's columns name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub id: RowId,
    /// The row this one hangs under in a tree list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<RowId>,
    /// The group heading this row sits under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// A stopped or exited thing: drawn at half strength.
    #[serde(default)]
    pub dim: bool,
    /// A role for the whole row, such as a log line's level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<Tone>,
    #[serde(default)]
    pub cells: BTreeMap<String, Value>,
}

impl Row {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: RowId::new(id),
            parent: None,
            group: None,
            dim: false,
            tone: None,
            cells: BTreeMap::new(),
        }
    }

    pub fn cell(mut self, field: &str, value: Value) -> Self {
        self.cells.insert(field.to_owned(), value);
        self
    }

    pub fn text(self, field: &str, text: impl Into<String>) -> Self {
        self.cell(field, Value::Text(text.into()))
    }

    pub fn get(&self, field: &str) -> Option<&Value> {
        self.cells.get(field)
    }

    /// The words of a cell, whatever its drawing; `None` for a cell with no words.
    pub fn words(&self, field: &str) -> Option<&str> {
        match self.get(field)? {
            Value::Text(text) | Value::Badge(text, _) | Value::Pill(text, _) => Some(text),
            Value::Meter { text, .. } => Some(text),
            Value::Dot(_)
            | Value::Tags(_)
            | Value::Bar { .. }
            | Value::Flag(_)
            | Value::Series(_) => None,
        }
    }
}

/// What a list reads: a feed's table, or a filtered, sorted or grouped view over one.
pub trait Rows {
    /// Changes whenever a row does; a list passes it to `virtual_rows_version`.
    fn version(&self) -> u64;
    fn len(&self) -> usize;
    fn get(&self, index: usize) -> Option<&Row>;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// JSON that does not have the shape a fixture or a file should have.
#[derive(Debug, thiserror::Error)]
#[error("the JSON does not have the expected shape")]
pub struct ShapeError(#[source] serde_json::Error);

impl From<serde_json::Error> for ShapeError {
    fn from(cause: serde_json::Error) -> Self {
        Self(cause)
    }
}

/// The rows of a fixture: a JSON array of rows as this crate writes them.
pub fn rows_from_json(text: &str) -> Result<Vec<Row>, ShapeError> {
    Ok(serde_json::from_str(text)?)
}
