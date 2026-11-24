//! The right-click menu as a person meets it: where it opens, what it lists, how it closes.

use std::error::Error;

use rune::{ElementInfo, Env, Key, KeyEvent, Mods, PointerEvent, Runtime, col, el};
use yd_core::address::attach::Attached;
use yd_core::command::by_kind::{COPY_ADDRESS, PUT_BACK};

use super::*;
use crate::menu::command_box::keys;

type Check = Result<(), Box<dyn Error>>;
type Window = Runtime<Win>;

const WINDOW: WindowId = WindowId(0);
const SIZE: (f32, f32) = (1200.0, 750.0);
const DT: f64 = 1.0 / 120.0;
/// A row of some pane, standing where a pane would draw it.
const ROW: &str = "changes-3.row.src/lib.rs";

fn root(win: &Win, cx: &mut Cx<Win>) -> El {
    let Ok(hub) = win.hub.try_borrow() else {
        return col();
    };
    let row = el().key(ROW).absolute(100.0, 100.0).size(400.0, 20.0);
    let base = el()
        .key("root.base")
        .w_full()
        .h_full()
        .child(row)
        .on_secondary_press_local(|win: &mut Win, p: PointerEvent, tx: &mut Tx| press(win, p, tx));
    rune_kit::root(base, command_box::layer(win, &hub, cx))
}

fn window() -> Window {
    let hub = Hub::new(&[], Vec::new());
    let mut window = Runtime::new(Win::new(hub, WINDOW), root, SIZE, Env::new(true, 1.0));
    window.on_key = Some(keys);
    rest(&mut window);
    window
}

fn rest(window: &mut Window) {
    for _ in 0..120 {
        if !window.frame(DT) {
            break;
        }
    }
}

fn found(tree: &[ElementInfo], key: &str) -> Option<ElementInfo> {
    let mut hits = tree
        .iter()
        .filter(|e| !e.exiting && e.key.as_deref() == Some(key));
    hits.next().cloned()
}

/// Every word drawn inside the element keyed `key`, in tree order.
fn words_in(tree: &[ElementInfo], key: &str) -> Vec<String> {
    let Some(outer) = found(tree, key) else {
        return Vec::new();
    };
    let [x, y, w, h] = outer.rect;
    let inside = |e: &&ElementInfo| {
        let [ex, ey, ..] = e.rect;
        ex >= x && ey >= y && ex <= x + w && ey <= y + h
    };
    tree.iter()
        .filter(inside)
        .filter_map(|e| e.text.clone())
        .collect()
}

fn line_18() -> Result<Thing, Box<dyn Error>> {
    let address = Address::new(Kind::Lines, LINE.0)?.part(LINE.1);
    Ok(Thing {
        label: "rate_limit.rs:18".to_owned(),
        address,
        excerpt: None,
    })
}

fn open_on(window: &mut Window, at: [f32; 2], thing: Thing) -> Check {
    window.update(move |win: &mut Win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            open_in(&mut hub, WINDOW, at, thing);
        }
    });
    rest(window);
    Ok(())
}

fn is_open(window: &Window) -> Result<bool, Box<dyn Error>> {
    let hub = window.state.hub.try_borrow()?;
    Ok(hub
        .panes
        .get::<Menus>(None)
        .is_some_and(|m| m.0.contains_key(&WINDOW)))
}

#[test]
fn a_secondary_press_opens_one_menu_at_the_pointer_for_the_pane_under_it() -> Check {
    let mut window = window();
    window.secondary_down(300.0, 110.0);
    window.secondary_up(300.0, 110.0);
    rest(&mut window);
    let tree = window.elements();
    let card = found(&tree, KEY).ok_or("no menu after a right-click")?;
    assert_eq!(
        (card.rect[0], card.rect[1]),
        (300.0, 110.0),
        "at the pointer"
    );
    let head = words_in(&tree, HEAD);
    assert_eq!(
        head,
        ["Changes", "yi://pane/3"],
        "a key with no thing yet: its pane"
    );
    assert!(found(&tree, &format!("{KEY}.row.{}", ATTACH.0)).is_some());
    assert!(!secondary(
        &mut window.state,
        [1.0, 1.0],
        "shell.top-bar",
        None
    ));
    Ok(())
}

#[test]
fn the_menu_reads_in_the_records_order_with_the_kinds_items_in_the_middle() -> Check {
    let mut window = window();
    open_on(&mut window, [100.0, 60.0], line_18()?)?;
    let tree = window.elements();
    let rows: Vec<String> = tree
        .iter()
        .filter_map(|e| {
            e.key
                .as_deref()?
                .strip_prefix(&format!("{KEY}.row."))
                .map(str::to_owned)
        })
        .collect();
    let want = [
        "thing.attach",
        "thing.ask",
        "thing.hand-off",
        "lines.references",
        "lines.which-turn",
        "lines.put-back",
        "lines.break",
        "thing.finding",
        "thing.watch",
        "thing.open-pane",
        "thing.copy-address",
    ];
    assert_eq!(rows, want);
    let head = words_in(&tree, HEAD);
    let address = "yi://lines/src/middleware/rate_limit.rs#L18";
    assert_eq!(head, ["rate_limit.rs line 18", address]);
    Ok(())
}

