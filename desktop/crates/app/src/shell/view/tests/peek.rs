//! Space to peek, in the real window: a card over the window about the thing the pointer is
//! on, which makes no pane and no tab, and goes at Space, Esc or a press.

use yd_core::address::thing::Thing;
use yd_core::feed::{FILES, FeedEvent, OPEN_FILES, SourceCmd};
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_core::row::{Row, Rows};
use yd_ui::grid_view::view::show_pane;
use yd_ui::overlay::peek::{KEY, LINES, Open, Peeks};
use yd_ui::win::dispatch;

use super::*;

const MAIN_RS: &str = "files-1.row.src/main.rs";
const HEAD: &str = "window.peek.head";
const COUNT: &str = "window.peek.count";

fn space(runtime: &mut Runtime<Win>) {
    strike(runtime, Key::Text(" ".to_owned()), Mods::default());
}

/// The pointer comes to rest on `key`, pressing nothing.
fn point(runtime: &mut Runtime<Win>, key: &str) -> Check {
    let (x, y) = centre(runtime, key)?;
    runtime.pointer_move(x, y);
    rest(runtime);
    Ok(())
}

fn line(runtime: &Runtime<Win>, at: usize) -> Result<String, Box<dyn Error>> {
    row_text(&runtime.elements(), &format!("{KEY}.line.{at}"))
}

/// The panes of the grid and the files with a tab: what a peek must leave as it found.
fn made(runtime: &Runtime<Win>) -> Result<(usize, usize), Box<dyn Error>> {
    let hub = runtime.state.hub.try_borrow()?;
    let tabs = hub
        .model
        .tables
        .get(OPEN_FILES)
        .map_or(0, |open| open.len());
    Ok((hub.model.grid.panes.len(), tabs))
}

/// The text of `path` arrives from the files source: `lines` lines, each its own number.
fn read(runtime: &mut Runtime<Win>, path: &str, lines: usize) {
    let text: String = (1..=lines).map(|n| format!("line {n}\n")).collect();
    let row = Row::new(format!("file:{path}")).text("text", text);
    runtime.update(move |win, _| {
        dispatch(win, Msg::Feed(FILES, FeedEvent::Upsert(row.clone())));
    });
    rest(runtime);
}

/// Space with the pointer on a file shows a card named by the file, asks for its text and
/// shows its first lines with their numbers. No pane and no tab is made; Space closes it.
#[test]
fn space_peeks_the_file_under_the_pointer_and_makes_no_pane() -> Check {
    let mut runtime = settled("shell/everything")?;
    let was = made(&runtime)?;
    let asked = Rc::new(RefCell::new(Vec::new()));
    let log = Rc::clone(&asked);
    runtime
        .state
        .hub
        .try_borrow_mut()?
        .set_sink(move |effect| log.borrow_mut().push(effect));
    point(&mut runtime, MAIN_RS)?;
    assert!(!live(&runtime, KEY), "pointing alone shows nothing");
    space(&mut runtime);
    assert!(row_text(&runtime.elements(), HEAD)?.contains("main.rs"));
    let reads = |effect: &Effect| matches!(effect, Effect::Source(FILES, SourceCmd::Read(path)) if path.to_str() == Some("src/main.rs"));
    assert_eq!(asked.borrow().iter().filter(|e| reads(e)).count(), 1);
    read(&mut runtime, "src/main.rs", LINES + 6);
    assert_eq!(line(&runtime, 0)?, "1 line 1");
    assert_eq!(line(&runtime, LINES - 1)?, format!("{LINES} line {LINES}"));
    assert!(!live(&runtime, &format!("{KEY}.line.{LINES}")), "the cap");
    let all = LINES + 6;
    let count = row_text(&runtime.elements(), COUNT)?;
    assert_eq!(count, format!("lines 1–{LINES} of {all}"), "and it says so");
    assert_eq!(
        asked.borrow().iter().filter(|e| reads(e)).count(),
        1,
        "asked once"
    );
    assert_eq!(made(&runtime)?, was, "no pane and no tab");
    space(&mut runtime);
    assert!(!live(&runtime, KEY), "Space again closes it");
    assert_eq!(made(&runtime)?, was);
    Ok(())
}

