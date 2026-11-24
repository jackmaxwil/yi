//! Daemons pane: this Mac's background services, as a list over the `daemons` feed.

use std::borrow::Cow;

use rune::{Cx, El, col, text};
use yd_core::feed::{DAEMONS, FeedEvent, FeedStatus, Table};
use yd_core::ids::{FeedId, SettingKey};
use yd_core::inbox::{Closed, FeedTx, inbox};
use yd_core::model::Model;
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::row::{Dot, Row, Rows, Value, rows_from_json};
use yd_core::settings::schema::{Control, Setting, SettingValue, Store};
use yd_ui::chrome::tabs::{PLUS, Tab, Tabs};
use yd_ui::kit::bars::{footer, header, lead};
use yd_ui::kit::chip::{Chip, chip};
use yd_ui::list::view::worded;
use yd_ui::overlay::pick::Finder;
use yd_ui::pane::{PaneCx, PaneDef, Scene, Stage};
use yd_ui::theme::look::{LABEL, META, colour};
use yd_ui::win::{Hub, Win};

use super::desc::DESC;
use super::feed::{COMMANDS, ask_bar};

use super::states::WORDS;

/// The feed this pane lists; its source declares the same name.
pub const FEED: FeedId = DAEMONS;

const ROWS: &str = include_str!("../../../../fixtures/mockup/daemons.rows.json");

/// The setting that names services to list beside Yi's own: launchd labels, one a line.
pub const LABELS: &str = "daemons.labels";
const LABELS_ROW: Setting = Setting {
    key: SettingKey::fixed(LABELS),
    group: Cow::Borrowed("Daemons"),
    name: Cow::Borrowed("Other services to list"),
    description: Cow::Borrowed("Launchd labels to show beside Yi's own, one a line."),
    control: Control::Text,
    store: Store::App,
    default: Some(SettingValue::Text(Cow::Borrowed(""))),
    value: None,
};
/// The most labels the setting sends its source, and the longest one.
const MAX_LABELS: usize = 64;
const MAX_LABEL: usize = 200;

/// The labels the setting names, as the launchd source is sent them.
pub fn labels(model: &Model) -> Vec<String> {
    let Some(SettingValue::Text(text)) = model.settings.value(LABELS) else {
        return Vec::new();
    };
    let lines = text.lines().map(str::trim);
    let fit = lines.filter(|label| !label.is_empty() && label.chars().count() <= MAX_LABEL);
    fit.take(MAX_LABELS).map(str::to_owned).collect()
}

pub const DEF: PaneDef = PaneDef {
    kind: PaneKind::Daemons,
    view,
    scenes: &[
        Scene {
            name: "daemons/stub",
            stage: Stage::Pane(PaneKind::Daemons),
            setup: |_| {},
        },
        Scene {
            name: "daemons/sample",
            stage: Stage::Pane(PaneKind::Daemons),
            setup: sample,
        },
        Scene {
            name: "daemons/mockup",
            stage: Stage::Pane(PaneKind::Daemons),
            setup: mockup,
        },
    ],
    commands: COMMANDS,
    settings: &[LABELS_ROW],
    tabs: Some(tabs),
    on_tab: None,
    finder: Some(Finder::Rows(&DESC)),
};

/// Frame 7's services from the fixture, the way the source would send them, with mcp-figma open.
fn mockup(hub: &mut Hub) {
    let event = match rows_from_json(ROWS) {
        Ok(rows) => FeedEvent::Reset(rows),
        Err(error) => FeedEvent::Status(FeedStatus::Error(error.to_string())),
    };
    step(&mut hub.model, Msg::Feed(FEED, event));
    if let Some(lists) = hub
        .panes
        .get_mut::<yd_ui::list::state::Lists>(Some(yd_ui::pane::SCENE_PANE))
    {
        lists
            .list_mut("")
            .set_open(Some(yd_core::ids::RowId::new("mcp-figma")));
    }
}

/// The services: every row but the log lines hung under one.
fn each(table: Option<&Table>) -> impl Iterator<Item = &Row> {
    let rows = table
        .into_iter()
        .flat_map(|table| (0..table.len()).filter_map(move |at| table.get(at)));
    rows.filter(|row| row.parent.is_none())
}

fn count(table: Option<&Table>, state: Dot) -> usize {
    each(table)
        .filter(|row| matches!(row.get("state"), Some(Value::Dot(dot)) if *dot == state))
        .count()
}

fn tabs(pane: &PaneCx<'_>) -> Tabs {
    let total = each(pane.model().tables.get(FEED)).count();
    let tab = Tab {
        count: u32::try_from(total).ok().filter(|total| *total > 0),
        meta: format!("this Mac · {total}"),
        ..Tab::of(PaneKind::Daemons)
    };
    Tabs {
        tabs: vec![tab],
        ..Tabs::default()
    }
}

/// The header bar: the machine, how many run, how many failed and stopped, and the button that adds one.
fn headline(pane: &PaneCx<'_>, key: &str, table: Option<&Table>) -> El {
    let tokens = pane.tokens();
    let left = vec![
        chip(tokens, Chip::Plain, "This Mac", None, true),
        lead(tokens, &format!("{} running", count(table, Dot::Run))),
        text(format!("{} failed", count(table, Dot::Fail)))
            .font(LABEL)
            .color(colour(tokens.bad)),
        text(format!("· {} stopped", count(table, Dot::Stop)))
            .font(LABEL)
            .color(colour(tokens.dim)),
    ];
    let right = vec![chip(tokens, Chip::Plain, "Add", Some(&PLUS), false)];
    header(tokens, left, right).key(format!("{key}.pane-header"))
}

fn base(pane: &PaneCx<'_>, key: &str) -> El {
    let tokens = pane.tokens();
    let legend = vec![
        text("Login means it starts when you log in to this Mac")
            .font(META)
            .color(colour(tokens.faint)),
    ];
    footer(tokens, legend, Vec::new()).key(format!("{key}.footer"))
}

fn view(pane: &PaneCx<'_>, _cx: &mut Cx<Win>) -> El {
    let key = pane.key(PaneKind::Daemons);
    let table = pane.model().tables.get(FEED);
    let tokens = pane.tokens();
    col()
        .key(key.clone())
        .w_full()
        .h_full()
        .fill(colour(tokens.pane_tint.alpha(tokens.glass_pane)))
        .child(headline(pane, &key, table))
        .children(ask_bar(pane))
        .child(worded(
            pane,
            PaneKind::Daemons,
            "",
            &DESC,
            table,
            &WORDS,
            false,
        ))
        .child(base(pane, &key))
}

/// A stand-in for the launchd source: three services, sent as a source thread would.
fn fake(out: &FeedTx) -> Result<(), Closed> {
    let service = |id: &str, state: Dot| {
        Row::new(id)
            .text("name", id)
            .cell("state", Value::Dot(state))
    };
    out.send(FeedEvent::Reset(vec![
        service("com.yi.daemon", Dot::Run),
        service("com.docker.vmnetd", Dot::Run),
        service("homebrew.mxcl.postgresql@16", Dot::Fail),
    ]))
}

/// The keystone's thin slice: a feed, through the inbox and `step`, into the table drawn.
fn sample(hub: &mut Hub) {
    let (sender, inbox) = inbox(|| {});
    if fake(&sender.feed(FEED)).is_ok() {
        inbox.drain(&mut hub.model);
    }
}
