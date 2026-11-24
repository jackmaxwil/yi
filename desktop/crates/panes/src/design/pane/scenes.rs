//! The design scenes: the Design pane as frame 24 draws it, the box over the web page, and
//! a marked-up screen.

use rune::{Cx, El, col, el};
use yd_core::command::tools::design::{CHANGE, ELEMENT, Property};
use yd_core::feed::{FeedEvent, FeedStatus};
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::row::Row;
use yd_ui::pane::{PaneCx, SCENE_PANE, Scene, Stage};
use yd_ui::screen::input::Point;
use yd_ui::screen::surface::{Screen, surface};
use yd_ui::theme::look::colour;
use yd_ui::win::{Hub, Win};

use super::{FEED, Mode};
use crate::design::markup::{self, Marks, Note};
use crate::web::pane::scenes::{framed, strings};
use crate::web::pane::{Sample, State as Web};

const TEXT: &str = include_str!("../../../../../fixtures/mockup/panes/design.text.json");
const PAGE: &str = include_str!("../../../../../fixtures/mockup/panes/web.text.json");
const FRAME: &str = include_str!("../../../../../fixtures/mockup/frames/24.strings.json");
/// Frame 24's page area at 2x: 634 points wide under the tab bar and the browser bar.
const BLANK: (u32, u32) = (1268, 1316);
/// Where the sample page draws its alert, in CSS pixels: under the site's bar, inset 16.
const ALERT: (u32, u32) = (16, 40);
/// The remote screen's picture in the mark-up scene, and the row the mockup boxes on it.
const REMOTE: (u32, u32) = (800, 450);
const REGION: (Point, u32, u32) = (Point { x: 44, y: 120 }, 300, 36);
/// The mockup's note (record 4.9.2): the fixtures hold the screen's words but not the note's.
const NOTE: &str = "Say when the retry will happen";

pub const SCENES: &[Scene] = &[
    Scene {
        name: "design/mockup",
        stage: Stage::View(|win, hub, cx| framed(win, hub, cx, PaneKind::Design, (400.0, 450.0))),
        setup: mockup,
    },
    Scene {
        name: "design/box",
        stage: Stage::View(|win, hub, cx| framed(win, hub, cx, PaneKind::Web, (634.0, 700.0))),
        setup: boxed,
    },
    Scene {
        name: "design/mark-up",
        stage: Stage::View(marked_up),
        setup: marks,
    },
    Scene {
        name: "design/empty",
        stage: Stage::Pane(PaneKind::Design),
        setup: |_| {},
    },
];

/// The mockup's value now and before of each property, by where frame 24's text holds them.
fn values(s: &[String]) -> Vec<(Property, String, String)> {
    let at = |i: usize| s.get(i).cloned().unwrap_or_default();
    let (said_before, said) = (strings(PAGE).get(9).cloned().unwrap_or_default(), at(23));
    let padding = format!("{} {}", at(10), at(11));
    let (before, round) = at(30).split_once(" → ").map_or_else(
        || (at(20), at(11)),
        |(before, round)| (before.to_owned(), round.to_owned()),
    );
    vec![
        (Property::Size, at(8), at(8)),
        (Property::Padding, padding.clone(), padding),
        (Property::Fill, at(14), at(15)),
        (Property::Text, at(17), at(18)),
        (Property::Corners, round, before),
        (Property::Says, said, said_before),
    ]
}

/// The element's row and one row a change, as the page's source sends them for frame 24.
pub fn rows() -> Vec<Row> {
    let s = strings(TEXT);
    let at = |i: usize| s.get(i).cloned().unwrap_or_default();
    let page = strings(FRAME).into_iter().find(|s| s.contains("/upload"));
    let page = page.unwrap_or_default();
    let size = yd_core::command::tools::design::pair(&at(8)).unwrap_or_default();
    let twice = |v: u32| v.saturating_mul(2).to_string();
    let mut element = Row::new(ELEMENT)
        .text("n", "0")
        .text("name", at(3))
        .text("source", at(4))
        .text("page", page.as_str())
        .text("leaf", "yes")
        .text("x", twice(ALERT.0))
        .text("y", twice(ALERT.1))
        .text("w", twice(size.0))
        .text("h", twice(size.1))
        .text("ratio", "2")
        .text("gap", "10")
        .text("inset", ALERT.0.to_string())
        .text("note", "");
    let mut rows = Vec::new();
    for (property, now, was) in values(&s) {
        let slug = property.slug();
        if now != was {
            let change = Row::new(format!("{CHANGE}0:{slug}"))
                .text("element", at(3))
                .text("source", at(4))
                .text("page", page.as_str())
                .text("property", slug)
                .text("was", was.as_str())
                .text("now", now.as_str());
            rows.push(change);
        }
        element = element.text(slug, now).text(&format!("{slug}_was"), was);
    }
    rows.push(element);
    rows
}

