//! What a person designing an element would notice if it broke: the pane's rows and card, a
//! typed value, the handles over the page, Reset and Hand to agent. Drawn in process.

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, col};
use yd_core::command::by_kind::ELEMENT_SOURCE;
use yd_core::command::table::{Run, Target};
use yd_core::command::tools::design::{AT, ELEMENT, HAND, RESET, SET};
use yd_core::feed::{FeedEvent, SourceCmd};
use yd_core::ids::{RowId, WindowId};
use yd_core::msg::{Effect, Msg, step};
use yd_core::pane::PaneKind;
use yd_core::row::Row;
use yd_ui::menu::command_box;
use yd_ui::pane::{PaneDef, Stage};
use yd_ui::win::{Hub, Win};

use super::scenes::SCENES;
use super::{DEF, FEED, Mode, carry, mode};
use crate::design::handles::{fit, measured, resized, thing_at};
use crate::design::markup::{self, Marks};
use crate::test_support::tap;

type Check = Result<(), Box<dyn Error>>;
type Sent = Rc<RefCell<Vec<Effect>>>;

/// A thread the window shows, made the way a client sees one made.
const THREAD: [&str; 3] = [
    r#"{"dir":"recv","frame":{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":2,"info":{"name":"yi","version":"0.1041.0"}}}}"#,
    r#"{"dir":"send","frame":{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/work/atlas-api","mcpServers":[]}}}"#,
    r#"{"dir":"recv","frame":{"jsonrpc":"2.0","id":2,"result":{"sessionId":"t-1"}}}"#,
];

fn root(win: &Win, cx: &mut Cx<Win>) -> El {
    let Ok(hub) = win.hub.try_borrow() else {
        return col();
    };
    match hub.stage {
        Stage::View(view) => view(win, &hub, cx),
        Stage::App | Stage::Pane(_) => {
            yd_ui::grid_view::view::alone(win, &hub, cx, PaneKind::Design)
        }
    }
}

/// The scene `name` drawn, with every effect its window asks for from then on. `thread`
/// gives the window a thread to prompt.
fn draw(name: &str, thread: bool) -> Result<(Runtime<Win>, Sent), Box<dyn Error>> {
    static DEFS: [&PaneDef; 2] = [&DEF, &crate::web::DEF];
    let scene = SCENES.iter().find(|scene| scene.name == name);
    let scene = scene.ok_or_else(|| format!("no scene {name}"))?;
    let hub = Hub::new(&DEFS, Vec::new());
    let sent: Sent = Rc::default();
    let into = sent.clone();
    {
        let mut hub = hub.try_borrow_mut()?;
        hub.stage = scene.stage;
        if thread {
            for line in THREAD {
                yd_core::wire::replay::play_line(&mut hub.model, line)?;
            }
        }
        (scene.setup)(&mut hub);
        command_box::install(&mut hub, &[], carry);
        hub.set_sink(move |effect| into.borrow_mut().push(effect));
    }
    let mut rt = Runtime::new(
        Win::new(hub, WindowId(0)),
        root,
        (700.0, 760.0),
        Env::new(true, 1.0),
    );
    rest(&mut rt);
    Ok((rt, sent))
}

fn rest(rt: &mut Runtime<Win>) {
    for _ in 0..4 {
        rt.frame(0.016);
    }
}

fn rect(rt: &Runtime<Win>, key: &str) -> Result<[f32; 4], Box<dyn Error>> {
    let all = rt.elements();
    let mut live = all.iter().filter(|e| !e.exiting);
    let hit = live.find(|e| e.key.as_deref() == Some(key));
    Ok(hit.ok_or_else(|| format!("no {key}"))?.rect)
}

fn press(rt: &mut Runtime<Win>, key: Key, cmd: bool) {
    let mods = Mods {
        cmd,
        logo: cmd,
        ..Mods::default()
    };
    rt.key(KeyEvent { key, mods });
    rt.frame(0.016);
}

