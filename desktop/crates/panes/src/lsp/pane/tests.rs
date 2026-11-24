//! What a person reading the servers and problems would notice if it broke, with no GPU.

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use rune::{Cx, El, Env, Runtime};
use yd_core::command::table::Target;
use yd_core::feed::{FeedEvent, SourceCmd};
use yd_core::ids::WindowId;
use yd_core::msg::{Effect, Msg, step};
use yd_core::pane::PaneKind;
use yd_core::row::Row;
use yd_ui::grid_view::view::alone;
use yd_ui::pane::PaneDef;
use yd_ui::win::{Hub, Win};

use super::scenes::{SCENES, problem_rows, server_rows};
use super::{DEF, FEED};

type Check = Result<(), Box<dyn Error>>;
type Sent = Rc<RefCell<Vec<Effect>>>;

fn view(win: &Win, cx: &mut Cx<Win>) -> El {
    let Ok(hub) = win.hub.try_borrow() else {
        return rune::col();
    };
    alone(win, &hub, cx, PaneKind::LanguageServers)
}

/// The scene `name` drawn on a pane 334 wide, as the grid gives it, with its effects kept.
fn draw(name: &str) -> Result<(Runtime<Win>, Sent), Box<dyn Error>> {
    static DEFS: [&PaneDef; 1] = [&DEF];
    let scene = SCENES
        .iter()
        .find(|scene| scene.name == name)
        .ok_or("no scene")?;
    let hub = Hub::new(&DEFS, Vec::new());
    let sent: Sent = Rc::default();
    let log = sent.clone();
    hub.try_borrow_mut()?
        .set_sink(move |effect| log.borrow_mut().push(effect));
    hub.try_borrow_mut()?.stage = scene.stage;
    (scene.setup)(&mut *hub.try_borrow_mut()?);
    let mut rt = Runtime::new(
        Win::new(hub, WindowId(0)),
        view,
        (334.0, 700.0),
        Env::new(true, 1.0),
    );
    for _ in 0..4 {
        rt.frame(0.016);
    }
    Ok((rt, sent))
}

fn keys(rt: &Runtime<Win>) -> Vec<String> {
    let live = rt.elements();
    live.iter()
        .filter(|e| !e.exiting)
        .filter_map(|e| e.key.clone())
        .collect()
}

fn count(rt: &Runtime<Win>, needle: &str) -> usize {
    keys(rt).iter().filter(|key| key.contains(needle)).count()
}

fn press(rt: &mut Runtime<Win>, key: &str) -> Check {
    let all = rt.elements();
    let hit = all
        .iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key))
        .ok_or(format!("no {key}"))?;
    let [x, y, w, h] = hit.rect;
    let (x, y) = (x + w / 2.0, y + h / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    for _ in 0..3 {
        rt.frame(0.016);
    }
    Ok(())
}

fn text_of(rt: &Runtime<Win>, key: &str) -> Option<String> {
    let all = rt.elements();
    let inside = |at: [f32; 4], e: [f32; 4]| {
        at[0] <= e[0] && e[0] <= at[0] + at[2] && at[1] <= e[1] && e[1] <= at[1] + at[3]
    };
    let host = all
        .iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key))?;
    let mut words: Vec<_> = all
        .iter()
        .filter(|e| !e.exiting && inside(host.rect, e.rect))
        .filter_map(|e| Some((e.rect[0], e.text.clone()?)))
        .collect();
    words.sort_by(|a, b| a.0.total_cmp(&b.0));
    Some(
        words
            .into_iter()
            .map(|(_, text)| text)
            .collect::<Vec<_>>()
            .join(" "),
    )
}

#[test]
fn the_servers_tab_lists_the_servers_and_the_problems_each_in_their_own_list() -> Check {
    let (rt, _) = draw("lsp/mockup")?;
    assert_eq!(count(&rt, "lsp-0.servers.row.server:"), 3);
    assert_eq!(count(&rt, "lsp-0.problems.row.problem:"), 4);
    let tab = |n: u32| text_of(&rt, &format!("lsp-0.tab-{n}.pane-tab"));
    assert_eq!(tab(1).as_deref(), Some("Language servers 3"));
    assert_eq!(tab(2).as_deref(), Some("Problems 4"));
    Ok(())
}

