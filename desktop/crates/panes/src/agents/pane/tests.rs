//! What a user sees and does in the Agents pane, drawn in process with no GPU.

use std::error::Error;

use rune::{ElementInfo, Env, Runtime};
use yd_core::ids::{RowId, WindowId};
use yd_core::row::{Dot, Row, Rows, Value};
use yd_ui::pane::{PaneCx, PaneDef, SCENE_PANE};
use yd_ui::win::{Hub, Win};

use super::fold::{rows, summarise, table};
use super::{DEF, scenes};

type Check = Result<(), Box<dyn Error>>;

/// The setup of the scene named `name`.
fn setup(name: &str) -> Result<fn(&mut Hub), Box<dyn Error>> {
    let mut all = scenes::SCENES.iter().chain(yd_ui::states::status::SCENES);
    let scene = all.find(|scene| scene.name == name);
    Ok(scene.ok_or_else(|| format!("no scene {name}"))?.setup)
}

fn root(win: &Win, cx: &mut rune::Cx<Win>) -> rune::El {
    let Ok(hub) = win.hub.try_borrow() else {
        return rune::col();
    };
    let pane = PaneCx {
        hub: &hub,
        pane: SCENE_PANE,
        focused: true,
        edits: Some(&win.edits),
    };
    (DEF.view)(&pane, cx)
}

fn draw(setup: fn(&mut Hub)) -> Result<Runtime<Win>, Box<dyn Error>> {
    static DEFS: [&PaneDef; 1] = [&DEF];
    let hub = Hub::new(&DEFS, Vec::new());
    setup(&mut *hub.try_borrow_mut()?);
    let mut rt = Runtime::new(
        Win::new(hub, WindowId(0)),
        root,
        (517.0, 346.0),
        Env::new(true, 1.0),
    );
    for _ in 0..4 {
        rt.frame(0.016);
    }
    Ok(rt)
}

fn find<'a>(tree: &'a [ElementInfo], key: &str) -> Result<&'a ElementInfo, Box<dyn Error>> {
    let found = tree.iter().find(|e| e.key.as_deref() == Some(key));
    found.ok_or_else(|| format!("no element keyed {key}").into())
}

fn shows(tree: &[ElementInfo], words: &str) -> bool {
    let live = tree.iter().filter(|e| !e.exiting);
    live.into_iter()
        .any(|e| e.text.as_deref().is_some_and(|t| t.contains(words)))
}

/// The thread of the scene, folded.
fn folded(scene: fn(&mut Hub)) -> Result<Vec<Row>, Box<dyn Error>> {
    let shared = Hub::new(&[], Vec::new());
    let mut hub = shared.try_borrow_mut()?;
    scene(&mut hub);
    let thread = hub
        .model
        .wire
        .listed()
        .into_iter()
        .next()
        .ok_or("no thread")?;
    Ok(rows(&hub.model, thread))
}

fn cell<'a>(rows: &'a [Row], id: &str, field: &str) -> Result<&'a str, Box<dyn Error>> {
    let row = rows.iter().find(|row| row.id.as_str() == id);
    let row = row.ok_or_else(|| format!("no row {id}"))?;
    row.words(field)
        .ok_or_else(|| format!("{id} has no words in {field}").into())
}

#[test]
fn the_mockup_draws_the_record_header_rows_and_footer() -> Check {
    let rt = draw(setup("agents/mockup")?)?;
    let tree = rt.elements();
    for words in [
        "Rate limit",
        "2 working · 2 need you · 2 done",
        "$0.54 of $2.00",
        "Fork · sliding window",
        "wants to run cargo test",
        "72.9K tokens · 4 of 8 agent slots · depth 2",
        "Sonnet",
        "Said last",
        "Run its command",
    ] {
        assert!(shows(&tree, words), "{words:?} is not drawn");
    }
    let order = [
        "rate-limit-the-upload-endpoint",
        "fork-sliding-window",
        "tests-upload",
        "bisect-the-flaky-case",
        "read-the-ci-logs",
        "docs-changelog",
    ];
    let mut top = f32::MIN;
    for id in order {
        let at = find(&tree, &format!("agents-0.row.{id}"))?.rect[1];
        assert!(at > top, "{id} is not under the row before it");
        top = at;
    }
    Ok(())
}