/// Esc closes the card, and so does a press off it, which reaches nothing under it.
#[test]
fn escape_or_a_press_off_the_card_closes_it_and_presses_nothing() -> Check {
    let mut runtime = settled("shell/everything")?;
    let was = made(&runtime)?;
    point(&mut runtime, MAIN_RS)?;
    space(&mut runtime);
    assert!(live(&runtime, KEY));
    strike(&mut runtime, Key::Escape, Mods::default());
    assert!(!live(&runtime, KEY), "Esc");
    space(&mut runtime);
    assert!(
        live(&runtime, KEY),
        "the pointer has not moved: the same file"
    );
    tap(&mut runtime, "files-1.row.src/routes/sessions.rs")?;
    assert!(!live(&runtime, KEY), "a press");
    assert_eq!(made(&runtime)?, was, "which opened nothing");
    Ok(())
}

/// A file just pressed is peeked by Space like any other: the press is not made again.
#[test]
fn space_after_a_press_on_a_file_peeks_it_and_presses_nothing_twice() -> Check {
    let mut runtime = settled("shell/everything")?;
    tap(&mut runtime, MAIN_RS)?;
    let was = made(&runtime)?;
    space(&mut runtime);
    assert!(row_text(&runtime.elements(), HEAD)?.contains("main.rs"));
    assert_eq!(made(&runtime)?, was);
    Ok(())
}

/// In a text box Space is a space: the composer takes it and nothing is peeked, though
/// the pointer rests on a file.
#[test]
fn space_in_the_composer_types_a_space() -> Check {
    let mut runtime = settled("shell/everything")?;
    tap(&mut runtime, "chat-8.composer.input")?;
    point(&mut runtime, MAIN_RS)?;
    strike(&mut runtime, Key::Text("a".to_owned()), Mods::default());
    space(&mut runtime);
    strike(&mut runtime, Key::Text("b".to_owned()), Mods::default());
    assert!(!live(&runtime, KEY));
    let typed = row_text(&runtime.elements(), "chat-8.composer.input")?;
    assert!(typed.contains("a b"), "{typed:?}");
    Ok(())
}

/// The card hangs just under the pointer, and stands just over it where the window ends
/// first. Either way all of it is inside the window.
#[test]
fn the_card_is_under_the_pointer_or_over_it_and_always_inside_the_window() -> Check {
    let mut runtime = settled("shell/everything")?;
    let card = |runtime: &Runtime<Win>| -> Result<[f32; 4], Box<dyn Error>> {
        Ok(by_key(&runtime.elements(), KEY).ok_or("no card")?.rect)
    };
    let (_, y) = centre(&runtime, MAIN_RS)?;
    point(&mut runtime, MAIN_RS)?;
    space(&mut runtime);
    let [left, top, wide, tall] = card(&runtime)?;
    assert!(
        (top - (y + 14.0)).abs() < 0.5,
        "under the pointer: {top} {y}"
    );
    assert!(left >= 4.0 && left + wide <= 1196.0 && top + tall <= 750.0);
    space(&mut runtime);
    runtime.set_viewport((1200.0, 400.0), 1.0);
    rest(&mut runtime);
    let (_, y) = centre(&runtime, MAIN_RS)?;
    point(&mut runtime, MAIN_RS)?;
    space(&mut runtime);
    let [_, top, _, tall] = card(&runtime)?;
    assert!(top >= 4.0, "inside the window: {top}");
    assert!(
        (top + tall - (y - 14.0)).abs() < 0.5,
        "over the pointer: {top} {tall} {y}"
    );
    Ok(())
}

