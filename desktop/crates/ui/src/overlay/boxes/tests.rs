//! What a person attaching things would notice if it broke, on Rune's in-process runtime:
//! where the boxes are, what a chip and a box light, and what picking does.

use std::error::Error;

use rune::{Cx, El, ElementInfo, Env, Key, KeyEvent, Mods, Runtime, Tx, col};
use yd_core::address::attach::{ATTACH, AttachMsg, MAX_ATTACHED, PICK};
use yd_core::address::thing::Thing;
use yd_core::address::uri::Address;
use yd_core::command::table::Target;
use yd_core::feed::FeedEvent;
use yd_core::ids::{FeedId, PaneId, WindowId};
use yd_core::kind::Kind;
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::row::Row;

use super::scenes::{BOXED, HOVERED, PANES, SCENES, TRAY, found, row_key};
use super::tray::chip_key;
use super::{BASE, KEY, PREVIEW, box_key, line_prefix, over};
use crate::overlay::pick::{self, Context, Finder, LAYER, find, thing_at};
use crate::pane::Stage;
use crate::theme::look::colour;
use crate::win::{Hub, Win};

type Check = Result<(), Box<dyn Error>>;
type Window = Runtime<Win>;

const SIZE: (f32, f32) = (1200.0, 750.0);
/// One frame of a 120 Hz display, in seconds.
const DT: f64 = 1.0 / 120.0;

fn view(win: &Win, cx: &mut Cx<Win>) -> El {
    let Ok(hub) = win.hub.try_borrow() else {
        return col();
    };
    let body = match hub.stage {
        Stage::View(view) => view(win, &hub, cx),
        Stage::App | Stage::Pane(_) => col(),
    };
    col().key("root").w_full().h_full().child(body)
}

fn settle(rt: &mut Window) {
    for _ in 0..240 {
        if !rt.frame(DT) {
            break;
        }
    }
    rt.frame(DT);
}

/// A window on the scene named `name`, as the app would draw it.
fn window(name: &str) -> Result<Window, Box<dyn Error>> {
    let scene = SCENES.iter().find(|scene| scene.name == name);
    let scene = scene.ok_or("no such scene")?;
    let hub = Hub::new(&[], Vec::new());
    {
        let mut hub = hub.try_borrow_mut()?;
        hub.stage = scene.stage;
        (scene.setup)(&mut hub);
    }
    let win = Win::new(hub, WindowId(0));
    let mut rt = Runtime::new(win, view, SIZE, Env::new(true, 1.0));
    settle(&mut rt);
    Ok(rt)
}

fn keyed(rt: &Window, key: &str) -> Result<ElementInfo, Box<dyn Error>> {
    let all = rt.elements();
    let found = all
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key));
    Ok(found.ok_or_else(|| format!("no element keyed {key}"))?)
}

fn shows(rt: &Window, key: &str) -> bool {
    keyed(rt, key).is_ok()
}

/// The words inside the element keyed `key`, in tree order.
fn words(rt: &Window, key: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let within = format!("{}/", keyed(rt, key)?.path);
    let inside = rt.elements().into_iter().filter(|e| !e.exiting);
    let inside = inside.filter(|e| e.path.starts_with(&within));
    Ok(inside.filter_map(|e| e.text).collect())
}

fn address(index: usize, id: &str) -> Result<Address, Box<dyn Error>> {
    let kind = PANES.get(index).ok_or("no such stand-in")?.desc.kind;
    Ok(Address::new(kind, id)?)
}

fn centre(rt: &Window, key: &str) -> Result<(f32, f32), Box<dyn Error>> {
    let [x, y, w, h] = keyed(rt, key)?.rect;
    Ok((x + w / 2.0, y + h / 2.0))
}

fn point(rt: &mut Window, key: &str) -> Check {
    let (x, y) = centre(rt, key)?;
    rt.pointer_move(x, y);
    settle(rt);
    Ok(())
}

fn press(rt: &mut Window, key: &str) -> Check {
    let (x, y) = centre(rt, key)?;
    rt.pointer_move(x, y);
    settle(rt);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
    Ok(())
}

