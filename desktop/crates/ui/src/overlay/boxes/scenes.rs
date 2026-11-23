//! The overlay's scenes: three boxes over stand-in list panes, picking, and the mockup's tray.

use rune::{Cx, El, col, row, spacer};
use yd_core::address::attach::AttachMsg;
use yd_core::address::thing::Thing;
use yd_core::feed::{FeedEvent, FeedStatus};
use yd_core::ids::{FeedId, PaneId, RowId};
use yd_core::kind::Kind;
use yd_core::list::{Col, Detail, ListDesc, RowHeight, Text, Width};
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::row::{Row, rows_from_json};
use yd_core::wire::replay::play;

use super::over;
use super::tray::tray;
use crate::list::row as list_row;
use crate::list::view::list;
use crate::overlay::pick::{self, Context, Finder, Found};
use crate::pane::{PaneCx, Scene, Stage};
use crate::theme::look::{colour, white};
use crate::win::{Hub, Win};

pub(super) const SCENES: &[Scene] = &[
    Scene {
        name: "overlay/boxes",
        stage: Stage::View(boxed),
        setup: boxes,
    },
    Scene {
        name: "overlay/picking",
        stage: Stage::View(boxed),
        setup: picking,
    },
    Scene {
        name: "overlay/tray",
        stage: Stage::View(boxed),
        setup: mockup,
    },
];

/// The tray as a chat pane keys it: `<pane key>.context-tray`.
pub(super) const TRAY: &str = "chat-0.context-tray";
/// The width of the chat pane in the mockup's frame 23; its tray is 264 of it.
const CHAT: f32 = 280.0;

const fn column(field: &'static str, title: &'static str, width: Width, text: Text) -> Col {
    Col {
        field,
        title,
        width,
        text,
    }
}

fn bare(_: &Row) -> Detail {
    Detail::default()
}

const fn desc(kind: Kind, columns: &'static [Col], tree: bool) -> ListDesc {
    ListDesc {
        kind,
        columns,
        tree,
        height: RowHeight::Row,
        facts_per_line: 3,
        detail: bare,
        footer: None,
    }
}

/// A list pane standing in for a built one: its pane, its kind, its rows and how they read.
pub(super) struct StandIn {
    pub pane: PaneId,
    pub kind: PaneKind,
    pub desc: ListDesc,
    rows: &'static str,
}

/// Three of the mockup's lists, drawn by the list engine with a few of their columns.
pub(super) static PANES: [StandIn; 3] = [
    StandIn {
        pane: PaneId(1),
        kind: PaneKind::Agents,
        desc: desc(
            Kind::Agent,
            &[
                column("state", "", Width::Px(7.0), Text::Plain),
                column("agent", "Agent", Width::Px(150.0), Text::Name),
                column("doing_now", "Doing now", Width::Fr(1.0), Text::Plain),
            ],
            true,
        ),
        rows: include_str!("../../../../../fixtures/mockup/agents.rows.json"),
    },
    StandIn {
        pane: PaneId(2),
        kind: PaneKind::Containers,
        desc: desc(
            Kind::Container,
            &[
                column("state", "", Width::Px(10.0), Text::Plain),
                column("name", "Name", Width::Px(88.0), Text::Name),
                column("image", "Image", Width::Fr(1.0), Text::Mono),
                column("up", "Up", Width::Px(40.0), Text::Number),
            ],
            false,
        ),
        rows: include_str!("../../../../../fixtures/mockup/containers.rows.json"),
    },
    StandIn {
        pane: PaneId(3),
        kind: PaneKind::Daemons,
        desc: desc(
            Kind::Service,
            &[
                column("state", "", Width::Px(10.0), Text::Plain),
                column("name", "Name", Width::Px(96.0), Text::Name),
                column("what_it_is", "What it is", Width::Fr(1.0), Text::Plain),
            ],
            false,
        ),
        rows: include_str!("../../../../../fixtures/mockup/daemons.rows.json"),
    },
];

/// The row of each stand-in that the scenes box, by pane: an agent, a container, a service.
pub(super) const BOXED: [(usize, &str); 3] = [
    (0, "fork-sliding-window"),
    (1, "atlas-api"),
    (2, "yi-serve"),
];
/// A container attached fourth whose row then leaves its feed: its chip stays, struck.
pub(super) const GONE: (usize, &str) = (1, "registry");
/// The row the pointer is over in the picking scene.
pub(super) const HOVERED: (usize, &str) = (1, "atlas-db");

/// The stand-ins side by side, a chat pane's column at the right with the tray at its foot,
/// and the overlay over all of it.
fn boxed(_: &Win, hub: &Hub, cx: &mut Cx<Win>) -> El {
    let tokens = &hub.model.tokens;
    let hairline = colour(white(tokens.lines));
    let pane = |stand_in: &StandIn| {
        let context = PaneCx {
            hub,
            pane: stand_in.pane,
            focused: false,
            edits: None,
        };
        let table = hub.model.tables.get(FeedId::new(stand_in.kind.slug()));
        col()
            .grow(1.0)
            .min_w(0.0)
            .h_full()
            .border_sides([0.0, 1.0, 0.0, 0.0], hairline)
            .child(list(&context, stand_in.kind, &stand_in.desc, table))
    };
    let chat = col()
        .w(CHAT)
        .h_full()
        .px(8.0)
        .pb(6.0)
        .fill(colour(tokens.pane_tint.alpha(tokens.glass_pane_focus)))
        .child(spacer())
        .children(tray(hub, TRAY));
    let shown = PANES.iter().filter(|stand_in| {
        let table = hub.model.tables.get(FeedId::new(stand_in.kind.slug()));
        table.is_some()
    });
    let base = row()
        .w_full()
        .h_full()
        .children(shown.map(pane))
        .child(spacer())
        .child(chat);
    over(base, hub, cx)
}