/// Frame 14's strings less the address (plan default 15 writes `yi://<kind>/<id>`), the
/// Ctrl-click hint intended.json drops, and "turn 2", which needs the scene's thread.
#[test]
fn every_string_frame_14_draws_in_the_menu_is_drawn() -> Check {
    let mut window = window();
    open_on(&mut window, [100.0, 60.0], line_18()?)?;
    let tree = window.elements();
    let drawn: Vec<String> = tree.iter().filter_map(|e| e.text.clone()).collect();
    let page = include_str!("../../../../../fixtures/mockup/panes/context-menu.text.json");
    // One string a line, as the extractor writes it; the keyed lines are not strings.
    let strings = page.lines().filter(|line| !line.contains("\": "));
    let strings = strings.filter_map(|line| line.trim().trim_end_matches(',').strip_prefix('"'));
    let strings: Vec<&str> = strings.filter_map(|line| line.strip_suffix('"')).collect();
    assert_eq!(strings.len(), 23, "the fixture's strings");
    let left_out = |s: &&&str| s.starts_with("yi://") || **s == "⌃ click" || **s == "turn 2";
    let missing: Vec<&str> = strings
        .iter()
        .filter(|s| !left_out(s))
        .copied()
        .filter(|s| !drawn.iter().any(|d| d == s))
        .collect();
    assert_eq!(missing, Vec::<&str>::new());
    let card = found(&tree, KEY).ok_or("no menu")?;
    assert_eq!(card.rect[2], 244.0, "the record's width");
    let head = found(&tree, HEAD).ok_or("no header")?;
    assert!(
        (head.rect[3] - 41.0).abs() <= 1.0,
        "header {}",
        head.rect[3]
    );
    let row = found(&tree, &format!("{KEY}.row.{}", ATTACH.0)).ok_or("no row")?;
    assert_eq!(row.rect[3], 22.0);
    Ok(())
}

#[test]
fn the_menu_opens_inside_the_window_near_an_edge() -> Check {
    let mut window = window();
    open_on(&mut window, [1190.0, 740.0], line_18()?)?;
    let card = found(&window.elements(), KEY).ok_or("no menu")?;
    assert_eq!(
        (card.rect[0], card.rect[1]),
        (SIZE.0 - 252.0, SIZE.1 - 380.0)
    );
    open_on(&mut window, [-20.0, 1.0], line_18()?)?;
    let card = found(&window.elements(), KEY).ok_or("no menu")?;
    assert_eq!((card.rect[0], card.rect[1]), (4.0, 4.0));
    Ok(())
}

#[test]
fn esc_a_press_outside_and_a_pick_each_close_it() -> Check {
    let mut window = window();
    let clipboard = rune::clipboard::memory();
    window.use_clipboard(&clipboard);
    open_on(&mut window, [100.0, 60.0], line_18()?)?;
    window.key(KeyEvent {
        key: Key::Escape,
        mods: Mods::default(),
    });
    rest(&mut window);
    assert!(!is_open(&window)?, "esc");
    open_on(&mut window, [100.0, 60.0], line_18()?)?;
    window.pointer_down(900.0, 700.0);
    window.pointer_up(900.0, 700.0);
    rest(&mut window);
    assert!(!is_open(&window)?, "a press outside");
    open_on(&mut window, [100.0, 60.0], line_18()?)?;
    let copy = found(&window.elements(), &format!("{KEY}.row.{}", COPY_ADDRESS.0));
    let [x, y, w, h] = copy.ok_or("no copy row")?.rect;
    window.pointer_down(x + w / 2.0, y + h / 2.0);
    window.pointer_up(x + w / 2.0, y + h / 2.0);
    rest(&mut window);
    assert!(!is_open(&window)?, "a pick");
    let copied = clipboard.text().map(|text| text.to_string());
    assert_eq!(
        copied.as_deref(),
        Some("yi://lines/src/middleware/rate_limit.rs#L18")
    );
    Ok(())
}

#[test]
fn attach_reads_remove_once_the_thing_is_attached() -> Check {
    let mut window = window();
    let thing = line_18()?;
    {
        let mut hub = window.state.hub.try_borrow_mut()?;
        hub.model.attached = Attached {
            things: vec![thing.clone()],
        };
    }
    open_on(&mut window, [100.0, 60.0], thing)?;
    let row = words_in(&window.elements(), &format!("{KEY}.row.{}", ATTACH.0));
    assert_eq!(row.first().map(String::as_str), Some("Remove from chat"));
    Ok(())
}

#[test]
fn an_item_nothing_carries_out_says_so_and_keeps_the_menu() -> Check {
    let mut window = window();
    open_on(&mut window, [100.0, 60.0], line_18()?)?;
    let item = found(&window.elements(), &format!("{KEY}.row.{}", PUT_BACK.0));
    let [x, y, w, h] = item.ok_or("no put-back row")?.rect;
    window.pointer_down(x + w / 2.0, y + h / 2.0);
    window.pointer_up(x + w / 2.0, y + h / 2.0);
    rest(&mut window);
    assert!(is_open(&window)?, "the menu stays to say why");
    assert_eq!(
        words_in(&window.elements(), &format!("{KEY}.said")),
        ["Put These Lines Back is not available yet"]
    );
    Ok(())
}
