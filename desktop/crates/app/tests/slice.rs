//! The thin slice: a feed, through `step`, into a pane a scene draws; and a command from a
//! window changing the model every window shares.

use std::collections::BTreeSet;
use std::error::Error;

use rune::{Env, Runtime};
use yd_core::command::table::{FLIP_CHAT, Route, Run};
use yd_core::grid::layout::ChatSide;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_ui::pane::{Stage, show_scene};
use yd_ui::slots::SLOTS;
use yd_ui::win::{Win, dispatch};
use yi_desktop::{root, scenes};

type Check = Result<(), Box<dyn Error>>;

/// What the Daemons pane says while launchd has not answered.
const LOADING: &str = yd_panes::daemons::states::WORDS.loading.what;

/// What the scene `name` draws, as the words on screen.
fn words(name: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let mut win = root::init();
    let all = scenes::all();
    let index = all
        .iter()
        .position(|scene| scene.name == name)
        .ok_or(format!("no scene {name}"))?;
    show_scene(&mut win, index);
    let mut runtime = Runtime::new(win, root::view, (1200.0, 750.0), Env::new(true, 1.0));
    runtime.frame(1.0 / 60.0);
    Ok(runtime
        .elements()
        .into_iter()
        .filter_map(|element| element.text)
        .collect())
}

#[test]
fn rows_from_a_feed_show_in_a_pane() -> Check {
    let shown = words("daemons/sample")?;
    for service in [
        "com.yi.daemon",
        "com.docker.vmnetd",
        "homebrew.mxcl.postgresql@16",
    ] {
        assert!(
            shown.iter().any(|text| text == service),
            "{service} is not drawn: {shown:?}"
        );
    }
    assert!(
        !shown.iter().any(|text| text == LOADING),
        "a live list still says Loading"
    );
    let before = words("daemons/stub")?;
    assert!(
        before.iter().any(|text| text == LOADING),
        "a feed that sent nothing: {before:?}"
    );
    Ok(())
}

#[test]
fn a_command_from_a_window_changes_the_shared_model() -> Check {
    let mut win = root::init();
    let other = Win::new(win.hub.clone(), yd_core::ids::WindowId(1));
    dispatch(&mut win, Msg::Command(Route::Model, Run::new(FLIP_CHAT)));
    assert_eq!(other.hub.try_borrow()?.model.grid.side, ChatSide::Right);
    Ok(())
}

/// `just scenes` renders every name the binary lists: each needs a slot, and its own name.
#[test]
fn every_scene_has_a_slot_and_draws_something() -> Check {
    let all = scenes::all();
    assert!(all.len() <= SLOTS, "{} scenes, {SLOTS} slots", all.len());
    let names: BTreeSet<&str> = all.iter().map(|scene| scene.name).collect();
    assert_eq!(names.len(), all.len(), "two scenes share a name");
    for scene in &all {
        let shown = words(scene.name)?;
        assert!(!shown.is_empty(), "{} draws no words", scene.name);
        if let Stage::Pane(kind) = scene.stage {
            // A pane that keeps its own tabs is named by them: a file, a shell.
            let defs = yd_panes::registry::PANES.iter();
            let own = defs
                .clone()
                .any(|def| def.kind == kind && def.tabs.is_some());
            assert!(
                own || shown.iter().any(|text| text == kind.title()),
                "{} does not name its pane",
                scene.name
            );
        }
    }
    for kind in PaneKind::ALL {
        let shows = |stage: Stage| matches!(stage, Stage::Pane(of) if of == *kind);
        assert!(
            all.iter().any(|scene| shows(scene.stage)),
            "{kind:?} has no scene"
        );
    }
    Ok(())
}

/// A pane and its source never name each other: they meet at the feed's name.
#[test]
fn every_list_pane_has_a_source_for_its_feed() {
    use yd_panes::{assets, containers, daemons, lsp, vms};
    let feeds = [
        assets::pane::FEED,
        containers::pane::FEED,
        vms::pane::FEED,
        daemons::pane::FEED,
        lsp::pane::FEED,
    ];
    for feed in feeds {
        let found = yd_sources::registry::find(feed).map(|def| def.id);
        assert_eq!(
            found,
            Some(feed),
            "no source fills the {} feed",
            feed.source
        );
    }
}
