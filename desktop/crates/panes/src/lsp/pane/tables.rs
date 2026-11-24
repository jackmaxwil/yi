//! The two lists' rows, cut from the `lsp` feed by id prefix and joined with their fixes.

use yd_core::feed::{FeedEvent, LSP, Table};
use yd_core::model::Model;
use yd_core::row::{Row, Rows};

fn part(model: &Model, prefix: &str) -> Table {
    model
        .tables
        .get(LSP)
        .map_or_else(Table::default, |table| table.prefixed(prefix))
}

/// The language servers the feed has rows for.
pub(super) fn servers(model: &Model) -> Table {
    part(model, "server:")
}

fn number(row: &Row, field: &str) -> Option<usize> {
    row.words(field)?.parse().ok()
}

/// A problem with the fix the server offered for its line: the fix's title when the row
/// says no fix of its own, and the fix row's id for the quick-fix button to apply.
fn joined(model: &Model, mut row: Row) -> Row {
    let (Some(path), Some(line)) = (row.words("path"), number(&row, "line")) else {
        return row;
    };
    let wanted = format!("fix:{path}:{line}:");
    let Some(table) = model.tables.get(LSP) else {
        return row;
    };
    let fixes = (0..table.len()).filter_map(|at| table.get(at));
    let Some(fix) = fixes
        .filter(|fix| fix.id.as_str().starts_with(&wanted))
        .min_by_key(|fix| &fix.id)
    else {
        return row;
    };
    let (id, title) = (
        fix.id.as_str().to_owned(),
        fix.words("title").map(str::to_owned),
    );
    if row.words("fix").is_none()
        && let Some(title) = title
    {
        row = row.text("fix", title);
    }
    row.text("fix_row", id)
}

/// The problems the feed has rows for. Grouped, they sit under the name of their file in
/// file and line order; otherwise they stay in the order they came.
pub(super) fn problems(model: &Model, grouped: bool) -> Table {
    let found = part(model, "problem:");
    let mut rows: Vec<Row> = (0..found.len())
        .filter_map(|at| found.get(at))
        .map(|row| joined(model, row.clone()))
        .collect();
    if grouped {
        for row in &mut rows {
            row.group = row.words("path").map(str::to_owned);
        }
        rows.sort_by_key(|row| (row.group.clone(), number(row, "line")));
    }
    let mut table = Table::default();
    table.apply(FeedEvent::Reset(rows));
    table.apply(FeedEvent::Status(found.status().clone()));
    table
}
