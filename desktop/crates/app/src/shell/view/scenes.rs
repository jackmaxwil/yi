//! The shell's scenes, built by playing the mockup's threads through the wire reducer.

use yd_core::feed::{FeedEvent, FeedStatus};
use yd_core::grid::step::GridMsg;
use yd_core::ids::Millis;
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::row::Row;
use yd_core::wire::replay::play;
use yd_ui::grid_view::view::{Grip, Grips};
use yd_ui::pane::{SCENE_PANE, Scene, Stage};
use yd_ui::win::Hub;

use crate::root::MAIN;
use crate::shell::view::{JUST_CHAT, change};

/// The mockup's threads, list, forks, replay and options as a server recording: one line
/// a frame, its times counted back from [`NOW`].
const MOCKUP: &str = include_str!("../mockup.jsonl");
/// A real turn whose command was allowed and ran.
const ALLOWED: &str = include_str!("../../../../../fixtures/wire/approval-allow.jsonl");
/// A real server with no thread: it is up and lists none.
const EMPTY: &str = include_str!("../../../../../fixtures/wire/empty-list.jsonl");
/// A real server's turn whose child asked its parent, was answered and finished.
const ASKS: &str = include_str!("../../../../../fixtures/wire/child-ask.jsonl");
/// The mockup's clock, the moment its ages are read at: 14 November, 17:58:44.
pub const NOW: Millis = Millis(1_794_679_124_000);
/// The thread the recording shows first.
const SHOWN: &str = "rate-limit-the-upload-endpoint";
/// The panes of Everything, each with the scene that builds its mockup state. Chat is
/// first: its scene makes the thread the others then read.
const BUILT: [(PaneKind, &str); 8] = [
    (PaneKind::Chat, "chat/mockup"),
    (PaneKind::Files, "files/mockup"),
    (PaneKind::Changes, "changes/mockup"),
    (PaneKind::Terminal, "terminal/mockup"),
    (PaneKind::Code, "code/mockup"),
    (PaneKind::Web, "web/mockup"),
    (PaneKind::Remote, "desktop/mockup"),
    (PaneKind::Report, "report/mockup"),
];

pub const SCENES: &[Scene] = &[
    Scene {
        name: "shell/mockup",
        stage: Stage::App,
        setup: mockup,
    },
    Scene {
        name: "shell/hidden",
        stage: Stage::App,
        setup: hidden,
    },
    Scene {
        name: "shell/just-chat",
        stage: Stage::App,
        setup: just_chat,
    },
    Scene {
        name: "shell/places",
        stage: Stage::App,
        setup: places,
    },
    Scene {
        name: "shell/empty",
        stage: Stage::App,
        setup: empty,
    },
    Scene {
        name: "shell/everything",
        stage: Stage::App,
        setup: everything,
    },
    Scene {
        name: "shell/child-chat",
        stage: Stage::App,
        setup: child_chat,
    },
    Scene {
        name: "shell/reading",
        stage: Stage::App,
        setup: reading,
    },
    Scene {
        name: "shell/panes",
        stage: Stage::App,
        setup: panes,
    },
];

/// The status bar's git row and the mockup's repositories, as the git feed reports them.
fn repositories() -> Vec<Row> {
    vec![
        Row::new("repo")
            .text("project", "atlas-api")
            .text("branch", "rate-limit-uploads")
            .text("ahead", "2")
            .text("behind", "0")
            .text("commit", "a1b9f3e")
            .text("base", "main")
            .text("added", "54")
            .text("removed", "1"),
        Row::new("repo.storefront")
            .text("project", "storefront")
            .text("base", "main"),
    ]
}

/// Invariant: a fork takes the clock as its last activity, so the list plays at clock zero;
/// an ask counts its wait from the clock, so it arrives once the clock is set.
fn played(hub: &mut Hub, recording: &str) {
    let ask = |line: &&str| line.contains("session/request_permission");
    let listed: Vec<&str> = recording.lines().filter(|line| !ask(line)).collect();
    let asks: Vec<&str> = recording.lines().filter(ask).collect();
    for (part, clock) in [(listed, Some(NOW)), (asks, None)] {
        if play(&mut hub.model, &part.join("\n")).is_err() {
            return;
        }
        if let Some(now) = clock {
            step(&mut hub.model, Msg::Tick(now));
        }
    }
}

pub fn mockup(hub: &mut Hub) {
    played(hub, MOCKUP);
    git(hub);
}

/// The repositories' rows, beside whatever changes the feed already holds.
fn git(hub: &mut Hub) {
    let feed = yd_sources::git::FEED;
    for row in repositories() {
        step(&mut hub.model, Msg::Feed(feed, FeedEvent::Upsert(row)));
    }
    step(
        &mut hub.model,
        Msg::Feed(feed, FeedEvent::Status(FeedStatus::Live)),
    );
}

