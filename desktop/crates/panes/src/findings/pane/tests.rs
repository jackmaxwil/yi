//! What a user triaging findings would notice if it broke, drawn in process with no GPU.

use std::error::Error;

use rune::{Cx, El, ElementInfo, Env, Runtime, col};
use yd_core::command::by_kind::{FINDING_ASSIGN, FINDING_DISMISS};
use yd_core::findings::state::Stage;
use yd_core::findings::triage::Decision;
use yd_core::findings::view::{Scope, findings};
use yd_core::ids::{CommandId, ThreadId, WindowId};
use yd_ui::menu::command_box::install;
use yd_ui::pane::{PaneCx, PaneDef, SCENE_PANE};
use yd_ui::win::{Hub, Win};

use super::scenes::{OPEN, SCENES, THREAD, goal, thread};
use super::{DEF, Form, View, carry};
use crate::findings::desc::{
    ACCEPT, ACCEPT_ALL, ALLOW_TEST, ASK_PROOF, FIX_NOW, MERGE, NOT_A_PROBLEM,
};

type Check = Result<(), Box<dyn Error>>;

const CARRIED: &[CommandId] = &[
    ACCEPT,
    ACCEPT_ALL,
    ALLOW_TEST,
    ASK_PROOF,
    FIX_NOW,
    MERGE,
    NOT_A_PROBLEM,
    FINDING_ASSIGN,
    FINDING_DISMISS,
];
const KEY: &str = "findings-0";
const UNDERFLOW: &str = "Bucket::take can underflow when the clock steps back";

fn root(win: &Win, cx: &mut Cx<Win>) -> El {
    let Ok(hub) = win.hub.try_borrow() else {
        return col();
    };
    let pane = PaneCx {
        hub: &hub,
        pane: SCENE_PANE,
        focused: true,
        edits: Some(&win.edits),
    };
    (DEF.view)(&pane, cx)
}

fn scene(name: &str) -> Result<fn(&mut Hub), Box<dyn Error>> {
    let found = SCENES.iter().find(|scene| scene.name == name);
    Ok(found.ok_or_else(|| format!("no scene {name}"))?.setup)
}

/// The pane alone in a window that carries its commands, as the app's window does.
fn draw(setup: impl FnOnce(&mut Hub)) -> Result<Runtime<Win>, Box<dyn Error>> {
    static DEFS: [&PaneDef; 1] = [&DEF];
    let hub = Hub::new(&DEFS, Vec::new());
    {
        let mut hub = hub.try_borrow_mut()?;
        install(&mut hub, CARRIED, carry);
        setup(&mut hub);
    }
    let mut rt = Runtime::new(
        Win::new(hub, WindowId(0)),
        root,
        (1100.0, 760.0),
        Env::new(true, 1.0),
    );
    settle(&mut rt);
    Ok(rt)
}

fn settle(rt: &mut Runtime<Win>) {
    for _ in 0..4 {
        rt.frame(0.016);
    }
}

fn live(tree: &[ElementInfo]) -> impl Iterator<Item = &ElementInfo> {
    tree.iter().filter(|e| !e.exiting)
}

fn has_key(rt: &Runtime<Win>, key: &str) -> bool {
    live(&rt.elements()).any(|e| e.key.as_deref() == Some(key))
}