fn attached(rt: &Window) -> Result<Vec<String>, Box<dyn Error>> {
    let hub = rt.state.hub.try_borrow()?;
    let things = &hub.model.attached.things;
    Ok(things.iter().map(|thing| thing.label.clone()).collect())
}

fn close(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(a, b)| (a - b).abs() < 0.01)
}

/// The box a thing's element should wear: its rectangle grown by three points a side
/// (record 4.8).
fn grown(rect: [f32; 4]) -> [f32; 4] {
    let [x, y, w, h] = rect;
    [x - 3.0, y - 3.0, w + 6.0, h + 6.0]
}

#[test]
fn each_attached_thing_wears_a_numbered_box_three_points_out() -> Check {
    let rt = window("overlay/boxes")?;
    for (number, (index, id)) in BOXED.into_iter().enumerate() {
        let row = keyed(&rt, &row_key(index, id).ok_or("no row key")?)?.rect;
        let frame = box_key(&address(index, id)?);
        let drawn = keyed(&rt, &frame)?;
        // A group's first row holds the group's heading: the box is round the row under it.
        let bottom = row[1] + row[3];
        let row = [row[0], bottom - 20.0, row[2], 20.0];
        assert!(close(drawn.rect, grown(row)), "{id}: {:?}", drawn.rect);
        let tab = words(&rt, &format!("{frame}.number"))?;
        assert_eq!(tab, [(number + 1).to_string()], "{id} is numbered in order");
        assert!(
            !shows(&rt, &format!("{frame}.tag")),
            "a name tag is hidden at rest"
        );
        assert!(drawn.inert, "a box takes no input");
    }
    Ok(())
}

#[test]
fn a_box_follows_its_element_and_hides_while_it_is_off_screen() -> Check {
    let mut rt = window("overlay/boxes")?;
    let (index, id) = (1, "atlas-api");
    let (row, frame) = (
        row_key(index, id).ok_or("no key")?,
        box_key(&address(index, id)?),
    );
    let before = keyed(&rt, &frame)?.rect;
    rt.set_viewport((900.0, 140.0), 1.0);
    settle(&mut rt);
    let moved = keyed(&rt, &frame)?.rect;
    assert!(!close(before, moved), "the pane it is in got narrower");
    let [x, y, w, h] = keyed(&rt, &row)?.rect;
    let under = [x, y + h - 20.0, w, 20.0];
    assert!(close(moved, grown(under)), "{moved:?}");
    // The list is now shorter than its rows: scrolled, the row leaves through the top.
    let (x, y) = centre(&rt, &row)?;
    rt.wheel_xy(x, y, 0.0, -400.0);
    settle(&mut rt);
    assert!(
        !shows(&rt, &frame),
        "no box for a row scrolled out of sight"
    );
    let label = chip_key(TRAY, &address(index, id)?);
    assert!(shows(&rt, &label), "its chip stays");
    Ok(())
}

#[test]
fn a_chip_lights_its_box_and_a_box_lights_its_chip() -> Check {
    let mut rt = window("overlay/boxes")?;
    let things: Vec<Address> = BOXED
        .into_iter()
        .map(|(index, id)| address(index, id))
        .collect::<Result<_, _>>()?;
    let [agent, container, service] = things.as_slice() else {
        return Err("three things are boxed".into());
    };
    point(&mut rt, &chip_key(TRAY, container))?;
    let lit = box_key(container);
    assert_eq!(
        words(&rt, &format!("{lit}.tag"))?,
        ["container · atlas-api"]
    );
    assert!((keyed(&rt, &lit)?.opacity - 1.0).abs() < 0.01);
    for other in [agent, service] {
        let faded = keyed(&rt, &box_key(other))?.opacity;
        assert!((faded - 0.4).abs() < 0.01, "the others fade: {faded}");
    }
    let shown = words(&rt, &format!("{TRAY}.address"))?;
    assert_eq!(
        shown.first().map(String::as_str),
        Some("yi://container/atlas-api")
    );
    // The other way round: the pointer on a boxed row lights that row's chip.
    let strength = |rt: &Window, thing: &Address| -> Result<f32, Box<dyn Error>> {
        Ok(keyed(rt, &chip_key(TRAY, thing))?.fill.a)
    };
    point(&mut rt, &row_key(0, "fork-sliding-window").ok_or("no key")?)?;
    assert!(strength(&rt, agent)? > strength(&rt, container)? * 1.5);
    let shown = words(&rt, &format!("{TRAY}.address"))?;
    assert_eq!(
        shown.first().map(String::as_str),
        Some("yi://agent/fork-sliding-window")
    );
    // Off every box and chip nothing is lit: the last thing attached is the one shown.
    point(&mut rt, &row_key(1, "runner-a").ok_or("no key")?)?;
    let hub = rt.state.hub.try_borrow()?;
    assert_eq!(pick::context(&hub).and_then(Context::lit), None);
    Ok(())
}