#[test]
fn the_problems_tab_lists_only_problems_and_groups_them_by_file() -> Check {
    let (mut rt, _) = draw("lsp/mockup")?;
    press(&mut rt, "lsp-0.tab-2.pane-tab")?;
    assert_eq!(count(&rt, ".servers.row."), 0);
    assert_eq!(count(&rt, "lsp-0.problems.row.problem:"), 4);
    let groups = keys(&rt)
        .into_iter()
        .filter(|key| key.contains(".problems.group."));
    let first: Vec<String> = groups.collect();
    let named = |file: &str| {
        first
            .iter()
            .any(|key| key.contains(&format!("problem:{file}:")))
    };
    assert_eq!(first.len(), 3);
    assert!(named("rate_limit.rs") && named("tests/upload.rs") && named("upload.rs"));
    Ok(())
}

#[test]
fn a_fix_the_server_offered_is_joined_to_the_problem_it_is_for() -> Check {
    let (mut rt, sent) = draw("problems/mockup")?;
    let fix = Row::new("fix:rate_limit.rs:20:0").text("title", "Use saturating_duration_since");
    rt.update(|win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            step(&mut hub.model, Msg::Feed(FEED, FeedEvent::Upsert(fix)));
        }
    });
    for _ in 0..3 {
        rt.frame(0.016);
    }
    let key = "lsp-0.problems.detail.problem:rate_limit.rs:20:0.lsp.apply-fix";
    press(&mut rt, key)?;
    let asked: Vec<(String, String)> = sent
        .borrow()
        .iter()
        .filter_map(|effect| match effect {
            Effect::Source(_, SourceCmd::Run(run)) => match &run.target {
                Target::Thing(at) => Some((run.command.0.to_owned(), at.id().to_owned())),
                Target::App | Target::Pane(_) => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        asked,
        [(
            "lsp.apply-fix".to_owned(),
            "fix:rate_limit.rs:20:0".to_owned()
        )]
    );
    Ok(())
}

#[test]
fn an_open_servers_longest_label_is_drawn_whole() -> Check {
    let (rt, _) = draw("lsp/mockup")?;
    let said = "Agent may ask it";
    let tree = rt.elements();
    let label = tree.iter().find(|e| e.text.as_deref() == Some(said));
    let width = label.ok_or("no such label")?.rect[2];
    let needs = rune::text::advance(said, yd_ui::theme::look::META);
    assert!(width >= needs, "{width} wide, the words need {needs}");
    Ok(())
}

#[test]
fn restart_on_an_open_server_asks_the_source_for_that_server_alone() -> Check {
    let (mut rt, sent) = draw("lsp/mockup")?;
    press(
        &mut rt,
        "lsp-0.servers.detail.server:rust-analyzer.lsp.restart",
    )?;
    let sent = sent.borrow();
    let run = sent
        .iter()
        .find_map(|effect| match effect {
            Effect::Source(_, SourceCmd::Run(run)) => Some(run),
            _ => None,
        })
        .ok_or("nothing was asked of the source")?;
    let Target::Thing(at) = &run.target else {
        return Err("Restart was not aimed at a server".into());
    };
    assert_eq!(
        (run.command.0, at.id()),
        ("lsp.restart", "server:rust-analyzer")
    );
    Ok(())
}

#[test]
fn rows_of_other_prefixes_never_reach_either_list() -> Check {
    let (mut rt, _) = draw("lsp/mockup")?;
    rt.update(|win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            let answers = [
                "hover:a.rs:1:1",
                "ref:a.rs:1:1:0",
                "hint:a.rs:1:1",
                "def:a.rs:1:1",
            ];
            for id in answers {
                step(
                    &mut hub.model,
                    Msg::Feed(FEED, FeedEvent::Upsert(Row::new(id))),
                );
            }
        }
    });
    for _ in 0..3 {
        rt.frame(0.016);
    }
    assert_eq!(count(&rt, "lsp-0.servers.row."), server_rows().len());
    assert_eq!(count(&rt, "lsp-0.problems.row."), problem_rows().len());
    Ok(())
}