fn press(rt: &mut Runtime<Win>, key: &str) -> Check {
    let tree = rt.elements();
    let hit = live(&tree).find(|e| e.key.as_deref() == Some(key));
    let [x, y, w, h] = hit.ok_or_else(|| format!("no {key}"))?.rect;
    let (x, y) = (x + w / 2.0, y + h / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
    Ok(())
}

/// The words inside the keyed element, left to right and top to bottom.
fn words_in(rt: &Runtime<Win>, key: &str) -> Result<String, Box<dyn Error>> {
    let tree = rt.elements();
    let outer = live(&tree).find(|e| e.key.as_deref() == Some(key));
    let [x, y, w, h] = outer.ok_or_else(|| format!("no {key}"))?.rect;
    let inside = |e: &&ElementInfo| {
        let [ex, ey, _, _] = e.rect;
        ex >= x && ey >= y && ex < x + w && ey < y + h
    };
    let mut said: Vec<_> = live(&tree)
        .filter(inside)
        .filter_map(|e| Some((e.rect[1], e.rect[0], e.text.clone()?)))
        .collect();
    said.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    Ok(said
        .into_iter()
        .map(|(_, _, t)| t)
        .collect::<Vec<_>>()
        .join(" "))
}

fn hub_form(hub: &mut Hub) {
    if let Some(view) = hub.panes.get_mut::<View>(Some(SCENE_PANE)) {
        view.form = Form::Hub;
        view.project = true;
    }
}

fn line(hub: &mut Hub, line: &str) {
    let _ = yd_core::wire::replay::play_line(&mut hub.model, line);
}

/// The agent's goal raised two suggestions: a High one that breaks its check, and a Med one.
fn raised(hub: &mut Hub) {
    thread(hub);
    line(
        hub,
        &goal(&[(UNDERFLOW, true), ("Uploads have no limit", false)]),
    );
    hub_form(hub);
}

fn id_of(rt: &Runtime<Win>, title: &str) -> Result<String, Box<dyn Error>> {
    let hub = rt.state.hub.try_borrow()?;
    let all = findings(&hub.model, &ThreadId::new(THREAD), Scope::Project);
    let found = all.into_iter().find(|finding| finding.title == title);
    Ok(found
        .ok_or_else(|| format!("no finding {title}"))?
        .id
        .as_str()
        .to_owned())
}

fn stage_of(rt: &Runtime<Win>, title: &str) -> Result<Option<Stage>, Box<dyn Error>> {
    let hub = rt.state.hub.try_borrow()?;
    let all = findings(&hub.model, &ThreadId::new(THREAD), Scope::Project);
    Ok(all.into_iter().find(|f| f.title == title).map(|f| f.stage))
}

#[test]
fn accepting_a_suggestion_moves_it_out_of_triage_into_the_open_group() -> Check {
    let mut rt = draw(raised)?;
    let id = id_of(&rt, UNDERFLOW)?;
    let button = format!("{KEY}.triage.{id}.{}", ACCEPT.0);
    let head = words_in(&rt, &format!("{KEY}.triage"))?;
    assert!(head.starts_with("2 new from agents"), "{head}");
    press(&mut rt, &button)?;
    assert_eq!(stage_of(&rt, UNDERFLOW)?, Some(Stage::Open));
    assert!(!has_key(&rt, &button), "it left the triage box");
    let head = words_in(&rt, &format!("{KEY}.triage"))?;
    assert!(head.starts_with("1 new from agents"), "{head}");
    let group = words_in(&rt, &format!("{KEY}.group.{id}"))?;
    assert_eq!(group, "Open 1");
    Ok(())
}

#[test]
fn an_opened_finding_dismissed_from_its_detail_leaves_the_list() -> Check {
    let mut rt = draw(raised)?;
    let id = id_of(&rt, UNDERFLOW)?;
    press(&mut rt, &format!("{KEY}.triage.{id}.{}", ACCEPT.0))?;
    press(&mut rt, &format!("{KEY}.row.{id}"))?;
    let detail = format!("{KEY}.detail.{id}");
    let words = words_in(&rt, &detail)?;
    for button in ["Give it to an agent", "Ask for proof", "Attach", "Dismiss"] {
        assert!(words.contains(button), "{button} missing from {words}");
    }
    press(&mut rt, &format!("{detail}.{}", FINDING_DISMISS.0))?;
    let hub = rt.state.hub.try_borrow()?;
    let decided = hub
        .model
        .findings
        .triage
        .decision(&yd_core::findings::triage::FindingId::new(&id));
    assert!(
        matches!(decided, Some(Decision::Dismissed { .. })),
        "{decided:?}"
    );
    drop(hub);
    assert!(!has_key(&rt, &format!("{KEY}.row.{id}")));
    Ok(())
}

#[test]
fn a_button_nothing_carries_says_so_beside_the_buttons() -> Check {
    let mut rt = draw(scene("findings/mockup")?)?;
    let detail = format!("{KEY}.detail.{OPEN}");
    press(&mut rt, &format!("{detail}.finding.send-to-tracker"))?;
    let words = words_in(&rt, &detail)?;
    assert!(
        words.contains("Send to tracker is not available yet"),
        "{words}"
    );
    Ok(())
}

#[test]
fn pressing_a_place_lists_only_its_findings_and_pressing_it_again_lists_all() -> Check {
    let mut rt = draw(scene("findings/mockup")?)?;
    let docs = format!("{KEY}.row.docs-still-describe-the-old-upload-limits");
    let limit = format!("{KEY}.row.limit-is-keyed-by-user-id-only-anonymous-uploads-are-unlimited");
    assert!(has_key(&rt, &docs));
    press(&mut rt, &format!("{KEY}.place.rate_limit.rs"))?;
    assert!(has_key(&rt, &limit), "rate_limit.rs:41 is in rate_limit.rs");
    assert!(has_key(&rt, &format!("{KEY}.row.{OPEN}")));
    assert!(!has_key(&rt, &docs), "docs/uploads.md is another place");
    press(&mut rt, &format!("{KEY}.place.rate_limit.rs"))?;
    assert!(has_key(&rt, &docs));
    Ok(())
}

/// A todo the agent finished, with its check red or green, titled as frame 10's High row.
fn todo(last: &str) -> String {
    let list = format!(
        r#"{{"phases":[{{"name":"Limit uploads","items":[{{"label":"{UNDERFLOW}","state":"done","check":{{"command":"cargo test clock_steps_back","last":"{last}"}}}}]}}],"nextId":2}}"#
    );
    format!(
        r#"{{"dir":"recv","frame":{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"{THREAD}","update":{{"sessionUpdate":"_yi/todo","list":{list}}}}}}}}}"#
    )
}

#[test]
fn the_gate_blocks_while_a_high_finding_lacks_a_passed_check() -> Check {
    let mut rt = draw(|hub| {
        thread(hub);
        line(hub, &todo("red:9f2c"));
        hub_form(hub);
    })?;
    let gate = format!("{KEY}.gate.verdict");
    assert!(
        words_in(&rt, &gate)?.starts_with("Ready"),
        "a suggestion never blocks"
    );
    let id = id_of(&rt, UNDERFLOW)?;
    press(&mut rt, &format!("{KEY}.triage.{id}.{}", ACCEPT.0))?;
    assert_eq!(words_in(&rt, &gate)?, "Not yet 1 thing stands in the way");
    let high = words_in(&rt, &format!("{KEY}.gate.check.0"))?;
    assert_eq!(high, "No High findings open 1 open");
    rt.update(|win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            line(&mut hub, &todo("green"));
        }
    });
    settle(&mut rt);
    assert!(
        words_in(&rt, &gate)?.starts_with("Ready"),
        "its check passed"
    );
    Ok(())
}