#[test]
fn a_chip_is_removed_by_its_cross_and_clear_empties_the_tray() -> Check {
    let mut rt = window("overlay/boxes")?;
    let container = address(1, "atlas-api")?;
    let chip = chip_key(TRAY, &container);
    press(&mut rt, &format!("{chip}.remove"))?;
    assert!(!shows(&rt, &chip) && !shows(&rt, &box_key(&container)));
    let service = address(2, "yi-serve")?;
    let tab = words(&rt, &format!("{}.number", box_key(&service)))?;
    assert_eq!(tab, ["2"], "the rest are renumbered");
    press(&mut rt, &format!("{TRAY}.clear"))?;
    assert!(attached(&rt)?.is_empty());
    assert!(!shows(&rt, TRAY), "no tray while nothing is attached");
    Ok(())
}

#[test]
fn a_thing_whose_source_is_gone_keeps_its_chip_in_faint_words() -> Check {
    let mut rt = window("overlay/boxes")?;
    let gone = address(1, "registry")?;
    let tokens = rt.state.hub.try_borrow()?.model.tokens.clone();
    let ink = |rt: &Window| -> Result<_, Box<dyn Error>> {
        let chip = keyed(rt, &chip_key(TRAY, &gone))?;
        let within = format!("{}/", chip.path);
        let inside = rt.elements().into_iter();
        let mut label = inside.filter(|e| e.path.starts_with(&within));
        let label = label.find(|e| e.text.as_deref() == Some("container · registry"));
        Ok(label.and_then(|e| e.text_color).ok_or("no label")?)
    };
    assert_eq!(attached(&rt)?.len(), 4, "it is still attached");
    assert_eq!(ink(&rt)?, colour(tokens.faint), "its row left the feed");
    assert!(!shows(&rt, &box_key(&gone)), "and there is nothing to box");
    // While its row is there, the same chip is drawn as the last one attached: bright.
    {
        let mut hub = rt.state.hub.try_borrow_mut()?;
        let back = FeedEvent::Upsert(Row::new("registry").text("name", "registry"));
        step(&mut hub.model, Msg::Feed(FeedId::new("containers"), back));
    }
    rt.update(|_, _| {});
    settle(&mut rt);
    assert_eq!(ink(&rt)?, colour(tokens.fg));
    Ok(())
}

#[test]
fn picking_outlines_what_is_under_the_pointer_and_a_press_attaches_it() -> Check {
    let mut rt = window("overlay/boxes")?;
    rt.update(|win: &mut Win, tx: &mut Tx| {
        pick::run(win, tx, PICK, &Target::App);
    });
    settle(&mut rt);
    assert!(keyed(&rt, LAYER)?.focused, "the layer holds the keyboard");
    let row = row_key(HOVERED.0, HOVERED.1).ok_or("no key")?;
    let thing = address(HOVERED.0, HOVERED.1)?;
    point(&mut rt, &row)?;
    assert!(close(
        keyed(&rt, PREVIEW)?.rect,
        grown(keyed(&rt, &row)?.rect)
    ));
    let says = words(&rt, &format!("{PREVIEW}.tag"))?;
    assert_eq!(says, ["container · atlas-db", "click to attach"]);
    let before = attached(&rt)?.len();
    press(&mut rt, &row)?;
    assert_eq!(attached(&rt)?.len(), before + 1);
    let tab = words(&rt, &format!("{}.number", box_key(&thing)))?;
    assert_eq!(tab, [(before + 1).to_string()], "numbered next");
    assert!(!shows(&rt, &format!("containers-2.detail.{}", HOVERED.1)));
    let says = words(&rt, &format!("{PREVIEW}.tag"))?;
    assert_eq!(says.last().map(String::as_str), Some("click to remove"));
    press(&mut rt, &row)?;
    assert_eq!(
        attached(&rt)?.len(),
        before,
        "a press on a boxed thing removes it"
    );
    // A column title is no thing: nothing is outlined and a press attaches nothing.
    press(&mut rt, "containers-2.table-header")?;
    assert!(!shows(&rt, PREVIEW));
    assert_eq!(attached(&rt)?.len(), before);
    rt.key(KeyEvent {
        key: Key::Escape,
        mods: Mods::default(),
    });
    settle(&mut rt);
    assert!(!shows(&rt, LAYER), "Escape leaves");
    press(&mut rt, &row)?;
    assert_eq!(attached(&rt)?.len(), before, "a press no longer picks");
    Ok(())
}

