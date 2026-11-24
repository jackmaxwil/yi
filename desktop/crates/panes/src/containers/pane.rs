//! Containers pane: Docker containers, as a list over the `containers` feed.

use rune::{Cx, El, Tx, col, row, text};
use yd_core::feed::{CONTAINERS, FeedEvent, FeedStatus, Table};
use yd_core::ids::{FeedId, RowId};
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::row::{Dot, Row, Rows, Value, rows_from_json};
use yd_ui::chrome::tabs::{Tab, Tabs};
use yd_ui::kit::bars::{header, lead};
use yd_ui::kit::chip::{Chip, chip};
use yd_ui::list::state::Lists;
use yd_ui::list::view::worded;
use yd_ui::overlay::pick::Finder;
use yd_ui::pane::{PaneCx, PaneDef, SCENE_PANE, Scene, Stage};
use yd_ui::theme::look::{LABEL, META, colour};
use yd_ui::win::{Hub, Win};

use super::desc::DESC;
use super::feed::question;

use super::states::WORDS;

/// The feed this pane lists; its source declares the same name.
pub const FEED: FeedId = CONTAINERS;

const ROWS: &str = include_str!("../../../../fixtures/mockup/containers.rows.json");

pub const DEF: PaneDef = PaneDef {
    kind: PaneKind::Containers,
    view,
    scenes: &[
        Scene {
            name: "containers/stub",
            stage: Stage::Pane(PaneKind::Containers),
            setup: |_| {},
        },
        Scene {
            name: "containers/mockup",
            stage: Stage::Pane(PaneKind::Containers),
            setup: mockup,
        },
    ],
    commands: &[],
    settings: &[],
    tabs: Some(tabs),
    on_tab: None,
    finder: Some(Finder::Rows(&DESC)),
};

/// Frame 7's containers from the fixture, the way the source would send them, with atlas-api open.
fn mockup(hub: &mut Hub) {
    let event = match rows_from_json(ROWS) {
        Ok(rows) => FeedEvent::Reset(rows),
        Err(error) => FeedEvent::Status(FeedStatus::Error(error.to_string())),
    };
    step(&mut hub.model, Msg::Feed(FEED, event));
    if let Some(lists) = hub.panes.get_mut::<Lists>(Some(SCENE_PANE)) {
        lists.list_mut("").set_open(Some(RowId::new("atlas-api")));
    }
}

/// What the header's chips said when pressed: the host picker and the two other views are
/// not built.
#[derive(Debug, Default)]
pub struct Said(pub Option<String>);

/// The containers alone, each carrying the lines its `log:` rows hold as `output`: a log row
/// is a line of its container's output, never a container.
pub fn listed(table: &Table) -> Table {
    let line = |row: &Row| row.id.as_str().starts_with("log:");
    let output = |of: &RowId| {
        let lines = each(Some(table)).filter(|row| line(row) && row.parent.as_ref() == Some(of));
        let lines: Vec<&str> = lines.filter_map(|row| row.words("last_line")).collect();
        (!lines.is_empty()).then(|| lines.join("\n"))
    };
    table.kept(|row| match (line(row), output(&row.id)) {
        (true, _) => None,
        (false, Some(lines)) => Some(row.clone().text("output", lines)),
        (false, None) => Some(row.clone()),
    })
}

fn each(table: Option<&Table>) -> impl Iterator<Item = &Row> {
    table
        .into_iter()
        .flat_map(|table| (0..table.len()).filter_map(move |at| table.get(at)))
}

/// How many containers are in one state: a running one is `run`, an exited one `fail`.
fn count(table: Option<&Table>, state: Dot) -> usize {
    each(table)
        .filter(|row| matches!(row.get("state"), Some(Value::Dot(dot)) if *dot == state))
        .count()
}

fn tabs(pane: &PaneCx<'_>) -> Tabs {
    let table = pane.model().tables.get(FEED).map(listed);
    let total = each(table.as_ref()).count();
    let tab = Tab {
        count: u32::try_from(total).ok().filter(|total| *total > 0),
        ..Tab::of(PaneKind::Containers)
    };
    Tabs {
        tabs: vec![tab],
        ..Tabs::default()
    }
}

/// A chip of the header that nothing is behind yet: pressed, the pane says `what` is not
/// available under the bar.
fn unbuilt(pane: &PaneCx<'_>, key: &str, name: &str, words: &str, what: &'static str) -> El {
    let id = pane.pane;
    chip(pane.tokens(), Chip::Plain, words, None, name == "host")
        .key(format!("{key}.{name}"))
        .on_press_tx(move |win: &mut Win, _: &mut Tx| {
            if let Ok(mut hub) = win.hub.try_borrow_mut()
                && let Some(said) = hub.panes.get_mut::<Said>(Some(id))
            {
                said.0 = Some(format!("{what} is not available yet"));
            }
        })
}

/// The header bar: the engine, how many run and exited, and the two views beside them.
/// Incident: it named the mockup's host over an empty list; Docker's rows name no host.
fn headline(pane: &PaneCx<'_>, key: &str, table: Option<&Table>) -> El {
    let tokens = pane.tokens();
    let left = vec![
        unbuilt(pane, key, "host", "docker", "Choosing a Docker host"),
        lead(tokens, &format!("{} running", count(table, Dot::Run))),
        text(format!("{} exited", count(table, Dot::Fail)))
            .font(LABEL)
            .color(colour(tokens.dim)),
    ];
    let right = vec![
        unbuilt(pane, key, "images", "Images", "Images"),
        unbuilt(pane, key, "volumes", "Volumes", "Volumes"),
    ];
    header(tokens, left, right).key(format!("{key}.pane-header"))
}

/// The line between the bar and the list: the question a stop or a remove waits on, else
/// what a chip last said.
fn under_bar(pane: &PaneCx<'_>, key: &str) -> Option<El> {
    let tokens = pane.tokens();
    let said = pane.state::<Said>().and_then(|said| said.0.as_deref());
    question(pane.hub, tokens).or_else(|| {
        let line = text(said?).font(META).color(colour(tokens.faint));
        Some(row().key(format!("{key}.said")).px(8.0).py(4.0).child(line))
    })
}

fn view(pane: &PaneCx<'_>, _cx: &mut Cx<Win>) -> El {
    let key = pane.key(PaneKind::Containers);
    let table = pane.model().tables.get(FEED).map(listed);
    let table = table.as_ref();
    let tokens = pane.tokens();
    col()
        .key(key.clone())
        .w_full()
        .h_full()
        .fill(colour(tokens.pane_tint.alpha(tokens.glass_pane)))
        .child(headline(pane, &key, table))
        .children(under_bar(pane, &key))
        .child(worded(
            pane,
            PaneKind::Containers,
            "",
            &DESC,
            table,
            &WORDS,
            false,
        ))
}
