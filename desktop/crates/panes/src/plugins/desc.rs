//! The Plugins list: rows and what an opened plugin shows.

use yd_core::address::attach::ATTACH;
use yd_core::ids::CommandId;
use yd_core::kind::Kind;
use yd_core::list::{Col, Detail, ListDesc, Main, RowHeight, Text, Width};
use yd_core::row::{Row, Value};

/// Reload and Open source are also right-click rows, so `yd-core` declares them.
pub use yd_core::command::tools::plugin::{OPEN_SOURCE, RELOAD};
pub const REMOVE: CommandId = CommandId("plugins.remove");

pub const DESC: ListDesc = ListDesc {
    kind: Kind::Plugin,
    columns: &[
        Col {
            field: "name",
            title: "Plugin",
            width: Width::Fr(0.8),
            text: Text::Name,
        },
        Col {
            field: "version",
            title: "Version",
            width: Width::Px(84.0),
            text: Text::Faint,
        },
        Col {
            field: "adds",
            title: "Adds",
            width: Width::Fr(1.2),
            text: Text::Plain,
        },
    ],
    tree: false,
    height: RowHeight::Row,
    facts_per_line: 1,
    detail,
    footer: Some("Plugin panes run in a sandbox inside the app: a broken one shows its error"),
};

/// What it adds, what it may reach, its version and last error; Remove asks once first.
fn detail(row: &Row) -> Detail {
    let tags = |field: &str| match row.get(field) {
        Some(Value::Tags(tags)) if !tags.is_empty() => Some(Value::Text(tags.join(" · "))),
        _ => None,
    };
    let text = |field: &str| {
        row.words(field)
            .filter(|w| !w.is_empty())
            .map(str::to_owned)
    };
    let reach = tags("reach").or_else(|| tags("allowed"));
    let facts = [
        ("Adds", tags("adds")),
        ("May reach", reach.or(Some(Value::Text("nothing".into())))),
        ("May not", tags("denied")),
        (
            "Version",
            text("version").map(|v| {
                Value::Text(
                    [Some(v), text("took")]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" · "),
                )
            }),
        ),
        (
            "Last error",
            Some(Value::Text(text("error").unwrap_or_else(|| "none".into()))),
        ),
    ];
    let about = [text("description"), text("source")];
    let confirm = matches!(row.get("confirm"), Some(Value::Flag(true)));
    let name = text("name").unwrap_or_default();
    Detail {
        facts: facts
            .into_iter()
            .filter_map(|(label, value)| Some((label.to_owned(), value?)))
            .collect(),
        text: Some(about.into_iter().flatten().collect::<Vec<_>>().join(" ")),
        commands: vec![RELOAD, OPEN_SOURCE, ATTACH, REMOVE],
        titles: confirm
            .then(|| (REMOVE, "Move to Trash".to_owned()))
            .into_iter()
            .chain([(ATTACH, "Attach".to_owned())])
            .collect(),
        note: confirm.then(|| format!("Press Move to Trash to move {name}'s folder to the Trash.")),
        main: Main::First,
        ..Detail::default()
    }
}