#[test]
fn the_tab_counts_the_agents_and_says_how_deep() -> Check {
    let shared = Hub::new(&[], Vec::new());
    let mut hub = shared.try_borrow_mut()?;
    setup("agents/mockup")?(&mut hub);
    let pane = PaneCx {
        hub: &hub,
        pane: SCENE_PANE,
        focused: true,
        edits: None,
    };
    let tabs = DEF.tabs.ok_or("no tabs")?(&pane);
    let tab = tabs.tabs.first().ok_or("no tab")?;
    assert_eq!((tab.count, tab.meta.as_str()), (Some(6), "depth 2"));
    Ok(())
}

#[test]
fn a_live_thread_folds_its_children_under_the_agents_that_reported_them() -> Check {
    let rows = folded(setup("agents/live")?)?;
    let parent = |id: &str| {
        let row = rows.iter().find(|row| row.id.as_str() == id);
        row.and_then(|row| row.parent.as_ref().map(|p| p.as_str().to_owned()))
    };
    let top = rows.first().ok_or("no rows")?.id.as_str().to_owned();
    assert_eq!(parent("tests"), Some(top.clone()));
    assert_eq!(parent("bisect"), Some("tests".to_owned()));
    assert_eq!(parent("docs"), Some(top));
    assert_eq!(rows.len(), 5, "the thread, then its four agents");
    Ok(())
}

#[test]
fn a_waiting_command_is_named_and_what_the_server_never_sent_is_a_dash() -> Check {
    let rows = folded(setup("agents/live")?)?;
    assert_eq!(
        cell(&rows, "tests", "doing_now")?,
        "wants to run cargo test -p api upload::"
    );
    let state = rows.iter().find(|row| row.id.as_str() == "tests");
    assert_eq!(
        state.and_then(|row| row.get("state")),
        Some(&Value::Dot(Dot::Need))
    );
    for field in ["model", "time"] {
        assert_eq!(cell(&rows, "tests", field)?, "–", "{field}");
    }
    assert_eq!(
        cell(&rows, "tests", "budget")?,
        "5.0K of – tokens · 1 child"
    );
    assert_eq!(cell(&rows, "fork", "time")?, "3m");
    assert_eq!(
        cell(&rows, "fork", "doing_now")?,
        "Sliding window kept as a branch."
    );
    let actions = rows.iter().find(|row| row.id.as_str() == "tests");
    let Some(Value::Tags(names)) = actions.and_then(|row| row.get("actions")) else {
        return Err("no buttons".into());
    };
    assert_eq!(
        names,
        &["Run its command ↵", "Pause", "More budget", "Stop"]
    );
    Ok(())
}

#[test]
fn any_agent_opens_in_place_and_clicking_it_again_closes_it() -> Check {
    let mut rt = draw(setup("agents/live")?)?;
    let press = |rt: &mut Runtime<Win>, key: &str| -> Check {
        let [x, y, _, _] = find(&rt.elements(), key)?.rect;
        rt.pointer_down(x + 40.0, y + 4.0);
        rt.pointer_up(x + 40.0, y + 4.0);
        rt.frame(0.016);
        rt.frame(0.016);
        Ok(())
    };
    press(&mut rt, "agents-0.row.docs")?;
    let tree = rt.elements();
    find(&tree, "agents-0.detail.docs")?;
    for label in ["Asked to", "Said last", "Touched", "Budget"] {
        assert!(shows(&tree, label), "{label} is not in the open row");
    }
    assert!(shows(&tree, "Stop") && shows(&tree, "More budget"));
    assert!(
        !shows(&tree, "not available yet"),
        "a row says so only once a button is pressed"
    );
    press(&mut rt, "agents-0.row.tests")?;
    let tree = rt.elements();
    assert!(
        find(&tree, "agents-0.detail.docs").is_err(),
        "one open at a time"
    );
    find(&tree, "agents-0.detail.tests")?;
    Ok(())
}

#[test]
fn a_live_row_is_found_as_a_thing_for_pick_and_right_click() -> Check {
    let shared = Hub::new(&[], Vec::new());
    let mut hub = shared.try_borrow_mut()?;
    setup("agents/live")?(&mut hub);
    let thing = (match DEF.finder {
        Some(yd_ui::overlay::pick::Finder::Keyed(find)) => find,
        _ => return Err("no keyed finder".into()),
    })(&hub, SCENE_PANE, "agents-0.row.tests")
    .ok_or("not found")?;
    assert_eq!(thing.label, "Tests · upload");
    assert_eq!(thing.address.to_string(), "yi://agent/tests");
    Ok(())
}

#[test]
fn a_pane_with_no_thread_says_so_instead_of_drawing_a_lone_row() -> Check {
    let rt = draw(setup("agents/empty")?)?;
    let tree = rt.elements();
    assert!(
        shows(&tree, "No thread yet"),
        "Yi is up and the window shows no thread"
    );
    assert!(
        tree.iter()
            .all(|e| !e.key.as_deref().is_some_and(|k| k.contains(".row.")))
    );
    Ok(())
}

