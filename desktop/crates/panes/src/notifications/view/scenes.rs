//! The panel's scenes: the mockup's four rows, played through the wire reducer.

use rune::{Cx, El, col};
use yd_core::ids::Millis;
use yd_core::msg::{Msg, step};
use yd_core::wire::replay::play;
use yd_ui::pane::{Scene, Stage};
use yd_ui::theme::look::colour;
use yd_ui::win::{Hub, Win};

use super::bell::Bell;

/// The mockup's list, replays and ask: three threads that need or finished, and one that
/// finished unseen.
const MOCKUP: &str = include_str!("mockup.jsonl");
/// The same list once the user has seen the last thread.
const SEEN: &str = include_str!("seen.jsonl");
/// The mockup's clock, the moment its ages are read at: 14 November, 17:58:44.
const NOW: Millis = Millis(1_794_679_124_000);

pub const SCENES: &[Scene] = &[
    Scene {
        name: "notifications/mockup",
        stage: Stage::View(window),
        setup: mockup,
    },
    Scene {
        name: "notifications/empty",
        stage: Stage::View(window),
        setup: empty,
    },
];

/// The window the panel hangs from.
fn window(_: &Win, hub: &Hub, _: &mut Cx<Win>) -> El {
    col()
        .key("root")
        .w_full()
        .h_full()
        .fill(colour(hub.model.tokens.window))
        .children(super::layer(hub))
}

fn fold(hub: &mut Hub) {
    let Hub { model, panes, .. } = hub;
    if let Some(bell) = panes.get_mut::<Bell>(None) {
        bell.sync(&model.wire);
    }
}

/// Rate limit and Cold start wait, Checkout finished, and Notes 2.4 finished and was seen.
pub fn mockup(hub: &mut Hub) {
    let lines: Vec<&str> = MOCKUP.lines().collect();
    let (head, tail) = lines.split_at(4.min(lines.len()));
    for (part, clock) in [(head, None), (tail, Some(NOW))] {
        if let Some(now) = clock {
            step(&mut hub.model, Msg::Tick(now));
        }
        if play(&mut hub.model, &part.join("\n")).is_err() {
            return;
        }
    }
    fold(hub);
    if play(&mut hub.model, SEEN).is_ok() {
        fold(hub);
    }
    open(hub);
}

/// Connected, nothing to read.
fn empty(hub: &mut Hub) {
    let hello: Vec<&str> = MOCKUP.lines().take(2).collect();
    if play(&mut hub.model, &hello.join("\n")).is_ok() {
        step(&mut hub.model, Msg::Tick(NOW));
    }
    open(hub);
}

fn open(hub: &mut Hub) {
    if let Some(bell) = hub.panes.get_mut::<Bell>(None) {
        bell.open = true;
    }
}