/// The words drawn inside the element keyed `key`, top to bottom then left to right.
fn words(rt: &Runtime<Win>, key: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let [x, y, w, h] = rect(rt, key)?;
    let all = rt.elements();
    let inside = |r: &[f32; 4]| {
        r[0] >= x - 0.5
            && r[1] >= y - 0.5
            && r[0] + r[2] <= x + w + 0.5
            && r[1] + r[3] <= y + h + 0.5
    };
    let mut said: Vec<_> = all
        .iter()
        .filter(|e| !e.exiting && inside(&e.rect))
        .filter_map(|e| Some((e.rect, e.text.clone()?)))
        .collect();
    let place = |r: &[f32; 4]| ((r[1] + r[3] / 2.0).round() as i64, r[0].round() as i64);
    said.sort_by_key(|(rect, _)| place(rect));
    Ok(said.into_iter().map(|(_, text)| text).collect())
}

/// What the window asked of the page's source, as `command text` lines.
fn asked(sent: &Sent) -> Vec<String> {
    let all = sent.borrow();
    let runs = all.iter().filter_map(|effect| match effect {
        Effect::Source(feed, SourceCmd::Run(run)) if *feed == FEED => Some(run),
        _ => None,
    });
    runs.map(|run| format!("{} {}", run.command.0, run.text.as_deref().unwrap_or("")))
        .collect()
}

fn type_value(rt: &mut Runtime<Win>, slug: &str, typed: &str) -> Check {
    tap(rt, &format!("design-0.value.{slug}"), rest)?;
    press(rt, Key::Command('a'), true);
    press(rt, Key::Text(typed.to_owned()), false);
    press(rt, Key::Enter, false);
    rest(rt);
    Ok(())
}

#[test]
fn four_edits_make_the_four_lines_of_the_changes_card_in_the_mockups_words() -> Check {
    let (rt, _) = draw("design/mockup", false)?;
    assert_eq!(
        words(&rt, "design-0.changes")?,
        [
            "4 changes",
            "live in the preview only",
            "fill",
            "#fdecec → #fff4e5",
            "text colour",
            "#9c2424 → #8a4b00",
            "corners",
            "6 → 10",
            "words",
            "\"Too many uploads…\" → \"Slow down…\"",
        ]
    );
    assert_eq!(
        words(&rt, "design-0.row.fill")?,
        ["Fill", "#fff4e5", "#fdecec"],
        "a changed value is followed by its old one"
    );
    assert_eq!(words(&rt, "design-0.row.size")?, ["Size", "602 × 30"]);
    assert_eq!(words(&rt, "design-0.row.padding")?, ["Padding", "7", "10"]);
    Ok(())
}

#[test]
fn a_bad_colour_is_refused_in_place_and_a_good_one_is_asked_of_the_page() -> Check {
    let (mut rt, sent) = draw("design/mockup", false)?;
    type_value(&mut rt, "fill", "tomato")?;
    assert_eq!(asked(&sent), Vec::<String>::new(), "nothing is applied");
    let refused = words(&rt, "design-0.refused")?;
    assert_eq!(refused, ["“tomato” was not applied: a colour is #rrggbb"]);
    assert!(rect(&rt, "design-0.field").is_ok(), "the field stays open");

    press(&mut rt, Key::Command('a'), true);
    press(&mut rt, Key::Text("#FFE0B2".to_owned()), false);
    press(&mut rt, Key::Enter, false);
    rest(&mut rt);
    assert_eq!(asked(&sent), ["design.set fill=#ffe0b2"]);
    assert!(rect(&rt, "design-0.refused").is_err());
    assert!(rect(&rt, "design-0.field").is_err(), "the field closes");

    type_value(&mut rt, "corners", "9999")?;
    type_value(&mut rt, "size", "0 x 4")?;
    assert_eq!(asked(&sent).len(), 1, "a number out of range is not asked");
    Ok(())
}

#[test]
fn reset_asks_the_page_and_the_card_goes_when_the_change_rows_do() -> Check {
    let (mut rt, sent) = draw("design/mockup", false)?;
    tap(&mut rt, "design-0.reset", rest)?;
    assert_eq!(asked(&sent), [format!("{} ", RESET.0)]);
    // The source answers a reset by taking its change rows away.
    rt.update(|win, _| {
        let Ok(mut hub) = win.hub.try_borrow_mut() else {
            return;
        };
        for slug in ["fill", "text", "corners", "says"] {
            let gone = FeedEvent::Remove(RowId::new(format!("change:0:{slug}")));
            step(&mut hub.model, Msg::Feed(FEED, gone));
        }
    });
    rest(&mut rt);
    assert!(
        rect(&rt, "design-0.changes").is_err(),
        "the card is empty and gone"
    );
    let hub = rt.state.hub.try_borrow()?;
    assert!(super::changes::list(hub.model.tables.get(FEED)).is_empty());
    Ok(())
}