#[test]
fn the_compact_list_counts_new_findings_as_open() -> Check {
    let rt = draw(scene("findings-table/mockup")?)?;
    let head = words_in(&rt, &format!("{KEY}.pane-header"))?;
    assert!(
        head.starts_with("7 open High 1 Med 3 Low 3 2 new · 2 being fixed"),
        "{head}"
    );
    let open = words_in(&rt, &format!("{KEY}.detail.{OPEN}"))?;
    assert!(open.contains("Allow the test"), "{open}");
    Ok(())
}

#[test]
fn a_narrow_findings_pane_drops_the_last_columns_and_keeps_each_title_readable() -> Check {
    let mut rt = draw(scene("findings-table/mockup")?)?;
    rt.set_viewport((300.0, 500.0), 1.0);
    settle(&mut rt);
    let heads = words_in(&rt, &format!("{KEY}.table-header"))?;
    let heads = heads.trim();
    assert_eq!(
        heads, "Finding State",
        "the columns that fit, the least important dropped"
    );
    let tree = rt.elements();
    let title = live(&tree).find(|e| e.text.as_deref() == Some(UNDERFLOW));
    let width = title.ok_or("no title drawn")?.rect[2];
    assert!(width >= 120.0, "the title keeps room to be read: {width}");
    Ok(())
}