fn stage(win: &Win, cx: &mut Cx<Win>) -> El {
    let Ok(hub) = win.hub.try_borrow() else {
        return rune::col();
    };
    match hub.stage {
        yd_ui::pane::Stage::View(build) => build(win, &hub, cx),
        yd_ui::pane::Stage::App | yd_ui::pane::Stage::Pane(_) => rune::col(),
    }
}

/// The code viewer scene drawn on a pane `width` wide; the pane table holds the code pane so the
/// scene has its tabs.
fn code(width: f32) -> Result<(Runtime<Win>, Sent), Box<dyn Error>> {
    static DEFS: [&PaneDef; 2] = [&DEF, &crate::code_view::DEF];
    let scene = SCENES
        .iter()
        .find(|scene| scene.name == "lsp-code/mockup")
        .ok_or("no scene")?;
    let hub = Hub::new(&DEFS, Vec::new());
    let sent: Sent = Rc::default();
    let log = sent.clone();
    hub.try_borrow_mut()?
        .set_sink(move |effect| log.borrow_mut().push(effect));
    hub.try_borrow_mut()?.stage = scene.stage;
    (scene.setup)(&mut *hub.try_borrow_mut()?);
    let mut rt = Runtime::new(
        Win::new(hub, WindowId(0)),
        stage,
        (width, 700.0),
        Env::new(true, 1.0),
    );
    for _ in 0..4 {
        rt.frame(0.016);
    }
    Ok((rt, sent))
}

fn rect(rt: &Runtime<Win>, key: &str) -> Result<[f32; 4], Box<dyn Error>> {
    let all = rt.elements();
    let found = all
        .iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key));
    Ok(found.ok_or(format!("no {key}"))?.rect)
}

#[test]
fn the_hover_card_sits_under_the_token_it_rests_on_and_over_the_lines_below() -> Check {
    let (rt, _) = code(1200.0)?;
    let (line, code, card) = (
        rect(&rt, "code-0.L4")?,
        rect(&rt, "code-0.row.L4")?,
        rect(&rt, "code-0.hover")?,
    );
    assert!(card[1] >= line[1] + line[3], "the card starts below line 4");
    let before = "const WINDOW: Duration = Duration::";
    let token_at = code[0] + rune::text::advance(before, yd_ui::theme::look::CODE_LINE);
    assert!(
        (card[0] - token_at).abs() < 1.0,
        "the card starts at the token"
    );
    let fifth = rect(&rt, "code-0.L5")?;
    assert!(card[1] < fifth[1] + fifth[3], "it covers the lines below");
    Ok(())
}

#[test]
fn only_one_card_shows_and_it_goes_when_the_pointer_leaves() -> Check {
    let (mut rt, _) = code(1200.0)?;
    assert_eq!(
        count(&rt, "code-0.hover"),
        1 + count(&rt, "code-0.hover.link-")
    );
    rt.update(|win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut()
            && let Some(kept) = hub.panes.get_mut::<crate::lsp::marks::Kept>(None)
        {
            kept.resting = None;
        }
    });
    for _ in 0..3 {
        rt.frame(0.016);
    }
    assert_eq!(count(&rt, "code-0.hover"), 0);
    Ok(())
}

#[test]
fn go_to_definition_on_the_card_asks_for_that_token() -> Check {
    let (mut rt, sent) = code(1200.0)?;
    press(&mut rt, "code-0.hover.link-definition")?;
    let sent = sent.borrow();
    let run = sent
        .iter()
        .find_map(|effect| match effect {
            Effect::Source(_, SourceCmd::Run(run)) => Some(run),
            _ => None,
        })
        .ok_or("nothing was asked")?;
    let Target::Thing(at) = &run.target else {
        return Err("not aimed at a place".into());
    };
    assert_eq!(run.command.0, "lsp.definition");
    assert_eq!(at.part_text(), Some("L4:C36"));
    Ok(())
}

#[test]
fn a_card_for_a_token_far_along_its_line_stays_inside_a_narrow_pane() -> Check {
    let (rt, _) = code(367.0)?;
    let card = rect(&rt, "code-0.hover")?;
    assert!(
        card[0] + card[2] <= 367.0,
        "the card reaches {}",
        card[0] + card[2]
    );
    Ok(())
}