#[test]
fn a_full_tray_says_so_and_takes_no_more() -> Check {
    let mut rt = window("overlay/picking")?;
    {
        let mut hub = rt.state.hub.try_borrow_mut()?;
        let have = hub.model.attached.things.len();
        for n in have..MAX_ATTACHED {
            let thing = Thing {
                label: format!("line {n}"),
                address: Address::new(Kind::Lines, "src/lib.rs")?.part(format!("L{n}")),
                excerpt: None,
            };
            step(&mut hub.model, Msg::Attach(AttachMsg::Toggle(thing)));
        }
    }
    let row = row_key(HOVERED.0, HOVERED.1).ok_or("no key")?;
    press(&mut rt, &row)?;
    assert_eq!(attached(&rt)?.len(), MAX_ATTACHED);
    let says = words(&rt, &format!("{PREVIEW}.tag"))?;
    let hint = says.last().map(String::as_str);
    assert_eq!(hint, Some("twelve attached: remove one first"));
    Ok(())
}

#[test]
fn attach_from_anywhere_is_a_command_aimed_at_an_address() -> Check {
    let mut rt = window("overlay/boxes")?;
    let before = attached(&rt)?.len();
    let service = address(2, "tailscaled")?;
    let target = Target::Thing(service.clone());
    let ran = std::rc::Rc::new(std::cell::Cell::new(false));
    let heard = ran.clone();
    let aimed = target.clone();
    rt.update(move |win: &mut Win, tx: &mut Tx| {
        heard.set(pick::run(win, tx, ATTACH, &aimed));
    });
    settle(&mut rt);
    assert!(ran.get(), "the overlay carries the command out");
    assert_eq!(attached(&rt)?.len(), before + 1);
    assert_eq!(
        attached(&rt)?.last().map(String::as_str),
        Some("service · tailscaled")
    );
    assert!(shows(&rt, &chip_key(TRAY, &service)));
    assert!(!shows(&rt, &box_key(&service)), "no element was pointed at");
    rt.update(|win: &mut Win, tx: &mut Tx| {
        let flip = yd_core::command::table::FLIP_CHAT;
        assert!(!pick::run(win, tx, flip, &Target::App), "not the overlay's");
    });
    assert_eq!(attached(&rt)?.len(), before + 1);
    Ok(())
}

#[test]
fn the_lookup_finds_the_nearest_thing_above_an_element() -> Check {
    let rt = window("overlay/boxes")?;
    let hub = rt.state.hub.try_borrow()?;
    let row = row_key(1, "atlas-db").ok_or("no key")?;
    let thing = thing_at(&hub, &row).ok_or("a row is a thing")?;
    assert_eq!(thing.label, "container · atlas-db");
    assert_eq!(thing.address.to_string(), "yi://container/atlas-db");
    assert_eq!(
        found(&hub, 1, "atlas-db").map(|f| f.thing),
        Some(thing.clone())
    );
    // An unkeyed word inside the row, as Rune names it: the row above it is the thing.
    let inner = format!("root/{KEY}/{BASE}/containers-2/containers-2.rows/{row}/#0/#1");
    let hit = find(&hub, &inner).ok_or("nothing found from inside the row")?;
    assert_eq!((hit.key.as_str(), &hit.thing), (row.as_str(), &thing));
    assert_eq!(find(&hub, &format!("root/{KEY}/{BASE}/containers-2")), None);
    assert_eq!(find(&hub, "root/#0/#4"), None);
    assert_eq!(thing_at(&hub, "containers-2.row.no-such-row"), None);
    assert_eq!(thing_at(&hub, "gadgets-2.row.atlas-db"), None);
    // A pane's tab is a thing whatever the pane: the chrome gives every one the same key.
    let tab = thing_at(&hub, "daemons-3.tab-0.pane-tab").ok_or("a tab is a thing")?;
    assert_eq!(
        (tab.label.as_str(), tab.address.to_string()),
        ("tab · Daemons", "yi://tab/3/0".to_owned())
    );
    Ok(())
}