/// A line of code peeked starts the card at that line, numbered as the file numbers it.
#[test]
fn a_line_of_a_file_is_peeked_from_that_line_on() -> Check {
    let mut runtime = settled("shell/everything")?;
    point(&mut runtime, "code-7.row.L5")?;
    space(&mut runtime);
    let head = row_text(&runtime.elements(), HEAD)?;
    assert!(head.contains("line 5"), "{head:?}");
    let first = line(&runtime, 0)?;
    assert!(first.starts_with("5 "), "{first:?}");
    let count = row_text(&runtime.elements(), COUNT)?;
    assert!(count.starts_with("lines 5–"), "{count:?}");
    Ok(())
}

/// A thing that is no file says what it is in its own words, with no line numbers.
#[test]
fn a_thread_is_peeked_by_its_name() -> Check {
    let mut runtime = settled("shell/everything")?;
    point(&mut runtime, "shell.thread.tidy-ci-cache-keys.sidebar-row")?;
    space(&mut runtime);
    let head = row_text(&runtime.elements(), HEAD)?;
    assert!(head.starts_with("thread ·"), "{head:?}");
    let first = line(&runtime, 0)?;
    assert!(!first.is_empty() && !first.starts_with("1 "), "{first:?}");
    assert_eq!(row_text(&runtime.elements(), COUNT)?, "");
    Ok(())
}

/// With the pointer on a pane's bare ground Space peeks nothing, until a list has the keys
/// and a row under its arrow keys. The pointer on a thing still comes first.
#[test]
fn off_any_thing_space_peeks_the_row_the_arrow_keys_stand_on() -> Check {
    let mut runtime = settled("shell/everything")?;
    runtime.update(|win, tx| {
        show_pane(win, tx, PaneKind::Agents);
    });
    rest(&mut runtime);
    let shown = runtime.elements();
    let rows = keys(&shown)
        .into_iter()
        .find(|k| k.starts_with("agents-") && k.ends_with(".rows"));
    let rows = rows.ok_or("no agents list")?.to_owned();
    let ground = rows.replace(".rows", ".pane-meta");
    point(&mut runtime, &ground)?;
    space(&mut runtime);
    assert!(!live(&runtime, KEY), "a pane is no thing to peek");
    let list = rows.clone();
    runtime.update(move |_, tx| tx.focus(list.clone(), false));
    rest(&mut runtime);
    strike(&mut runtime, Key::Down, Mods::default());
    space(&mut runtime);
    let head = row_text(&runtime.elements(), HEAD)?;
    assert!(head.starts_with("agent ·"), "{head:?}");
    space(&mut runtime);
    assert!(!live(&runtime, KEY), "Space closes it from the list too");
    strike(&mut runtime, Key::Enter, Mods::default());
    let detail = |runtime: &Runtime<Win>| {
        let shown = runtime.elements();
        keys(&shown).iter().any(|key| key.contains(".detail."))
    };
    assert!(detail(&runtime), "Return opens the row in place");
    space(&mut runtime);
    strike(&mut runtime, Key::Escape, Mods::default());
    assert!(!live(&runtime, KEY), "Esc closes the card first");
    assert!(detail(&runtime), "and leaves the row open");
    point(&mut runtime, "code-7.row.L5")?;
    space(&mut runtime);
    let head = row_text(&runtime.elements(), HEAD)?;
    assert!(
        head.contains("line 5"),
        "the pointer's thing first: {head:?}"
    );
    Ok(())
}

/// Space held down repeats: the card it opened stays, and the one it closed stays shut.
#[test]
fn a_held_space_opens_once_and_closes_once() -> Check {
    let mut runtime = settled("shell/everything")?;
    let held = |runtime: &mut Runtime<Win>| {
        space(runtime);
        for _ in 0..3 {
            runtime.repeat_key(KeyEvent {
                key: Key::Text(" ".to_owned()),
                mods: Mods::default(),
            });
            rest(runtime);
        }
    };
    point(&mut runtime, MAIN_RS)?;
    held(&mut runtime);
    assert!(live(&runtime, KEY), "open, with nothing focused");
    held(&mut runtime);
    assert!(!live(&runtime, KEY), "and shut");
    tap(&mut runtime, MAIN_RS)?;
    held(&mut runtime);
    assert!(live(&runtime, KEY), "open, with the row focused");
    held(&mut runtime);
    assert!(!live(&runtime, KEY), "and shut");
    Ok(())
}