#[test]
fn parents_in_a_ring_do_not_loop_the_depth() {
    let ring = |id: &str, parent: &str| {
        let mut row = Row::new(id)
            .text("agent", id)
            .text("tokens", "1.0K")
            .text("cost", "$0.10")
            .cell("state", Value::Dot(Dot::Work));
        row.parent = Some(RowId::new(parent));
        row
    };
    let sum = summarise(&table(vec![ring("x", "y"), ring("y", "x")]));
    assert_eq!((sum.rows, sum.depth, sum.tokens), (2, 1, 2_000));
    assert_eq!(
        super::fold::table(Vec::new()).len(),
        0,
        "an empty fold is an empty table"
    );
}

/// Yi's word for a run that failed is idle with an error stop: the thread's row says so.
#[test]
fn a_thread_whose_run_failed_reads_failed_not_done() -> Check {
    let shared = Hub::new(&[], Vec::new());
    let mut hub = shared.try_borrow_mut()?;
    setup("agents/live")?(&mut hub);
    let failed = r#""sessionUpdate":"state_update","state":"idle","stopReason":"_yi/error""#;
    scenes::say(&mut hub, &scenes::update(failed));
    let thread = yd_ui::shown::of_pane(&hub, SCENE_PANE).ok_or("no thread")?;
    let rows = rows(&hub.model, thread);
    let top = rows.first().ok_or("no rows")?;
    assert_eq!(top.get("state"), Some(&Value::Dot(Dot::Fail)));
    assert_eq!(top.words("doing_now"), Some("failed"));
    assert_eq!(summarise(&table(rows)).failed, 1);
    Ok(())
}

/// Record 5.3: the footer's legend is each model's letter, then its name.
#[test]
fn the_footer_names_each_model_letter() -> Check {
    let rt = draw(setup("agents/mockup")?)?;
    let tree = rt.elements();
    let foot = find(&tree, "agents-0.footer")?.rect;
    let inside = |e: &&ElementInfo| {
        let [x, y, w, h] = e.rect;
        y >= foot[1] && y + h <= foot[1] + foot[3] && x + w <= foot[0] + foot[2] / 2.0
    };
    let mut said: Vec<(f32, &str)> = tree
        .iter()
        .filter(inside)
        .filter_map(|e| Some((e.rect[0], e.text.as_deref()?)))
        .collect();
    said.sort_by(|a, b| a.0.total_cmp(&b.0));
    let said: Vec<&str> = said.into_iter().map(|(_, text)| text).collect();
    assert_eq!(said, ["O", "Opus", "S", "Sonnet", "H", "Haiku"]);
    Ok(())
}

#[test]
fn a_turn_waiting_on_an_approval_lists_the_threads_own_agent() -> Check {
    let rt = draw(setup("agents/working")?)?;
    let tree = rt.elements();
    assert!(
        !shows(&tree, "No agents in this thread"),
        "the pane is empty"
    );
    assert!(
        shows(&tree, "wants to run echo one > one.txt"),
        "no waiting row"
    );
    let own = |e: &&ElementInfo| {
        e.key
            .as_deref()
            .is_some_and(|k| k.starts_with("agents-0.row.thread:"))
    };
    assert!(
        tree.iter().any(|e| own(&e)),
        "no row for the thread's agent"
    );
    Ok(())
}

#[test]
fn a_step_lost_when_the_app_closed_does_not_read_as_needing_you() -> Check {
    const RESUME: &str = include_str!("../../../../../fixtures/wire/resume.jsonl");
    let lines: Vec<&str> = RESUME.lines().collect();
    let waits = lines.get(28).ok_or("no requires_action row")?;
    let second = lines.get(29..34).ok_or("no second connection")?;
    let shared = Hub::new(&[], Vec::new());
    let mut hub = shared.try_borrow_mut()?;
    for line in std::iter::once(waits).chain(second) {
        yd_core::wire::replay::play_line(&mut hub.model, line)?;
    }
    let thread = hub
        .model
        .wire
        .listed()
        .into_iter()
        .next()
        .ok_or("no thread")?;
    let rows = rows(&hub.model, thread);
    let top = rows.first().ok_or("no rows")?;
    assert_ne!(top.words("doing_now"), Some("waiting on you"));
    assert_eq!(summarise(&table(rows)).need, 0, "the headline counts it");
    Ok(())
}