#[test]
fn a_thread_row_in_the_sidebar_is_its_thread() -> Check {
    let rt = window("overlay/tray")?;
    let hub = rt.state.hub.try_borrow()?;
    for form in ["thread", "strip"] {
        let row = format!("shell.{form}.fork-sliding-window.sidebar-row");
        let thread = thing_at(&hub, &row).ok_or("a thread row is a thing")?;
        assert_eq!(thread.label, "thread · Fork · sliding window");
        assert_eq!(
            thread.address.to_string(),
            "yi://thread/fork-sliding-window"
        );
    }
    assert_eq!(
        thing_at(&hub, "shell.thread.no-such-thread.sidebar-row"),
        None
    );
    assert_eq!(
        thing_at(&hub, "shell.sidebar"),
        None,
        "the sidebar is no thing"
    );
    Ok(())
}

fn file_line(_: &Hub, _: PaneId, key: &str) -> Option<Thing> {
    let line = key.rsplit_once(".L")?.1.parse::<u32>().ok()?;
    let address = Address::new(Kind::Lines, "src/a/b.rs").ok()?;
    Some(Thing {
        label: String::new(),
        address: address.part(format!("L{line}-L{}", line.saturating_add(2))),
        excerpt: None,
    })
}

/// A file's row, as the files pane keys one: by a path that holds `/`.
fn file_row(_: &Hub, _: PaneId, key: &str) -> Option<Thing> {
    let path = key.strip_prefix("files-5.row.")?;
    let address = Address::new(Kind::File, path).ok()?;
    (path == "src/a/b.rs").then(|| Thing {
        label: String::new(),
        address,
        excerpt: None,
    })
}

/// Five lines of code in a pane, keyed as the code surface keys them.
fn slashed(_: &Win, hub: &Hub, cx: &mut Cx<Win>) -> El {
    let line = |n: u32| col().key(format!("code-4.view.L{n}")).w(300.0).h(16.0);
    let base = col()
        .w_full()
        .h_full()
        .child(col().key("code-4").w(300.0).children((1..=5).map(line)));
    over(base, hub, cx)
}

/// The same five lines in a pane two and a half lines tall: the rest are laid out below it.
fn cramped(_: &Win, hub: &Hub, cx: &mut Cx<Win>) -> El {
    let line = |n: u32| col().key(format!("code-4.view.L{n}")).w(300.0).h(16.0);
    let pane = col().key("code-4").w(300.0).h(40.0);
    let base = col()
        .w_full()
        .h_full()
        .child(pane.children((1..=5).map(line)));
    over(base, hub, cx)
}

#[test]
fn a_box_never_shows_outside_its_pane_not_even_for_a_frame() -> Check {
    let hub = Hub::new(&[], Vec::new());
    {
        let mut hub = hub.try_borrow_mut()?;
        hub.stage = Stage::View(cramped);
        let context = hub.panes.get_mut::<Context>(None).ok_or("no context")?;
        context.find_with(PaneKind::Code, Finder::Keyed(file_line));
        let below = format!("root/{KEY}/{BASE}/#0/code-4/code-4.view.L4");
        let found = find(&hub, &below).ok_or("a line is a thing")?;
        pick::attach(&mut hub, found);
    }
    let mut rt = Runtime::new(Win::new(hub, WindowId(0)), view, SIZE, Env::new(true, 1.0));
    let address: Address = "yi://lines/src/a/b.rs#L4-L6".parse()?;
    // A headless picture is taken after a frame or two, before anything settles.
    for frame in 1..=6 {
        rt.frame(DT);
        assert!(!shows(&rt, &box_key(&address)), "frame {frame}");
    }
    assert!(
        keyed(&rt, "code-4.view.L4")?.rect[1] >= 40.0,
        "the line is laid out below"
    );
    Ok(())
}