/// Frame 24's Design pane: the alert picked, four of its values changed.
pub fn mockup(hub: &mut Hub) {
    let live = FeedEvent::Status(FeedStatus::Live);
    step(&mut hub.model, Msg::Feed(FEED, live));
    for row in rows() {
        step(&mut hub.model, Msg::Feed(FEED, FeedEvent::Upsert(row)));
    }
    if let Some(mode) = hub.panes.get_mut::<Mode>(None) {
        mode.on = true;
    }
}

/// Frame 24's web page: the alert as changed, the design box round it.
fn boxed(hub: &mut Hub) {
    mockup(hub);
    let (frame, page) = (strings(FRAME), strings(PAGE));
    let found = |part: &str| frame.iter().find(|s| s.contains(part)).cloned();
    let url = found("/upload").unwrap_or_default();
    let meta = page.get(1).cloned().unwrap_or_default();
    let (status, took) = meta.split_once(" · ").unwrap_or(("", &meta));
    let wide = found(" wide").unwrap_or_default();
    let row = Row::new("page:0")
        .text("url", format!("http://{url}"))
        .text("title", url.split('/').next().unwrap_or_default())
        .text("status", status)
        .text("ms", took.trim_end_matches(" ms"))
        .text("width", wide.trim_end_matches(" wide"))
        .text("can_back", "yes")
        .text("can_forward", "no")
        .text("loading", "no")
        .text("note", "");
    step(&mut hub.model, Msg::Feed(FEED, FeedEvent::Upsert(row)));
    let look = values(&strings(TEXT));
    let now = |property: Property| {
        let found = look.iter().find(|(known, ..)| *known == property);
        found.map(|(_, now, _)| now.clone()).unwrap_or_default()
    };
    let Some(state) = hub.panes.get_mut::<Web>(Some(SCENE_PANE)) else {
        return;
    };
    let radius = now(Property::Corners).parse().unwrap_or(0);
    state.sample = Some(Sample {
        nav: page.get(4..9).map(<[String]>::to_vec).unwrap_or_default(),
        alert: now(Property::Says),
        look: Some((now(Property::Fill), now(Property::Text), radius)),
        drop: page.get(10).cloned().unwrap_or_default(),
        button: page.get(11).cloned().unwrap_or_default(),
    });
    let (w, h) = BLANK;
    let white = vec![255; (w * h * 4) as usize];
    let _ = state.screen.push(w, h, white);
}

/// What the mark-up scene keeps: the remote screen's picture.
#[derive(Debug, Default)]
struct Remote {
    screen: Screen,
}

/// Frame 24's remote screen: a box round the file that waits, and the note beside it.
fn marks(hub: &mut Hub) {
    let frame = strings(FRAME);
    let found = |part: &str| frame.iter().find(|s| s.contains(part)).cloned();
    let machine = found("ci-ubuntu").unwrap_or_default();
    let file = found("detail-front").unwrap_or_default();
    let note = Note {
        at: REGION.0,
        width: REGION.1,
        height: REGION.2,
        label: format!("{machine} · {file} row"),
        words: NOTE.to_owned(),
    };
    let thing = note.thing(&machine);
    if let Some(marks) = hub.panes.get_mut::<Marks>(Some(SCENE_PANE)) {
        marks.notes = vec![note];
    }
    if let Some(thing) = thing {
        use yd_core::address::attach::AttachMsg;
        step(&mut hub.model, Msg::Attach(AttachMsg::Toggle(thing)));
    }
    let (w, h) = REMOTE;
    let grey: Vec<u8> = [44, 48, 58, 255].repeat((w * h) as usize);
    if let Some(remote) = hub.panes.get_mut::<Remote>(Some(SCENE_PANE)) {
        let _ = remote.screen.push(w, h, grey);
    }
}

/// A screen with the mark-up layer over it, as a remote pane would draw the two.
fn marked_up(win: &Win, hub: &Hub, cx: &mut Cx<Win>) -> El {
    let tokens = &hub.model.tokens;
    let pane = PaneCx {
        hub,
        pane: SCENE_PANE,
        focused: true,
        edits: Some(&win.edits),
    };
    let fallback = Remote::default();
    let remote = hub.panes.get::<Remote>(Some(SCENE_PANE));
    let screen = &remote.unwrap_or(&fallback).screen;
    let machine = strings(FRAME).into_iter().find(|s| s.contains("ci-ubuntu"));
    let machine = machine.unwrap_or_default();
    let named = machine.clone();
    let on = move |win: &mut Win, input| {
        markup::took(win, SCENE_PANE, &named, input);
    };
    let key = "design-0.screen";
    col()
        .w_full()
        .h_full()
        .padding(16.0)
        .fill(colour(tokens.window))
        .child(
            el().w(400.0)
                .h(225.0)
                .child(surface(key, screen, on))
                .child(markup::layer(&pane, cx, key, screen, &machine)),
        )
}