#[test]
fn hand_to_agent_sends_one_prompt_with_the_list_and_two_attached_things() -> Check {
    let (mut rt, sent) = draw("design/mockup", true)?;
    tap(&mut rt, "design-0.hand", rest)?;
    let all = sent.borrow();
    let prompts: Vec<&str> = all
        .iter()
        .filter_map(|effect| match effect {
            Effect::Wire(request) => Some(request.line.as_str()),
            _ => None,
        })
        .filter(|line| line.contains("session/prompt"))
        .collect();
    let [prompt] = prompts.as_slice() else {
        return Err(format!("{} prompts were sent: {prompts:?}", prompts.len()).into());
    };
    for said in [
        "div.alert on localhost:5173/upload, drawn at src/pages/Upload.tsx:41",
        "- fill: #fdecec → #fff4e5",
        "- text colour: #9c2424 → #8a4b00",
        "- corners: 6 → 10",
        r#"- words: \"Too many uploads. Try again in 41 seconds.\" → \"Slow down. Try again in 41 seconds.\""#,
        r#""type":"resource_link","uri":"src/pages/Upload.tsx#L41""#,
    ] {
        assert!(prompt.contains(said), "{said} is not in {prompt}");
    }
    let attached = prompt.matches("Attached: ").count() + prompt.matches("resource_link").count();
    assert_eq!(attached, 2, "the element and the lines that draw it");
    let hub = rt.state.hub.try_borrow()?;
    assert!(hub.model.attached.things.is_empty(), "they went with it");
    Ok(())
}

#[test]
fn hand_to_agent_with_nothing_changed_sends_nothing() -> Check {
    let (mut rt, sent) = draw("design/empty", true)?;
    rt.update(|win, tx| {
        command_box::run(win, tx, Run::new(HAND));
    });
    rest(&mut rt);
    let wired = sent.borrow().iter().any(|e| matches!(e, Effect::Wire(_)));
    assert!(!wired);
    Ok(())
}

#[test]
fn the_box_sits_on_the_element_with_eight_handles_its_size_and_both_guides() -> Check {
    let (rt, _) = draw("design/box", false)?;
    let page = rect(&rt, "web-0.page")?;
    let frame = rect(&rt, "web-0.design.element")?;
    let near = |a: f32, b: f32| (a - b).abs() < 0.6;
    assert!(
        near(frame[0], page[0] + 16.0 - 3.0),
        "{frame:?} over {page:?}"
    );
    assert!(
        near(frame[1], page[1] + 40.0 - 3.0),
        "{frame:?} over {page:?}"
    );
    assert!(near(frame[2], 608.0) && near(frame[3], 36.0), "{frame:?}");
    let alert = rect(&rt, "web-0.page.alert")?;
    assert!(near(alert[0], frame[0] + 3.0) && near(alert[1], frame[1] + 3.0));
    for name in ["nw", "n", "ne", "e", "se", "s", "sw", "w"] {
        let handle = rect(&rt, &format!("web-0.design.handle.{name}"))?;
        assert!(
            near(handle[2], 7.0) && near(handle[3], 7.0),
            "{name} {handle:?}"
        );
    }
    let se = rect(&rt, "web-0.design.handle.se")?;
    assert!(near(se[0] + 3.5, frame[0] + frame[2]) && near(se[1] + 3.5, frame[1] + frame[3]));
    assert_eq!(words(&rt, "web-0.design.size")?, ["602 × 30"]);
    let badge = rect(&rt, "web-0.design.size")?;
    assert!(near(badge[0] + badge[2], frame[0] + frame[2]));
    assert!(near(badge[1], frame[1] + frame[3] + 7.0));
    assert_eq!(words(&rt, "web-0.design.gap")?, ["10"]);
    assert_eq!(words(&rt, "web-0.design.inset")?, ["16"]);
    assert_eq!(words(&rt, "web-0.design.element.number")?, ["1"]);
    assert_eq!(words(&rt, "web-0.design.element.tag")?, ["div.alert"]);
    assert_eq!(words(&rt, "web-0.mode")?, ["Browse", "Pick", "Design"]);
    Ok(())
}