fn hidden(hub: &mut Hub) {
    mockup(hub);
    change(hub, MAIN, |shell| shell.hidden = true);
}

fn just_chat(hub: &mut Hub) {
    mockup(hub);
    let show = GridMsg::Show {
        window: MAIN,
        saved: JUST_CHAT,
    };
    step(&mut hub.model, Msg::Grid(show));
}

/// Review picked in a session whose only change is a diff: Changes in its Review place beside
/// the chat, and Files and the terminal waiting with no hole where they will go.
fn places(hub: &mut Hub) {
    mockup(hub);
    let mut scenes = yd_panes::registry::scenes();
    if let Some(scene) = scenes.find(|scene| scene.name == "changes/mockup") {
        (scene.setup)(hub);
    }
    let (window, saved) = (MAIN, "review");
    step(&mut hub.model, Msg::Grid(GridMsg::Show { window, saved }));
}

/// The mockup's first frame: the sidebar, the bars, and every pane of Everything in the
/// state its own `<slug>/mockup` scene builds, moved from the scene's pane to the grid's.
fn everything(hub: &mut Hub) {
    for (kind, name) in BUILT {
        let mut scenes = yd_panes::registry::scenes();
        if let Some(scene) = scenes.find(|scene| scene.name == name) {
            (scene.setup)(hub);
        }
        let panes = hub.model.grid.panes.iter();
        let placed = panes
            .clone()
            .find(|pane| pane.kind == kind)
            .map(|pane| pane.id);
        if let Some(pane) = placed {
            hub.panes.adopt(SCENE_PANE, pane);
        }
    }
    // The shell's recording, aimed at the thread the chat's scene made. What that scene
    // already holds stays out: the reconnect, the replay, a third ask and its sixth agent.
    let Some(thread) = hub
        .model
        .wire
        .listed()
        .first()
        .map(|thread| thread.id.clone())
    else {
        return;
    };
    let skipped = [
        "\"method\":\"initialize\"",
        "\"_yi/replay\"",
        "session/request_permission",
        "\"id\":\"agent-6\"",
    ];
    let lines = MOCKUP
        .lines()
        .filter(|line| !skipped.iter().any(|part| line.contains(part)));
    let shell = lines.collect::<Vec<_>>().join("\n");
    played(hub, &shell.replace(SHOWN, thread.as_str()));
    git(hub);
}

/// The parent's chat and its child's, side by side, as Open Agent Chat shows them.
fn child_chat(hub: &mut Hub) {
    if play(&mut hub.model, ASKS).is_err() {
        return;
    }
    let shown = hub
        .model
        .wire
        .listed()
        .first()
        .map(|thread| thread.id.clone());
    let Some(thread) = shown else {
        return;
    };
    let show = GridMsg::Show {
        window: MAIN,
        saved: yd_core::grid::saved::BLANK.slug,
    };
    step(&mut hub.model, Msg::Grid(show));
    let agent = yd_core::ids::AgentId::new("sub-655c3f0b");
    if let Ok(thing) = yd_core::reduce::step::commands::child_address(&thread, &agent) {
        let (window, kind) = (MAIN, PaneKind::Chat);
        let about = GridMsg::About {
            window,
            kind,
            thing,
        };
        step(&mut hub.model, Msg::Grid(about));
    }
    yd_ui::shown::pick(hub, MAIN, thread);
}

/// A first window's chat alone, wider than its column, with the command it ran opened.
fn reading(hub: &mut Hub) {
    played(hub, ALLOWED);
    crate::root::blank(hub, MAIN);
    let grid = &hub.model.grid;
    let docked = grid.window(MAIN).map(|window| window.layout.docked());
    let chat = docked
        .into_iter()
        .flatten()
        .map(|(pane, _)| pane)
        .find(|pane| grid.kind(*pane) == Some(PaneKind::Chat));
    let state = hub.panes.get_mut::<yd_panes::chat::pane::Chat>(chat);
    if let Some(state) = state.filter(|_| chat.is_some()) {
        state.steps.push(yd_core::reduce::turns::CallId::new("c1"));
    }
}

/// The Panes panel open over Everything, with one layout of the person's own kept.
fn panes(hub: &mut Hub) {
    everything(hub);
    let name = "Writing".to_owned();
    step(
        &mut hub.model,
        Msg::Grid(GridMsg::Keep { window: MAIN, name }),
    );
    let show = GridMsg::Show {
        window: MAIN,
        saved: "everything",
    };
    step(&mut hub.model, Msg::Grid(show));
    if let Some(grips) = hub.panes.get_mut::<Grips>(None) {
        let open = Grip {
            picking: true,
            ..grips.of(MAIN)
        };
        grips.set(MAIN, open);
    }
}

/// Connected, with no threads yet.
fn empty(hub: &mut Hub) {
    played(hub, EMPTY);
}
