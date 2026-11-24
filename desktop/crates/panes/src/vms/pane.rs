//! VMs pane: virtual machines, as a list over the `vms` feed.

use rune::{Cx, El, Tx, col, text};
use yd_core::command::by_kind::VM_SCREEN;
use yd_core::command::table::Target;
use yd_core::feed::{FeedEvent, FeedStatus, Table, VMS};
use yd_core::ids::{CommandId, FeedId, RowId};
use yd_core::kind::Kind;
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::row::{Rows, rows_from_json};
use yd_ui::chrome::tabs::{PLUS, Tab, Tabs};
use yd_ui::grid_view::view::show_pane;
use yd_ui::kit::bars::{footer, header, lead};
use yd_ui::kit::chip::{Chip, chip};
use yd_ui::list::state::Lists;
use yd_ui::list::view::worded;
use yd_ui::overlay::pick::Finder;
use yd_ui::pane::{PaneCx, PaneDef, SCENE_PANE, Scene, Stage};
use yd_ui::theme::look::{META, colour};
use yd_ui::win::{Hub, Win};

use super::desc::{COMMANDS, DESC, OPEN_SCREEN};

use super::states::WORDS;

/// The feed this pane lists; its source declares the same name.
pub const FEED: FeedId = VMS;

const ROWS: &str = include_str!("../../../../fixtures/mockup/vms.rows.json");

pub const DEF: PaneDef = PaneDef {
    kind: PaneKind::Vms,
    view,
    scenes: &[
        Scene {
            name: "vms/stub",
            stage: Stage::Pane(PaneKind::Vms),
            setup: |_| {},
        },
        Scene {
            name: "vms/mockup",
            stage: Stage::Pane(PaneKind::Vms),
            setup: mockup,
        },
    ],
    commands: COMMANDS,
    settings: &[],
    tabs: Some(tabs),
    on_tab: None,
    finder: Some(Finder::Rows(&DESC)),
};

/// Frame 7's VMs from the fixture, the way the source would send them, with ci-ubuntu open.
fn mockup(hub: &mut Hub) {
    let event = match rows_from_json(ROWS) {
        Ok(rows) => FeedEvent::Reset(rows),
        Err(error) => FeedEvent::Status(FeedStatus::Error(error.to_string())),
    };
    step(&mut hub.model, Msg::Feed(FEED, event));
    if let Some(lists) = hub.panes.get_mut::<Lists>(Some(SCENE_PANE)) {
        lists.list_mut("").set_open(Some(RowId::new("ci-ubuntu")));
    }
}

/// The host every VM runs on, as its rows name it; the tab says it with the count.
fn tabs(pane: &PaneCx<'_>) -> Tabs {
    let table = pane.model().tables.get(FEED);
    let total = table.map_or(0, Table::len);
    let host = table
        .and_then(|table| table.get(0))
        .and_then(|row| row.words("host"))
        .unwrap_or_default();
    let meta = if host.is_empty() {
        String::new()
    } else {
        format!("{host} · {total}")
    };
    let tab = Tab {
        count: u32::try_from(total).ok().filter(|total| *total > 0),
        meta,
        ..Tab::of(PaneKind::Vms)
    };
    Tabs {
        tabs: vec![tab],
        ..Tabs::default()
    }
}

/// The header bar: the host, how many VMs there are, and the button that makes one.
fn headline(pane: &PaneCx<'_>, key: &str, table: Option<&Table>) -> El {
    let tokens = pane.tokens();
    let total = table.map_or(0, Table::len);
    // The host is the one the rows name: an empty or unanswered list names none.
    let host = table
        .and_then(|table| table.get(0))
        .and_then(|row| row.words("host"));
    let host = host.filter(|host| !host.is_empty());
    let host = host.map(|host| chip(tokens, Chip::Plain, host, None, true));
    let left = host
        .into_iter()
        .chain([lead(tokens, &format!("{total} VMs"))])
        .collect();
    let right = vec![chip(tokens, Chip::Plain, "New VM", Some(&PLUS), false)];
    header(tokens, left, right).key(format!("{key}.pane-header"))
}

fn base(pane: &PaneCx<'_>, key: &str) -> El {
    let tokens = pane.tokens();
    let legend = vec![
        text("Open screen puts the VM in a desktop pane")
            .font(META)
            .color(colour(tokens.faint)),
    ];
    footer(tokens, legend, Vec::new()).key(format!("{key}.footer"))
}

/// Open screen, from an opened machine or its right-click menu: the Remote desktop pane in
/// this window. False for any other command or target.
pub fn carry(win: &mut Win, tx: &mut Tx, command: CommandId, target: &Target) -> bool {
    let Target::Thing(address) = target else {
        return false;
    };
    let screen = command == OPEN_SCREEN || command == VM_SCREEN;
    if address.kind() != Kind::Vm || !screen {
        return false;
    }
    let shown = show_pane(win, tx, PaneKind::Remote);
    shown.is_some_and(|pane| {
        crate::remote::pane::open(win, pane, address.id());
        true
    })
}

fn view(pane: &PaneCx<'_>, _cx: &mut Cx<Win>) -> El {
    let key = pane.key(PaneKind::Vms);
    let table = pane.model().tables.get(FEED);
    let tokens = pane.tokens();
    col()
        .key(key.clone())
        .w_full()
        .h_full()
        .fill(colour(tokens.pane_tint.alpha(tokens.glass_pane)))
        .child(headline(pane, &key, table))
        .child(worded(pane, PaneKind::Vms, "", &DESC, table, &WORDS, false))
        .child(base(pane, &key))
}
