//! What a user sees and does in the Trace pane, drawn in process with no GPU.

use std::error::Error;

use rune::{ElementInfo, Env, Runtime};
use yd_core::ids::WindowId;
use yd_ui::pane::{PaneCx, PaneDef, SCENE_PANE};
use yd_ui::win::{Hub, Win};

use super::{DEF, scenes, tabs};

type Check = Result<(), Box<dyn Error>>;

fn by_key<'a>(tree: &'a [ElementInfo], key: &str) -> Result<&'a ElementInfo, Box<dyn Error>> {
    let found = tree
        .iter()
        .find(|element| !element.exiting && element.key.as_deref() == Some(key));
    found.ok_or_else(|| format!("no element keyed {key:?}").into())
}

fn rect(tree: &[ElementInfo], key: &str) -> Result<[f32; 4], Box<dyn Error>> {
    Ok(by_key(tree, key)?.rect)
}

/// The words drawn inside the keyed element, left to right, a space apart.
fn row_text(tree: &[ElementInfo], key: &str) -> Result<String, Box<dyn Error>> {
    Ok(words(tree, key)?.join(" "))
}

/// The words drawn inside the keyed element, in x order.
fn words<'a>(tree: &'a [ElementInfo], key: &str) -> Result<Vec<&'a str>, Box<dyn Error>> {
    let [x, y, w, h] = rect(tree, key)?;
    let inside = |at: &[f32; 4]| {
        let (cx, cy) = (at[0] + at[2] / 2.0, at[1] + at[3] / 2.0);
        cx >= x && cx <= x + w && cy >= y && cy <= y + h
    };
    let mut found: Vec<(f32, &str)> = tree
        .iter()
        .filter(|element| !element.exiting && inside(&element.rect))
        .filter_map(|element| Some((element.rect[0], element.text.as_deref()?)))
        .collect();
    found.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(found.into_iter().map(|(_, word)| word).collect())
}

/// An opened block holds exactly these words, in any order: its facts flow in a grid.
fn assert_words(tree: &[ElementInfo], key: &str, want: &[&str]) -> Check {
    let mut got = words(tree, key)?;
    let mut want = want.to_vec();
    got.sort_unstable();
    want.sort_unstable();
    if got == want {
        return Ok(());
    }
    Err(format!("block {key:?}\n  want: {want:?}\n  got:  {got:?}").into())
}

fn assert_row(tree: &[ElementInfo], key: &str, want: &str) -> Check {
    let got = row_text(tree, key)?;
    if got == want {
        return Ok(());
    }
    Err(format!("row {key:?}\n  want: {want:?}\n  got:  {got:?}").into())
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
        (600.0, 400.0),
        Env::new(true, 1.0),
    );
    for _ in 0..4 {
        rt.frame(0.016);
    }
    Ok(rt)
}

/// Whether a text or an accessible label shows `needle`, as the parity check reads them.
fn has(tree: &[ElementInfo], needle: &str) -> bool {
    let shows = |words: Option<&str>| words.is_some_and(|text| text.contains(needle));
    let live = tree.iter().filter(|element| !element.exiting);
    live.into_iter()
        .any(|element| shows(element.text.as_deref()) || shows(element.label.as_deref()))
}

#[test]
fn the_mockups_turn_reads_as_the_record_draws_it() -> Check {
    let tree = draw(scenes::mockup)?.elements();
    assert_row(
        &tree,
        "trace-0.pane-header",
        "Turn 2 14.8 s so far · $0.31 · 2 in parallel model tool child waiting",
    )?;
    assert_row(&tree, "trace-0.table-header", "Step Start Took 0 s 5 10 15")?;
    assert_row(&tree, "trace-0.row.model-plan", "model · plan 0 2.1 s")?;
    assert_row(
        &tree,
        "trace-0.row.read-upload-rs",
        "read upload.rs +2.1 250 ms",
    )?;
    Ok(())
}

#[test]
fn the_waiting_step_is_open_with_its_command_and_both_answers() -> Check {
    let tree = draw(scenes::mockup)?.elements();
    assert_words(
        &tree,
        "trace-0.detail.cargo-test-waiting-on-you",
        &[
            "Command",
            "cargo test -p api upload::",
            "Asked",
            "17:58:03",
            "Waiting",
            "0:42",
            "Blocks",
            "Tests · upload",
            "Runs in",
            "sandbox, no network",
            "Takes",
            "about 20 s",
            "Run it ↵",
            "Edit",
            "Deny",
            "Attach",
        ],
    )?;
    Ok(())
}

#[test]
fn a_press_on_a_closed_step_opens_it_in_place_and_closes_the_wait() -> Check {
    let mut rt = draw(scenes::mockup)?;
    let [x, y, _, _] = rect(&rt.elements(), "trace-0.row.model-plan")?;
    rt.pointer_down(x + 8.0, y + 4.0);
    rt.pointer_up(x + 8.0, y + 4.0);
    rt.frame(0.016);
    let tree = rt.elements();
    assert_words(
        &tree,
        "trace-0.detail.model-plan",
        &[
            "Model",
            "Opus 5.5 · High",
            "Read",
            "3.1K tokens",
            "Wrote",
            "212 tokens",
            "First word",
            "420 ms",
            "Cost",
            "$0.05",
            "Decided",
            "read, search, edit",
            "Show what it wrote",
            "Show its logs",
            "Attach",
        ],
    )?;
    assert!(
        by_key(&tree, "trace-0.detail.cargo-test-waiting-on-you").is_err(),
        "the wait stayed open beside it"
    );
    Ok(())
}

#[test]
fn a_recorded_turn_waiting_on_an_ask_draws_from_the_thread_alone() -> Check {
    let tree = draw(scenes::live)?.elements();
    assert_row(
        &tree,
        "trace-0.pane-header",
        "Turn 1 43.8 s so far · $0.00 model tool child waiting",
    )?;
    let wait = tree
        .iter()
        .filter_map(|element| element.key.as_deref())
        .find(|key| key.starts_with("trace-0.row.wait-"))
        .ok_or("no waiting step")?;
    let drawn = words(&tree, wait)?;
    for said in ["+1.6", "0:42…", "echo one > one.txt", "Run it ↵"] {
        assert!(drawn.contains(&said), "{said:?} is not in {drawn:?}");
    }
    Ok(())
}

#[test]
fn a_pane_with_no_turn_says_so() -> Check {
    let tree = draw(|_| {})?.elements();
    assert!(
        has(&tree, "Connecting to Yi"),
        "no thread, and no word from Yi yet"
    );
    Ok(())
}

#[test]
fn the_tab_counts_the_steps_and_says_how_many_ran_at_once() -> Check {
    static DEFS: [&PaneDef; 1] = [&DEF];
    let hub = Hub::new(&DEFS, Vec::new());
    scenes::mockup(&mut *hub.try_borrow_mut()?);
    let hub = hub.try_borrow()?;
    let pane = PaneCx {
        hub: &hub,
        pane: SCENE_PANE,
        focused: true,
        edits: None,
    };
    let tab = tabs(&pane).tabs.into_iter().next().ok_or("no tab")?;
    assert_eq!(
        (tab.count, tab.meta.as_str()),
        (Some(11), "11 spans · 2 parallel")
    );
    Ok(())
}
