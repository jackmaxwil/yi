//! How the debugger's rows are written: the console, breakpoints, and the text in cells.

use std::collections::VecDeque;
use std::path::Path;

use yd_core::feed::FeedEvent;
use yd_core::ids::RowId;
use yd_core::inbox::{Closed, FeedTx};
use yd_core::row::{Row, Value as Cell};

use super::super::Locate;
use super::Mark;

/// Console lines kept; past it the oldest row goes.
pub(super) const MAX_OUT: usize = 500;

/// Characters a cell keeps of an adapter's text.
pub(super) const MAX_CELL: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Role {
    Input,
    Output,
    Error,
}

impl Role {
    fn word(self) -> &'static str {
        match self {
            Role::Input => "input",
            Role::Output => "output",
            Role::Error => "error",
        }
    }
}

/// The newest console lines as `out:<n>` rows.
#[derive(Debug, Default)]
pub(super) struct Console {
    rows: VecDeque<u64>,
    next: u64,
}

impl Console {
    pub(super) fn push(&mut self, out: &FeedTx, text: &str, role: Role) -> Result<(), Closed> {
        self.next = self.next.saturating_add(1);
        let row = Row::new(format!("out:{}", self.next))
            .text("text", clip(text))
            .text("role", role.word());
        out.send(FeedEvent::Upsert(row))?;
        self.rows.push_back(self.next);
        while self.rows.len() > MAX_OUT {
            if let Some(old) = self.rows.pop_front() {
                out.send(FeedEvent::Remove(RowId::new(format!("out:{old}"))))?;
            }
        }
        Ok(())
    }
}

pub(super) fn mark_row(mark: &Mark) -> Row {
    let hits = if mark.hits > 0 {
        format!("hit {}", mark.hits)
    } else {
        String::new()
    };
    Row::new(format!("bp:{}", mark.n))
        .cell("enabled", Cell::Flag(true))
        .text("where", format!("{}:{}", name_of(&mark.path), mark.line))
        .text("condition", "")
        .text("hits", hits)
        .text("added", "by you")
        .text("path", &mark.path)
        .text("line", mark.line.to_string())
}

/// The line of a `lines` address part: `L18`, `L18:C4` or `L18-L20`.
pub(super) fn line_of(part: &str) -> Option<u64> {
    let digits: String = part
        .strip_prefix('L')?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok().filter(|l| *l > 0)
}

pub(super) fn name_of(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
        .to_owned()
}

pub(super) fn looked(at: &Locate<'_>) -> String {
    match at {
        Locate::Xcrun(name) => format!("{name} (xcrun -f, PATH)"),
        Locate::Path(name) => format!("{name} on PATH"),
        Locate::At(path) => (*path).to_owned(),
    }
}

pub(super) fn clip(text: &str) -> String {
    text.chars().take(MAX_CELL).collect()
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use yd_core::inbox::inbox;
    use yd_core::model::Model;
    use yd_core::row::Rows;

    use super::super::super::FEED;
    use super::*;

    #[test]
    fn the_console_keeps_its_newest_lines() -> Result<(), Box<dyn Error>> {
        let (sender, inbox) = inbox(|| {});
        let out = sender.feed(FEED);
        let mut console = Console::default();
        let mut model = Model::default();
        for n in 0..=MAX_OUT {
            console.push(&out, &format!("line {n}"), Role::Output)?;
            inbox.drain(&mut model);
        }
        let table = model.tables.get(FEED).ok_or("no table")?;
        assert_eq!(table.len(), MAX_OUT);
        assert!(table.find(&RowId::new("out:1")).is_none());
        let newest = table.find(&RowId::new(format!("out:{}", MAX_OUT + 1)));
        assert_eq!(
            newest.and_then(|r| r.words("text")),
            Some(format!("line {MAX_OUT}").as_str())
        );
        Ok(())
    }
}