/// Space on a tab that has the keys presses it, as on any control that is no thing to
/// peek: nothing is shown though the tab stands for one.
#[test]
fn space_on_a_focused_tab_presses_it_and_peeks_nothing() -> Check {
    let mut runtime = settled("shell/everything")?;
    point(&mut runtime, "code-7.pane-meta")?;
    runtime.update(|_, tx| tx.focus("code-7.tab-2.pane-tab", true));
    rest(&mut runtime);
    space(&mut runtime);
    assert!(!live(&runtime, KEY));
    Ok(())
}

/// A file whose read failed says why in the card.
#[test]
fn a_file_that_cannot_be_read_says_why() -> Check {
    let mut runtime = settled("shell/everything")?;
    point(&mut runtime, MAIN_RS)?;
    space(&mut runtime);
    let row = Row::new("file:src/main.rs").text("error", "No such file");
    runtime.update(move |win, _| {
        dispatch(win, Msg::Feed(FILES, FeedEvent::Upsert(row.clone())));
    });
    rest(&mut runtime);
    assert!(line(&runtime, 0)?.ends_with("No such file"));
    Ok(())
}

/// A thing with many lines of its own words shows the first of them and says how many
/// there are; found with no pointer on it, its card hangs under its element.
#[test]
fn many_words_are_cut_and_counted_and_the_card_hangs_from_the_element() -> Check {
    let mut runtime = settled("shell/everything")?;
    let all = LINES + 6;
    let words: String = (1..=all).map(|n| format!("word {n}\n")).collect();
    let thing = Thing {
        label: "pane · Code".to_owned(),
        address: Address::new(Kind::Agent, "a1")?,
        excerpt: Some(words),
    };
    let open = Open {
        key: "code-7.breadcrumb".to_owned(),
        thing,
        at: None,
    };
    runtime.update(move |win, _| {
        let hub = win.hub.try_borrow_mut().ok();
        let peeks = hub.and_then(|mut hub| {
            let peeks = hub.panes.get_mut::<Peeks>(None)?;
            peeks.0.insert(root::MAIN, open.clone())
        });
        drop(peeks);
    });
    rest(&mut runtime);
    // The card lies over code, whose words fall inside the same rows.
    assert!(line(&runtime, 0)?.starts_with("word 1"), "no line numbers");
    assert!(line(&runtime, LINES - 1)?.starts_with(&format!("word {LINES}")));
    assert!(!live(&runtime, &format!("{KEY}.line.{LINES}")));
    let shown = runtime.elements();
    assert_eq!(row_text(&shown, COUNT)?, format!("{LINES} of {all} lines"));
    let [x, y, _, h] = by_key(&shown, "code-7.breadcrumb").ok_or("no crumb")?.rect;
    let [left, top, ..] = by_key(&shown, KEY).ok_or("no card")?.rect;
    assert!(
        (left - x).abs() < 0.5 && (top - (y + h + 4.0)).abs() < 0.5,
        "{left} {top}"
    );
    Ok(())
}

/// A pane's bare ground is the pane, which is on screen already: Space there shows nothing.
#[test]
fn space_on_a_panes_bare_ground_peeks_nothing() -> Check {
    let mut runtime = settled("shell/everything")?;
    let [x, y, w, h] = by_key(&runtime.elements(), "files-1")
        .ok_or("no Files")?
        .rect;
    runtime.pointer_move(x + w / 2.0, y + h - 60.0);
    rest(&mut runtime);
    let under = runtime.element_at_in("overlay.base", x + w / 2.0, y + h - 60.0);
    let path = under
        .map(|picked| picked.path().to_owned())
        .unwrap_or_default();
    assert!(
        path.contains("files-1") && !path.contains(".row."),
        "{path}"
    );
    space(&mut runtime);
    assert!(!live(&runtime, KEY));
    Ok(())
}
