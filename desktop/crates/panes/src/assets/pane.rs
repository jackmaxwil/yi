//! Assets pane: what is covered and what is not, as a list over the `assets` feed.

use rune::{Cx, El, Tx, col, el, row, text};
use yd_core::command::table::Target;
use yd_core::feed::{ASSETS, FeedEvent, FeedStatus, Table};
use yd_core::ids::{CommandId, FeedId, RowId};
use yd_core::kind::Kind;
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::row::{Row, Rows, Value, rows_from_json};
use yd_core::tokens::Tokens;
use yd_ui::chrome::tabs::{Tab, Tabs};
use yd_ui::grid_view::view::show_pane;
use yd_ui::kit::bars::{footer, header, lead};
use yd_ui::kit::chip::{Chip, chip};
use yd_ui::list::state::Lists;
use yd_ui::list::view::worded;
use yd_ui::overlay::pick::Finder;
use yd_ui::pane::{PaneCx, PaneDef, SCENE_PANE, Scene, Stage};
use yd_ui::theme::look::{LABEL, META, colour};
use yd_ui::win::{Hub, Win};

use super::desc::{COMMANDS, DESC, OPEN_SHELL, SHOW_FINDINGS};

use super::states::WORDS;

/// The feed this pane lists; its source declares the same name.
pub const FEED: FeedId = ASSETS;

const ROWS: &str = include_str!("../../../../fixtures/mockup/assets.rows.json");

pub const DEF: PaneDef = PaneDef {
    kind: PaneKind::Assets,
    view,
    scenes: &[
        Scene {
            name: "assets/stub",
            stage: Stage::Pane(PaneKind::Assets),
            setup: |_| {},
        },
        Scene {
            name: "assets/mockup",
            stage: Stage::Pane(PaneKind::Assets),
            setup: mockup,
        },
    ],
    commands: COMMANDS,
    settings: &[],
    tabs: Some(tabs),
    on_tab: None,
    finder: Some(Finder::Rows(&DESC)),
};

/// Frame 7's assets from the fixture, the way the source would send them, with build-01 open.
fn mockup(hub: &mut Hub) {
    let event = match rows_from_json(ROWS) {
        Ok(rows) => FeedEvent::Reset(rows),
        Err(error) => FeedEvent::Status(FeedStatus::Error(error.to_string())),
    };
    step(&mut hub.model, Msg::Feed(FEED, event));
    if let Some(lists) = hub.panes.get_mut::<Lists>(Some(SCENE_PANE)) {
        lists.list_mut("").set_open(Some(RowId::new("build-01")));
    }
}

/// The four checks a row carries. A gap is a flag that is off; a check that does not apply is text.
const CHECKS: [&str; 4] = ["reach", "logs", "backup", "scan"];

fn gaps(row: &Row) -> usize {
    CHECKS
        .iter()
        .filter(|field| matches!(row.get(field), Some(Value::Flag(false))))
        .count()
}

fn each(table: Option<&Table>) -> impl Iterator<Item = &Row> {
    table
        .into_iter()
        .flat_map(|table| (0..table.len()).filter_map(move |at| table.get(at)))
}

fn tabs(pane: &PaneCx<'_>) -> Tabs {
    let count = each(pane.model().tables.get(FEED)).count();
    let tab = Tab {
        count: u32::try_from(count).ok().filter(|count| *count > 0),
        ..Tab::of(PaneKind::Assets)
    };
    Tabs {
        tabs: vec![tab],
        ..Tabs::default()
    }
}

/// The header bar: how many assets, how many have no gap, and the gaps all of them come to.
fn headline(pane: &PaneCx<'_>, key: &str, table: Option<&Table>) -> El {
    let tokens = pane.tokens();
    let total = each(table).count();
    let covered = each(table).filter(|row| gaps(row) == 0).count();
    let open: usize = each(table).map(gaps).sum();
    let left = vec![
        lead(tokens, &format!("{total} assets")),
        text(format!("{covered} fully covered · "))
            .font(LABEL)
            .color(colour(tokens.dim)),
        text(format!("{open} gaps"))
            .font(LABEL)
            .color(colour(tokens.bad)),
    ];
    let right = vec![
        chip(tokens, Chip::Plain, "Gaps only", None, false),
        chip(tokens, Chip::Plain, "Rescan", None, false),
    ];
    header(tokens, left, right).key(format!("{key}.pane-header"))
}

/// One mark of the footer's legend: filled when covered, a red ring for a gap, faint when it does not apply.
fn mark(tokens: &Tokens, covered: bool, gap: bool) -> El {
    let dot = el().size(8.0, 8.0).radius(4.0);
    match (covered, gap) {
        (true, _) => dot.fill(colour(tokens.ok)),
        (false, true) => dot.border(1.5, colour(tokens.bad)),
        (false, false) => dot.fill(colour(yd_ui::theme::scale::DOT_UNCOVERED.of(tokens))),
    }
}

fn legend_item(tokens: &Tokens, covered: bool, gap: bool, words: &str) -> El {
    row()
        .gap(4.0)
        .center_y()
        .child(mark(tokens, covered, gap))
        .child(text(words).font(META).color(colour(tokens.faint)))
}

fn base(pane: &PaneCx<'_>, key: &str) -> El {
    let tokens = pane.tokens();
    let legend = vec![
        legend_item(tokens, true, false, "covered"),
        legend_item(tokens, false, true, "gap"),
        legend_item(tokens, false, false, "does not apply"),
    ];
    let note = text("Reach means an agent can get in")
        .font(META)
        .color(colour(tokens.faint));
    footer(tokens, legend, vec![note]).key(format!("{key}.footer"))
}

/// The two buttons of an opened asset that name a pane: its findings show the Findings pane,
/// a shell the Terminal. Invariant: nothing is typed there, since how an asset is reached is unset.
pub fn carry(win: &mut Win, tx: &mut Tx, command: CommandId, target: &Target) -> bool {
    let aimed = matches!(target, Target::Thing(address) if address.kind() == Kind::Asset);
    let shown = match command {
        SHOW_FINDINGS => PaneKind::Findings,
        OPEN_SHELL => PaneKind::Terminal,
        _ => return false,
    };
    aimed && show_pane(win, tx, shown).is_some()
}

fn view(pane: &PaneCx<'_>, _cx: &mut Cx<Win>) -> El {
    let key = pane.key(PaneKind::Assets);
    let table = pane.model().tables.get(FEED);
    let tokens = pane.tokens();
    col()
        .key(key.clone())
        .w_full()
        .h_full()
        .fill(colour(tokens.pane_tint.alpha(tokens.glass_pane)))
        .child(headline(pane, &key, table))
        .child(worded(
            pane,
            PaneKind::Assets,
            "",
            &DESC,
            table,
            &WORDS,
            false,
        ))
        .child(base(pane, &key))
}