#[test]
fn a_guide_under_three_is_not_drawn() -> Check {
    let row = |gap: &str| Row::new(ELEMENT).text("gap", gap);
    assert_eq!(measured(&row("2"), "gap"), None);
    assert_eq!(measured(&row("3"), "gap"), Some(3));
    assert_eq!(measured(&row("-9"), "gap"), None);
    assert_eq!(measured(&row("ten"), "gap"), None);

    let (mut rt, _) = draw("design/box", false)?;
    rt.update(|win, _| {
        let Ok(mut hub) = win.hub.try_borrow_mut() else {
            return;
        };
        let table = hub.model.tables.get(FEED);
        let element = table.and_then(|t| t.find(&RowId::new(ELEMENT)).cloned());
        if let Some(element) = element {
            let near = element.text("gap", "2").text("inset", "0");
            step(&mut hub.model, Msg::Feed(FEED, FeedEvent::Upsert(near)));
        }
    });
    rest(&mut rt);
    assert!(rect(&rt, "web-0.design.element").is_ok());
    assert!(
        rect(&rt, "web-0.design.gap").is_err(),
        "a gap of 2 has no guide"
    );
    assert!(rect(&rt, "web-0.design.inset").is_err());
    Ok(())
}

#[test]
fn dragging_a_handle_asks_the_page_for_the_size_the_pointer_reached() -> Check {
    assert_eq!(resized((602, 30), (40.0, 9.0), (1.0, 1.0), 1.0), (642, 39));
    assert_eq!(resized((602, 30), (40.0, 9.0), (1.0, 0.5), 1.0), (642, 30));
    assert_eq!(
        resized((602, 30), (-40.0, -9.0), (0.0, 0.0), 2.0),
        (622, 35)
    );
    assert_eq!(resized((602, 30), (-9e9, 9e9), (1.0, 1.0), 1.0), (1, 8192));

    let (mut rt, sent) = draw("design/box", false)?;
    let [x, y, w, h] = rect(&rt, "web-0.design.handle.se")?;
    let (x, y) = (x + w / 2.0, y + h / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_move(x + 20.0, y + 6.0);
    rt.pointer_move(x + 38.0, y + 6.0);
    rt.pointer_move(x + 38.0, y + 6.0);
    rt.pointer_up(x + 38.0, y + 6.0);
    rest(&mut rt);
    assert_eq!(
        asked(&sent),
        ["design.set size=622 × 36", "design.set size=640 × 36"],
        "one ask a new size, none for a move that changes nothing"
    );
    let hub = rt.state.hub.try_borrow()?;
    assert_eq!(mode(&hub).and_then(|mode| mode.grip), None);
    Ok(())
}

#[test]
fn in_design_mode_a_press_picks_once_and_the_page_gets_no_other_input() -> Check {
    let (mut rt, sent) = draw("design/box", false)?;
    let [x, y, _, _] = rect(&rt, "web-0.page")?;
    let click = |rt: &mut Runtime<Win>| {
        rt.pointer_move(x + 100.0, y + 200.0);
        rt.pointer_down(x + 100.0, y + 200.0);
        rt.pointer_up(x + 100.0, y + 200.0);
        rest(rt);
    };
    click(&mut rt);
    let screened = |sent: &Sent| {
        let all = sent.borrow();
        all.iter()
            .any(|e| matches!(e, Effect::Source(_, SourceCmd::Screen(_))))
    };
    assert!(
        asked(&sent).is_empty() && !screened(&sent),
        "an element is picked already"
    );

    tap(&mut rt, "web-0.mode.2", rest)?;
    rt.update(|win, tx| {
        command_box::run(win, tx, Run::new(yd_core::command::tools::design::PICK));
    });
    rest(&mut rt);
    click(&mut rt);
    click(&mut rt);
    assert_eq!(
        asked(&sent),
        [format!("{} 200,400", AT.0)],
        "in the picture's pixels"
    );
    assert!(!screened(&sent));

    tap(&mut rt, "web-0.mode.0", rest)?;
    click(&mut rt);
    assert!(screened(&sent), "in Browse the press is the page's again");
    assert!(rect(&rt, "web-0.design.element").is_err());
    Ok(())
}

#[test]
fn the_fit_of_an_element_follows_the_picture_and_refuses_numbers_that_do_not_read() -> Check {
    let row = Row::new(ELEMENT)
        .text("x", "32")
        .text("y", "80")
        .text("w", "1204")
        .text("h", "60")
        .text("ratio", "2");
    let found = fit(&row, (1268, 1316), [634.0, 658.0]).ok_or("no fit")?;
    assert_eq!((found.rect, found.per), ([16.0, 40.0, 602.0, 30.0], 1.0));
    let half = fit(&row, (1268, 1316), [317.0, 658.0]).ok_or("no fit")?;
    assert_eq!((half.rect[2], half.per), (301.0, 0.5));
    assert_eq!(
        fit(&row.clone().text("x", "NaN"), (1268, 1316), [634.0, 658.0]),
        None
    );
    assert_eq!(
        fit(&row.text("ratio", "0"), (1268, 1316), [634.0, 658.0]),
        None
    );
    Ok(())
}

#[test]
fn a_marked_region_wears_its_number_and_its_note_and_goes_with_the_prompt() -> Check {
    let (rt, _) = draw("design/mark-up", false)?;
    let frame = rect(&rt, "design-0.screen.mark.0")?;
    let page = rect(&rt, "design-0.screen")?;
    let near = |a: f32, b: f32| (a - b).abs() < 0.6;
    assert!(
        near(frame[0], page[0] + 22.0 - 3.0) && near(frame[2], 156.0),
        "{frame:?}"
    );
    assert_eq!(words(&rt, "design-0.screen.mark.0.number")?, ["1"]);
    let note = rect(&rt, "design-0.screen.mark.0.note")?;
    assert!(near(note[0], frame[0] + frame[2] + 9.0) && near(note[1], frame[1] - 3.0));
    assert!(near(note[2], 136.0));
    assert_eq!(
        words(&rt, "design-0.screen.mark.0.note")?,
        ["Say when the retry will happen"]
    );
    assert!(
        rect(&rt, "design-0.screen.mark.0.tag").is_err(),
        "no label at rest"
    );
    let hub = rt.state.hub.try_borrow()?;
    let blocks = yd_core::address::attach::blocks(&hub.model.attached.things);
    let said = blocks.first().map(ToString::to_string).unwrap_or_default();
    for part in [
        "ci-ubuntu · detail-front.png row",
        "Say when the retry will happen",
    ] {
        assert!(said.contains(part), "{part} is not in {said}");
    }
    Ok(())
}

#[test]
fn a_drag_on_a_marked_up_screen_makes_a_region_and_its_typed_note_is_attached() -> Check {
    let (mut rt, sent) = draw("design/mark-up", false)?;
    let [x, y, _, _] = rect(&rt, "design-0.screen")?;
    rt.pointer_move(x + 200.0, y + 150.0);
    rt.pointer_down(x + 200.0, y + 150.0);
    rt.pointer_move(x + 250.0, y + 180.0);
    rt.pointer_up(x + 250.0, y + 180.0);
    rest(&mut rt);
    assert_eq!(words(&rt, "design-0.screen.mark.1.number")?, ["2"]);
    tap(&mut rt, "design-0.screen.mark.1.note.field", rest)?;
    press(&mut rt, Key::Text("Too dark here".to_owned()), false);
    press(&mut rt, Key::Enter, false);
    rest(&mut rt);
    let hub = rt.state.hub.try_borrow()?;
    let things = &hub.model.attached.things;
    let second = things.get(1).ok_or("the region was not attached")?;
    assert_eq!(
        second.address.to_string(),
        "yi://screen/ci-ubuntu#400,300,100,60"
    );
    assert_eq!(second.excerpt.as_deref(), Some("Too dark here"));
    let marks = hub.panes.get::<Marks>(Some(yd_ui::pane::SCENE_PANE));
    assert_eq!(marks.map(|marks| marks.notes.len()), Some(2));
    assert!(sent.borrow().is_empty(), "nothing reaches the far side");
    let _ = markup::MAX_NOTES;
    Ok(())
}

#[test]
fn the_box_is_the_element_as_a_thing_and_its_right_click_row_opens_the_lines_that_draw_it() -> Check
{
    let (mut rt, sent) = draw("design/box", true)?;
    let thing = {
        let hub = rt.state.hub.try_borrow()?;
        let found = thing_at(&hub, yd_ui::pane::SCENE_PANE, "web-0.design.element");
        assert_eq!(thing_at(&hub, yd_ui::pane::SCENE_PANE, "web-0.page"), None);
        found.ok_or("the box is no thing")?
    };
    let address = thing.address.to_string();
    assert_eq!(address, "yi://element/localhost:5173/upload#div.alert");
    assert_eq!(thing.label, "div.alert");
    assert_eq!(
        thing.excerpt.as_deref(),
        Some("Slow down. Try again in 41 seconds.")
    );
    let aimed = Run::new(ELEMENT_SOURCE).on(Target::Thing(thing.address));
    rt.update(move |win, tx| {
        command_box::run(win, tx, aimed.clone());
    });
    rest(&mut rt);
    let read = SourceCmd::Read("src/pages/Upload.tsx".into());
    let all = sent.borrow();
    let asked = all
        .iter()
        .any(|e| matches!(e, Effect::Source(_, ask) if *ask == read));
    assert!(asked, "the files source is asked for the file: {all:?}");
    Ok(())
}

#[test]
fn an_attached_element_carries_its_html_with_a_notice_where_it_was_cut() -> Check {
    let (rt, _) = draw("design/box", false)?;
    let mut hub = rt.state.hub.try_borrow_mut()?;
    let table = hub.model.tables.get(FEED).ok_or("no page feed")?;
    let row = table
        .find(&RowId::new(ELEMENT))
        .ok_or("no element")?
        .clone();
    let html = "<div class=\"alert\">Slow down.</div>";
    let row = row.text("html", html).text("html_size", "5000");
    step(&mut hub.model, Msg::Feed(FEED, FeedEvent::Upsert(row)));
    let found = thing_at(&hub, yd_ui::pane::SCENE_PANE, "web-0.design.element");
    let excerpt = found.and_then(|thing| thing.excerpt).ok_or("no words")?;
    assert_eq!(
        excerpt,
        "Slow down. Try again in 41 seconds.\nHTML: <div class=\"alert\">Slow down.</div>\n\
         […] kept 35 of 5000 characters of its HTML (cap 2000)"
    );
    Ok(())
}

#[test]
fn design_mode_starts_stops_and_is_off_until_asked() -> Check {
    let (mut rt, _) = draw("design/empty", false)?;
    let on = |rt: &Runtime<Win>| {
        let hub = rt.state.hub.try_borrow().ok()?;
        Some(mode(&hub).map_or((false, false), |mode| (mode.on, mode.picking)))
    };
    assert_eq!(on(&rt), Some((false, false)));
    let empty = words(&rt, "design-0.state")?;
    let how = "Choose Design in the web app's bar, then press an element of the page.";
    assert_eq!(empty, ["Nothing picked yet", how]);
    for (command, want) in [
        ("design.start", (true, true)),
        ("design.stop", (false, false)),
    ] {
        let id = DEF
            .commands
            .iter()
            .find(|c| c.id.0 == command)
            .map(|c| c.id);
        let id = id.ok_or("the command is not declared")?;
        rt.update(move |win, tx| {
            command_box::run(win, tx, Run::new(id));
        });
        rest(&mut rt);
        assert_eq!(on(&rt), Some(want), "{command}");
    }
    let _ = (SET, Mode::default());
    Ok(())
}

/// The review's picture: the inset's "16" was cut in two by the handle drawn over it.
#[test]
fn a_guides_distance_is_drawn_over_the_box_and_its_handles() -> Check {
    let (rt, _) = draw("design/box", true)?;
    let all = rt.elements();
    let at = |key: &str| {
        let found = all.iter().position(|e| e.key.as_deref() == Some(key));
        found.ok_or_else(|| format!("no {key}"))
    };
    let tag = at("web-0.design.inset")?;
    for under in [
        "web-0.design.element",
        "web-0.design.handle.w",
        "web-0.design.handle.sw",
    ] {
        assert!(
            tag > at(under)?,
            "{under} is drawn over the inset's distance"
        );
    }
    Ok(())
}