/// The key the list engine gives row `id` of stand-in `index`.
pub(super) fn row_key(index: usize, id: &str) -> Option<String> {
    let stand_in = PANES.get(index)?;
    let pane = format!("{}-{}", stand_in.kind.slug(), stand_in.pane.0);
    Some(list_row::key(&pane, &RowId::new(id)))
}

/// What the lookup finds at row `id` of stand-in `index`.
pub(super) fn found(hub: &Hub, index: usize, id: &str) -> Option<Found> {
    let key = row_key(index, id)?;
    let thing = pick::thing_at(hub, &key)?;
    Some(Found { key, thing })
}

/// Each stand-in's rows go through `step` as its feed would send them, and the lookup is
/// told how each reads its rows.
pub(super) fn lists(hub: &mut Hub) {
    for stand_in in &PANES {
        let feed = FeedId::new(stand_in.kind.slug());
        let event = match rows_from_json(stand_in.rows) {
            Ok(rows) => FeedEvent::Reset(rows),
            Err(error) => FeedEvent::Status(FeedStatus::Error(error.to_string())),
        };
        step(&mut hub.model, Msg::Feed(feed, event));
        step(
            &mut hub.model,
            Msg::Feed(feed, FeedEvent::Status(FeedStatus::Live)),
        );
        if let Some(context) = hub.panes.get_mut::<Context>(None) {
            context.find_with(stand_in.kind, Finder::Rows(&stand_in.desc));
        }
    }
}

pub(super) fn boxes(hub: &mut Hub) {
    lists(hub);
    for (index, id) in BOXED.into_iter().chain([GONE]) {
        if let Some(found) = found(hub, index, id) {
            pick::attach(hub, found);
        }
    }
    let feed = PANES
        .get(GONE.0)
        .map(|stand_in| FeedId::new(stand_in.kind.slug()));
    if let Some(feed) = feed {
        let gone = FeedEvent::Remove(RowId::new(GONE.1));
        step(&mut hub.model, Msg::Feed(feed, gone));
    }
}

fn picking(hub: &mut Hub) {
    boxes(hub);
    let hover = found(hub, HOVERED.0, HOVERED.1);
    if let Some(context) = hub.panes.get_mut::<Context>(None) {
        context.picking = true;
        context.hover = hover;
    }
}

const SIDEBAR: &str = include_str!("../../../../../fixtures/mockup/sidebar.rows.json");
/// The thread the mockup attaches fourth, by its row in the sidebar's fixture.
const FORK: &str = "fork-sliding-window";
/// The three things the mockup attaches that no pane here draws yet (record 4.8): eight
/// lines of a diff, the web app's alert, and a window on the remote screen.
const LINES: &str = "yi://lines/src/middleware/rate_limit.rs#L13-L20";
const ALERT: &str = "yi://element/localhost:5173/upload#div.alert";
const WINDOW: (&str, &str) = (
    "yi://screen/ci-ubuntu#window/terminal",
    "ci-ubuntu · Terminal window",
);

/// The server's list with the fixture's thread in it, as a recording: the title is the
/// sidebar row's own.
fn listed(title: &str) -> String {
    let hello = r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":2,"info":{"name":"yi","version":"0"},"capabilities":{},"authMethods":[]}}"#;
    let sessions = format!(
        r#"{{"jsonrpc":"2.0","id":2,"result":{{"sessions":[{{"sessionId":"{FORK}","cwd":"/work/atlas-api","title":{title:?}}}]}}}}"#
    );
    [
        r#"{"dir":"send","frame":{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":2}}}"#.to_owned(),
        format!(r#"{{"dir":"recv","frame":{hello}}}"#),
        r#"{"dir":"send","frame":{"jsonrpc":"2.0","id":2,"method":"session/list","params":{}}}"#.to_owned(),
        format!(r#"{{"dir":"recv","frame":{sessions}}}"#),
    ]
    .join("\n")
}

/// The mockup's four attached things (frame 23), each named by the lookup's own rules: what
/// `context-tray/mockup` attaches over the chat pane's own scene.
pub fn mockup(hub: &mut Hub) {
    let rows = rows_from_json(SIDEBAR).unwrap_or_default();
    let fork = rows.iter().find(|row| row.id.as_str() == FORK);
    if let Some(title) = fork.and_then(|row| row.words("title"))
        && play(&mut hub.model, &listed(title)).is_err()
    {
        return;
    }
    let thread = format!("yi://thread/{FORK}");
    let window = WINDOW.0.parse().ok().map(|address| Thing {
        label: WINDOW.1.to_owned(),
        address,
        excerpt: None,
    });
    let at = |hub: &Hub, address: &str| Some(Thing::at(&hub.model, address.parse().ok()?));
    let things = [at(hub, LINES), at(hub, ALERT), window, at(hub, &thread)];
    for thing in things.into_iter().flatten() {
        let named = thing.named(&hub.model);
        step(&mut hub.model, Msg::Attach(AttachMsg::Toggle(named)));
    }
}
