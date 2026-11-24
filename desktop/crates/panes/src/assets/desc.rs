//! The Assets list: columns and what an opened asset shows.

use yd_core::address::attach::ATTACH;
use yd_core::command::table::{Command, Route};
use yd_core::feed::ASSETS;
use yd_core::ids::CommandId;
use yd_core::kind::Kind;
use yd_core::list::{Col, Detail, ListDesc, RowHeight, Text, Width};
use yd_core::row::{Row, Value};

pub const RESCAN: CommandId = CommandId("asset.rescan");
pub const OPEN_SHELL: CommandId = CommandId("asset.open-a-shell");
pub const SHOW_FINDINGS: CommandId = CommandId("asset.show-findings");
pub const CLOSE_GAPS: CommandId = CommandId("asset.close-the-gaps");
pub const CLOSE_GAP: CommandId = CommandId("asset.close-the-gap");

const fn button(id: CommandId, title: &'static str) -> Command {
    Command {
        id,
        title,
        key: None,
        menu: None,
        kinds: &[Kind::Asset],
        route: Route::Source(ASSETS),
    }
}

/// The buttons of an opened asset, with the words its row's `actions` cell names them by.
pub const COMMANDS: &[Command] = &[
    button(RESCAN, "Rescan"),
    button(OPEN_SHELL, "Open a shell"),
    button(SHOW_FINDINGS, "Show findings"),
    button(CLOSE_GAPS, "Close the gaps"),
    button(CLOSE_GAP, "Close the gap"),
];

const fn col(field: &'static str, title: &'static str, width: Width, text: Text) -> Col {
    Col {
        field,
        title,
        width,
        text,
    }
}

pub const DESC: ListDesc = ListDesc {
    kind: Kind::Asset,
    columns: &[
        col("state", "", Width::Px(12.0), Text::Plain),
        col("asset", "Asset", Width::Px(92.0), Text::Name),
        col("about", "", Width::Fr(1.0), Text::Faint),
        col("reach", "Reach", Width::Px(38.0), Text::Mark),
        col("logs", "Logs", Width::Px(38.0), Text::Mark),
        col("backup", "Backup", Width::Px(44.0), Text::Mark),
        col("scan", "Scan", Width::Px(38.0), Text::Mark),
        col("seen", "Seen", Width::Px(46.0), Text::Number),
    ],
    tree: false,
    height: RowHeight::Row,
    facts_per_line: 3,
    detail,
    footer: None,
};

/// The facts of an opened asset: each check in words, then what the gaps came to.
const FACTS: [(&str, &str); 6] = [
    ("What", "about"),
    ("Reach", "reach_detail"),
    ("Logs", "logs_detail"),
    ("Backup", "backup_detail"),
    ("Scan", "scan_detail"),
    ("Findings", "findings"),
];

fn command_of(title: &str) -> Option<CommandId> {
    COMMANDS
        .iter()
        .find(|command| command.title == title)
        .map(|command| command.id)
}

/// The facts, then the row's buttons as its `actions` names them, then Attach, which every row ends with.
/// Each button keeps the words its row gives it: a global row may title the same command differently.
fn detail(row: &Row) -> Detail {
    let fact = |(label, field): &(&str, &str)| Some(((*label).to_owned(), row.get(field)?.clone()));
    let names: &[String] = match row.get("actions") {
        Some(Value::Tags(names)) => names,
        _ => &[],
    };
    let buttons: Vec<(CommandId, String)> = names
        .iter()
        .filter_map(|name| Some((command_of(name)?, name.clone())))
        .chain([(ATTACH, "Attach".to_owned())])
        .collect();
    Detail {
        facts: FACTS.iter().filter_map(fact).collect(),
        commands: buttons.iter().map(|(id, _)| *id).collect(),
        titles: buttons,
        note: row.words("note").map(str::to_owned),
        ..Detail::default()
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use yd_core::row::rows_from_json;

    use super::*;

    const ROWS: &str = include_str!("../../../../fixtures/mockup/assets.rows.json");

    fn open(id: &str) -> Result<Detail, Box<dyn Error>> {
        let rows = rows_from_json(ROWS)?;
        let row = rows
            .iter()
            .find(|row| row.id.as_str() == id)
            .ok_or("no such asset")?;
        Ok(detail(row))
    }

    #[test]
    fn a_gap_asset_offers_closing_the_gaps_before_the_rest() -> Result<(), Box<dyn Error>> {
        let shown = open("build-01")?;
        assert_eq!(
            shown.commands,
            [CLOSE_GAPS, OPEN_SHELL, SHOW_FINDINGS, ATTACH]
        );
        let wan = open("wan")?;
        assert_eq!(wan.commands, [CLOSE_GAP, SHOW_FINDINGS, ATTACH]);
        Ok(())
    }

    #[test]
    fn the_open_asset_shows_each_check_in_words() -> Result<(), Box<dyn Error>> {
        let shown = open("edge-01")?;
        let labels: Vec<&str> = shown
            .facts
            .iter()
            .map(|(label, _)| label.as_str())
            .collect();
        assert_eq!(
            labels,
            ["What", "Reach", "Logs", "Backup", "Scan", "Findings"]
        );
        assert_eq!(shown.facts[2].1, Value::Text("not coming in".to_owned()));
        assert_eq!(shown.note.as_deref(), Some("filed as a finding"));
        Ok(())
    }
}