#[test]
fn a_range_of_lines_wears_one_box_round_every_line_of_it() -> Check {
    assert_eq!(line_prefix("code-4.view.L13"), Some("code-4.view"));
    assert_eq!(line_prefix("code-4.view.L13.gutter"), Some("code-4.view"));
    assert_eq!(line_prefix("code-4.view"), None);
    let hub = Hub::new(&[], Vec::new());
    {
        let mut hub = hub.try_borrow_mut()?;
        hub.stage = Stage::View(slashed);
        let context = hub.panes.get_mut::<Context>(None).ok_or("no context")?;
        context.find_with(PaneKind::Code, Finder::Keyed(file_line));
        context.find_with(PaneKind::Files, Finder::Keyed(file_row));
        // A key that holds `/` is found whole, from a word inside its row.
        let row = format!("root/{KEY}/{BASE}/files-5/files-5.rows/files-5.row.src/a/b.rs/#0");
        let file = find(&hub, &row).ok_or("a file row is a thing")?;
        assert_eq!(file.key, "files-5.row.src/a/b.rs");
        assert_eq!(file.thing.label, "file · b.rs");
        let path = format!("root/{KEY}/{BASE}/#0/code-4/code-4.view.L2");
        let found = find(&hub, &path).ok_or("a line is a thing")?;
        assert_eq!(found.thing.label, "b.rs lines 2–4", "named by the lookup");
        pick::attach(&mut hub, found);
    }
    let mut rt = Runtime::new(Win::new(hub, WindowId(0)), view, SIZE, Env::new(true, 1.0));
    settle(&mut rt);
    let address: Address = "yi://lines/src/a/b.rs#L2-L4".parse()?;
    let frame = keyed(&rt, &box_key(&address))?.rect;
    let first = keyed(&rt, "code-4.view.L2")?.rect;
    let lines = [first[0], first[1], 300.0, 48.0];
    assert!(
        close(frame, grown(lines)),
        "one box round lines 2 to 4: {frame:?}"
    );
    Ok(())
}

/// One string of a text fixture: a line that is `"words",`.
fn quoted(line: &str) -> Option<&str> {
    let line = line.trim().trim_end_matches(',');
    line.strip_prefix('"')?.strip_suffix('"')
}

/// The strings of a mockup text fixture: one a line between `"strings": [` and `]`.
fn strings(fixture: &str) -> Vec<&str> {
    let body = fixture
        .split_once("\"strings\": [")
        .map_or("", |(_, rest)| rest);
    let body = body.split_once(']').map_or("", |(body, _)| body);
    body.lines().filter_map(quoted).collect()
}

#[test]
fn the_tray_shows_what_the_mockup_shows() -> Check {
    let rt = window("overlay/tray")?;
    let fixture = include_str!("../../../../../fixtures/mockup/panes/context-tray.text.json");
    let wanted = strings(fixture);
    assert_eq!(wanted.len(), 14, "the fixture's strings were read");
    let drawn = words(&rt, TRAY)?;
    // Plan default 15: an address is built from record ids, so the mockup's own is not drawn.
    let theirs = "yi://atlas-api/rate-limit-uploads/thread/fork-sliding-window";
    let missing: Vec<&str> = wanted
        .iter()
        .copied()
        .filter(|want| !drawn.iter().any(|said| said == want))
        .collect();
    assert_eq!(missing, [theirs]);
    assert!(
        drawn
            .iter()
            .any(|said| said == "yi://thread/fork-sliding-window")
    );
    let [_, _, w, h] = keyed(&rt, TRAY)?.rect;
    assert!((w - 264.0).abs() < 0.01, "264 wide in a 280 pane: {w}");
    assert!(
        (h - 130.0).abs() <= 1.0,
        "the mockup's tray is 130 tall: {h}"
    );
    Ok(())
}
