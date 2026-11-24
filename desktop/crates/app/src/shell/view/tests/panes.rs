//! The top bar's one Panes button and its panel: layouts as pictures, layouts of your own
//! kept, renamed and deleted in place, the chat side, and every pane switched on and off.

use yd_core::grid::layout::{ChatSide, Layout, Size};
use yd_core::grid::saved::{EVERYTHING, OwnId};
use yd_core::ids::PaneId;
use yd_core::pane::PaneKind;
use yd_ui::grid_view::picker::BUTTON;
use yd_ui::menu::command_box::{self, Found};

use super::*;

const PANEL: &str = "grid.panel";

fn held() -> Mods {
    Mods { alt: true, ..cmd() }
}

/// ⌥⌘0, the key the button names.
fn open(runtime: &mut Runtime<Win>) {
    strike(runtime, Key::Text("0".into()), held());
}

fn kinds(runtime: &Runtime<Win>) -> Result<Vec<PaneKind>, Box<dyn Error>> {
    seated(runtime, |layout| {
        layout.docked().map(|(pane, _)| pane).collect()
    })
}

/// The kinds the window's layout has a place for, drawn or resting.
fn places(runtime: &Runtime<Win>) -> Result<Vec<PaneKind>, Box<dyn Error>> {
    seated(runtime, |layout| {
        layout.areas.iter().map(|(pane, _)| *pane).collect()
    })
}

fn seated(
    runtime: &Runtime<Win>,
    panes: impl Fn(&Layout) -> Vec<PaneId>,
) -> Result<Vec<PaneKind>, Box<dyn Error>> {
    let hub = runtime.state.hub.try_borrow()?;
    let grid = &hub.model.grid;
    let window = grid.window(root::MAIN).ok_or("no window")?;
    Ok(panes(&window.layout)
        .into_iter()
        .filter_map(|pane| grid.kind(pane))
        .collect())
}

fn own(runtime: &Runtime<Win>) -> Result<Vec<(u8, String)>, Box<dyn Error>> {
    let hub = runtime.state.hub.try_borrow()?;
    let kept = hub.model.grid.own.iter();
    Ok(kept.map(|own| (own.id.0, own.name.clone())).collect())
}

fn shown(runtime: &Runtime<Win>) -> Result<Option<OwnId>, Box<dyn Error>> {
    let hub = runtime.state.hub.try_borrow()?;
    Ok(hub.model.grid.window(root::MAIN).and_then(|w| w.own))
}

fn help(runtime: &Runtime<Win>, key: &str) -> Option<String> {
    by_key(&runtime.elements(), key).and_then(|e| e.help.clone())
}

fn typing(runtime: &mut Runtime<Win>, words: &str) {
    strike(runtime, Key::Text(words.into()), Mods::default());
}

/// Saves the window's arrangement from the panel under `name`.
fn save(runtime: &mut Runtime<Win>, name: &str) -> Check {
    if !live(runtime, PANEL) {
        open(runtime);
    }
    tap(runtime, "grid.panel.save")?;
    typing(runtime, name);
    strike(runtime, Key::Enter, Mods::default());
    Ok(())
}

/// Dies with the `+` and the layout button still in the top bar.
#[test]
fn the_top_bar_has_one_panes_button_that_names_its_key_and_opens_the_panel() -> Check {
    let mut runtime = settled("shell/mockup")?;
    let shown = runtime.elements();
    assert!(
        by_key(&shown, "shell.new-thread").is_none(),
        "the + is gone"
    );
    assert!(
        by_key(&shown, "shell.layout").is_none(),
        "the layout button is gone"
    );
    let button = by_key(&shown, BUTTON).ok_or("no Panes button")?;
    assert_eq!(button.label.as_deref(), Some("Panes"));
    assert_eq!(help(&runtime, BUTTON).as_deref(), Some("Panes ⌥⌘0"));
    tap(&mut runtime, BUTTON)?;
    let shown = runtime.elements();
    let [bx, by, bw, bh] = by_key(&shown, BUTTON).ok_or("no button")?.rect;
    let [px, py, pw, _] = by_key(&shown, PANEL)
        .ok_or("the button opened no panel")?
        .rect;
    assert!(py >= by + bh, "under the button: {py} vs {}", by + bh);
    assert!(
        (px + pw - (bx + bw)).abs() <= 8.5,
        "its right edge at the button's"
    );
    Ok(())
}

/// The panel closes on Esc, on a press outside it, and when a layout is chosen.
#[test]
fn the_panel_closes_on_escape_a_press_outside_and_a_layout_chosen() -> Check {
    let mut runtime = settled("shell/mockup")?;
    open(&mut runtime);
    assert!(live(&runtime, PANEL), "⌥⌘0 opens it");
    strike(&mut runtime, Key::Escape, Mods::default());
    assert!(!live(&runtime, PANEL), "Esc closes it");
    open(&mut runtime);
    let (x, y) = centre(&runtime, "shell.status-bar")?;
    runtime.pointer_move(x, y);
    runtime.pointer_down(x, y);
    runtime.pointer_up(x, y);
    rest(&mut runtime);
    assert!(!live(&runtime, PANEL), "a press outside closes it");
    open(&mut runtime);
    tap(&mut runtime, "grid.panel.layout.review")?;
    assert!(!live(&runtime, PANEL), "choosing a layout closes it");
    let want = [
        PaneKind::Files,
        PaneKind::Changes,
        PaneKind::Terminal,
        PaneKind::Chat,
    ];
    assert_eq!(
        places(&runtime)?,
        want,
        "and gives each of its kinds a place"
    );
    assert_eq!(
        kinds(&runtime)?,
        [PaneKind::Chat],
        "a session with nothing in it draws the chat alone"
    );
    let changes = yd_panes::registry::scenes().find(|scene| scene.name == "changes/mockup");
    (changes.ok_or("no changes scene")?.setup)(&mut *runtime.state.hub.try_borrow_mut()?);
    rest(&mut runtime);
    assert_eq!(
        kinds(&runtime)?,
        [PaneKind::Changes, PaneKind::Chat],
        "Changes takes its place once there are changes"
    );
    let hub = runtime.state.hub.try_borrow()?;
    let size = Size {
        w: 1000.0,
        h: 700.0,
    };
    let placed = hub.model.grid.place(root::MAIN, size);
    let across: f32 = placed.iter().map(|(_, rect)| rect.w).sum();
    assert!((across - size.w).abs() < 0.5, "with no hole: {placed:?}");
    Ok(())
}

/// Each picture is where `place` puts each pane in this window's grid, chat flipped with the
/// side; the layout shown is marked, and hovering names it with its key.
#[test]
fn each_picture_is_the_layout_as_this_grid_draws_it() -> Check {
    let mut runtime = settled("shell/everything")?;
    open(&mut runtime);
    let key = "grid.panel.layout.everything";
    for want_side in [ChatSide::Left, ChatSide::Right] {
        if want_side == ChatSide::Right {
            tap(&mut runtime, "grid.panel.side.1")?;
        }
        let side = runtime.state.hub.try_borrow()?.model.grid.side;
        assert_eq!(side, want_side, "the switch moved chat");
        let shown = runtime.elements();
        let [_, _, w, h] = by_key(&shown, "grid").ok_or("no grid")?.rect;
        let room = Size { w, h };
        let picture = by_key(&shown, key).ok_or("no picture of Everything")?;
        assert_eq!(picture.label.as_deref(), Some("Everything ⌥⌘1, shown"));
        assert_eq!(picture.help.as_deref(), Some("Everything ⌥⌘1"));
        let review = by_key(&shown, "grid.panel.layout.review").ok_or("no Review")?;
        assert_eq!(review.label.as_deref(), Some("Review ⌥⌘2"), "not shown");
        let (x0, y0) = (picture.rect[0] + 2.0, picture.rect[1] + 2.0);
        let scale = 40.0 / room.w;
        let placed = EVERYTHING.shape().picture(room, side, 40.0);
        assert_eq!(placed.len(), 8);
        for (at, (kind, rect)) in placed.iter().enumerate() {
            let cell = format!("{key}.{}-{at}", kind.slug());
            let [x, y, cw, ch] = by_key(&shown, &cell).ok_or(format!("no {cell}"))?.rect;
            let drawn = [x - x0 - 0.5, y - y0 - 0.5, cw + 1.0, ch + 1.0];
            let want = [rect.x, rect.y, rect.w, rect.h];
            for (a, b) in drawn.iter().zip(want) {
                assert!(
                    (a - b).abs() / scale < 0.5,
                    "{cell}: {drawn:?} is not {want:?}"
                );
            }
        }
        let chat = placed.iter().position(|(kind, _)| *kind == PaneKind::Chat);
        let chat = by_key(&shown, &format!("{key}.chat-{}", chat.ok_or("no chat")?));
        let files = by_key(&shown, &format!("{key}.files-0")).ok_or("no files")?;
        assert_ne!(
            chat.ok_or("no chat cell")?.fill,
            files.fill,
            "chat is tinted"
        );
    }
    Ok(())
}

/// On places a pane as its Show command does; off closes it as its bar does, with no hole;
/// chat stays on.
#[test]
fn a_pane_switched_on_is_placed_as_show_places_it_and_off_closes_with_no_hole() -> Check {
    let mut panel = settled("shell/mockup")?;
    let mut shown = settled("shell/mockup")?;
    shown.update(|win, tx| view::run(win, tx, CommandId("pane.show-trace"), Target::App));
    rest(&mut shown);
    open(&mut panel);
    assert_eq!(
        by_key(&panel.elements(), "grid.panel.pane.trace")
            .and_then(|e| e.label.clone())
            .as_deref(),
        Some("Trace, off")
    );
    tap(&mut panel, "grid.panel.pane.trace")?;
    assert!(live(&panel, PANEL), "the panel stays open");
    let grid = |runtime: &Runtime<Win>| -> Result<_, Box<dyn Error>> {
        let hub = runtime.state.hub.try_borrow()?;
        Ok(hub.model.grid.window(root::MAIN).map(|w| w.layout.clone()))
    };
    assert_eq!(grid(&panel)?, grid(&shown)?, "where Show Trace puts it");
    let label = by_key(&panel.elements(), "grid.panel.pane.trace").and_then(|e| e.label.clone());
    assert_eq!(label.as_deref(), Some("Trace, on"));
    let before = kinds(&panel)?;
    tap(&mut panel, "grid.panel.pane.files")?;
    let after = kinds(&panel)?;
    // The session has child agents, so Agents waited behind Files as a tab of the column.
    let swap = |kind: PaneKind| match kind {
        PaneKind::Files => PaneKind::Agents,
        other => other,
    };
    let less: Vec<_> = before.into_iter().map(swap).collect();
    assert_eq!(after, less, "Files closed, and its column's next tab shows");
    let hub = panel.state.hub.try_borrow()?;
    let placed = hub.model.grid.place(
        root::MAIN,
        Size {
            w: 1000.0,
            h: 700.0,
        },
    );
    let area: f32 = placed.iter().map(|(_, r)| r.w * r.h).sum();
    assert!((area - 700_000.0).abs() < 1.0, "no hole: {area}");
    drop(hub);
    tap(&mut panel, "grid.panel.pane.chat")?;
    assert!(
        kinds(&panel)?.contains(&PaneKind::Chat),
        "chat cannot be turned off"
    );
    let chat = by_key(&panel.elements(), "grid.panel.pane.chat").and_then(|e| e.label.clone());
    assert_eq!(chat.as_deref(), Some("Chat, always on"));
    Ok(())
}

/// The keyboard: the panel takes it when opened, the arrows move from item to item, Space
/// and Return press the one they are on.
#[test]
fn arrows_move_through_the_panel_and_space_or_return_presses() -> Check {
    let mut runtime = settled("shell/mockup")?;
    open(&mut runtime);
    let focused = |runtime: &Runtime<Win>| {
        let shown = runtime.elements();
        let found = shown.iter().find(|e| e.focused && !e.exiting);
        found.and_then(|e| e.key.clone()).unwrap_or_default()
    };
    let first = focused(&runtime);
    assert!(
        first.starts_with("grid.panel."),
        "the panel has the keyboard: {first:?}"
    );
    strike(&mut runtime, Key::Right, Mods::default());
    let next = focused(&runtime);
    assert!(
        next.starts_with("grid.panel.") && next != first,
        "{first} then {next}"
    );
    strike(&mut runtime, Key::Up, Mods::default());
    assert_eq!(focused(&runtime), first, "Up goes back");
    let mut seen = vec![first.clone()];
    for _ in 0..40 {
        strike(&mut runtime, Key::Down, Mods::default());
        let now = focused(&runtime);
        if now == "grid.panel.pane.trace" {
            break;
        }
        seen.push(now);
    }
    assert_eq!(focused(&runtime), "grid.panel.pane.trace", "after {seen:?}");
    for key in [
        "grid.panel.save",
        "grid.panel.side.1",
        "grid.panel.pane.files",
    ] {
        assert!(
            seen.iter().any(|k| k == key),
            "{key} is passed on the way: {seen:?}"
        );
    }
    strike(&mut runtime, Key::Text(" ".into()), Mods::default());
    assert!(
        kinds(&runtime)?.contains(&PaneKind::Trace),
        "Space turns Trace on"
    );
    strike(&mut runtime, Key::Enter, Mods::default());
    assert!(
        !kinds(&runtime)?.contains(&PaneKind::Trace),
        "Return turns it off"
    );
    Ok(())
}

/// A layout saved in the panel sits beside the five with the next ⌥⌘ key, is in ⌘K and the
/// View menu as "Layout: <name>", and its key shows it again.
#[test]
fn a_layout_saved_in_the_panel_takes_the_next_key_and_is_listed_everywhere() -> Check {
    let mut runtime = settled("shell/mockup")?;
    tap(&mut runtime, "shell.panes")?;
    tap(&mut runtime, "grid.panel.pane.files")?;
    save(&mut runtime, "Writing")?;
    assert_eq!(own(&runtime)?, [(6, "Writing".to_owned())]);
    assert_eq!(shown(&runtime)?, Some(OwnId(6)), "the window shows it now");
    let kept = places(&runtime)?;
    assert_eq!(
        help(&runtime, "grid.panel.layout.own-6").as_deref(),
        Some("Writing ⌥⌘6")
    );
    let hub = runtime.state.hub.try_borrow()?;
    let found = command_box::search(&hub, "writing");
    let titles: Vec<String> = found
        .iter()
        .filter_map(|f| match f {
            Found::Command(_, title, key) => {
                Some(format!("{title} {}", key.clone().unwrap_or_default()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(titles, ["Layout: Writing alt+cmd+6"], "⌘K lists it");
    let view_menu = command_box::menu_bar(&hub)
        .into_iter()
        .find(|m| m.title == "View");
    let in_menu = view_menu
        .ok_or("no View menu")?
        .items
        .into_iter()
        .any(|i| i.command.0 == "layout.own-6");
    assert!(in_menu, "the View menu holds it");
    let put_up = crate::menubar::items(&hub);
    drop(hub);
    assert!(!put_up.is_empty());
    strike(&mut runtime, Key::Text("1".into()), held());
    assert_ne!(places(&runtime)?, kept, "Everything");
    strike(&mut runtime, Key::Text("6".into()), held());
    assert_eq!(places(&runtime)?, kept, "⌥⌘6 gives its places again");
    assert_eq!(shown(&runtime)?, Some(OwnId(6)));
    Ok(())
}

/// Past ⌥⌘9 a layout gets no key, and the panel says so while it is named.
#[test]
fn past_alt_cmd_9_a_saved_layout_gets_no_key_and_the_panel_says_so() -> Check {
    let mut runtime = settled("shell/mockup")?;
    for name in ["a", "b", "c", "d"] {
        save(&mut runtime, name)?;
    }
    assert!(live(&runtime, PANEL), "saving leaves the panel open");
    tap(&mut runtime, "grid.panel.save")?;
    let field = by_key(&runtime.elements(), "grid.panel.name").and_then(|e| e.label.clone());
    assert_eq!(
        field.as_deref(),
        Some("Name it: ⌥⌘6 to ⌥⌘9 are taken, so it gets no key")
    );
    typing(&mut runtime, "e");
    strike(&mut runtime, Key::Enter, Mods::default());
    assert_eq!(own(&runtime)?.last(), Some(&(10, "e".to_owned())));
    assert_eq!(
        help(&runtime, "grid.panel.layout.own-10").as_deref(),
        Some("e no key")
    );
    Ok(())
}

/// A right-click on a saved layout's picture offers Rename and Delete; the five offer
/// neither. Deleting the layout shown keeps the arrangement and unmarks it.
#[test]
fn a_saved_layout_is_renamed_and_deleted_in_the_panel() -> Check {
    let mut runtime = settled("shell/mockup")?;
    save(&mut runtime, "Mine")?;
    let right = |runtime: &mut Runtime<Win>, key: &str| -> Check {
        let (x, y) = centre(runtime, key)?;
        runtime.secondary_down(x, y);
        runtime.secondary_up(x, y);
        rest(runtime);
        Ok(())
    };
    right(&mut runtime, "grid.panel.layout.everything")?;
    assert!(
        !live(&runtime, "grid.panel.rename"),
        "the five cannot be renamed"
    );
    assert!(!live(&runtime, "grid.panel.delete"));
    right(&mut runtime, "grid.panel.layout.own-6")?;
    tap(&mut runtime, "grid.panel.rename")?;
    for _ in 0.."Mine".len() {
        strike(&mut runtime, Key::Backspace, Mods::default());
    }
    typing(&mut runtime, "Ours");
    strike(&mut runtime, Key::Enter, Mods::default());
    assert_eq!(
        own(&runtime)?,
        [(6, "Ours".to_owned())],
        "renamed, same key"
    );
    let before = kinds(&runtime)?;
    right(&mut runtime, "grid.panel.layout.own-6")?;
    tap(&mut runtime, "grid.panel.delete")?;
    assert!(own(&runtime)?.is_empty());
    assert!(!live(&runtime, "grid.panel.layout.own-6"));
    assert_eq!(kinds(&runtime)?, before, "the arrangement stays");
    assert_eq!(shown(&runtime)?, None, "only unmarked");
    Ok(())
}

/// Esc while a name is asked cancels the name and leaves the panel; a second Esc closes it.
#[test]
fn escape_while_naming_a_layout_cancels_the_name_and_a_second_closes_the_panel() -> Check {
    let mut runtime = settled("shell/mockup")?;
    open(&mut runtime);
    tap(&mut runtime, "grid.panel.save")?;
    typing(&mut runtime, "half");
    assert!(live(&runtime, "grid.panel.name"), "the name is asked");
    strike(&mut runtime, Key::Escape, Mods::default());
    assert!(
        !live(&runtime, "grid.panel.name"),
        "the first Esc cancels the name"
    );
    assert!(live(&runtime, PANEL), "and leaves the panel open");
    assert!(own(&runtime)?.is_empty(), "nothing was kept");
    strike(&mut runtime, Key::Escape, Mods::default());
    assert!(!live(&runtime, PANEL), "the second Esc closes it");
    Ok(())
}

/// The name asked, the keyboard moved to another control of the panel: Esc still cancels the
/// name first and leaves the panel, as it does with the field focused.
#[test]
fn escape_cancels_the_name_even_when_the_field_lost_the_keyboard() -> Check {
    let mut runtime = settled("shell/mockup")?;
    open(&mut runtime);
    tap(&mut runtime, "grid.panel.save")?;
    tap(&mut runtime, "grid.panel.side.1")?;
    assert!(live(&runtime, "grid.panel.name"), "the name is still asked");
    strike(&mut runtime, Key::Escape, Mods::default());
    assert!(
        !live(&runtime, "grid.panel.name"),
        "the first Esc cancels the name"
    );
    assert!(live(&runtime, PANEL), "and leaves the panel open");
    Ok(())
}

/// Two panes of one place open as tabs of the slot that place already has, with no slot
/// cut for either, and a press on another pane's tab brings it in front.
#[test]
fn panes_of_one_place_open_as_tabs_and_a_press_on_a_tab_brings_it_in_front() -> Check {
    let mut runtime = settled("shell/mockup")?;
    let before = kinds(&runtime)?;
    for command in ["pane.show-trace", "pane.show-logs"] {
        runtime.update(|win, tx| view::run(win, tx, CommandId(command), Target::App));
        rest(&mut runtime);
    }
    let shown = kinds(&runtime)?;
    assert_eq!(shown.len(), before.len(), "no slot was cut: {shown:?}");
    assert!(
        shown.contains(&PaneKind::Logs),
        "the last asked for is in front"
    );
    assert!(
        !shown.contains(&PaneKind::Terminal),
        "the terminal is behind it"
    );
    let tabs = |runtime: &Runtime<Win>| -> Vec<String> {
        let all = runtime.elements();
        let keys = all.iter().filter_map(|e| e.key.clone());
        let stacked = keys.filter(|key| key.contains(".tab-pane-") && key.ends_with(".pane-tab"));
        stacked.collect()
    };
    let trace = {
        let hub = runtime.state.hub.try_borrow()?;
        let mut panes = hub.model.grid.panes.iter();
        let found = panes.find(|pane| pane.kind == PaneKind::Trace);
        found.map(|pane| pane.id.0).ok_or("no trace pane")?
    };
    let behind = tabs(&runtime);
    let counted = |runtime: &Runtime<Win>| {
        let all = runtime.elements();
        let chip = all.iter().find(|e| {
            let key = e.key.as_deref().unwrap_or_default();
            key.ends_with(".pane-tab-more")
        });
        let label = chip.and_then(|e| e.label.clone()).unwrap_or_default();
        let count = label
            .split(' ')
            .next()
            .and_then(|n| n.parse::<usize>().ok());
        count.unwrap_or(0)
    };
    let hidden = counted(&runtime);
    assert_eq!(
        behind.len() + hidden,
        2,
        "a tab or a count for each: {behind:?}"
    );
    // The tab nearest the pane in front always has room: Trace, opened just before Logs.
    let ends = |with: String| behind.iter().find(|key| key.ends_with(&with)).cloned();
    let on_trace = ends(format!(".tab-pane-{trace}.pane-tab")).ok_or("no trace tab")?;
    assert!(
        on_trace.starts_with("logs-"),
        "on the bar of the pane in front"
    );
    tap(&mut runtime, &on_trace)?;
    let shown = kinds(&runtime)?;
    assert!(
        shown.contains(&PaneKind::Trace),
        "its tab pressed: {shown:?}"
    );
    assert!(!shown.contains(&PaneKind::Logs));
    assert_eq!(
        tabs(&runtime).len() + counted(&runtime),
        2,
        "the other two are behind it now"
    );
    Ok(())
}

/// The session gets changes while a code pane is in front: Changes joins that slot as a tab
/// with a dot, nothing drawn moves, and the dot goes once the tab has been in front.
#[test]
fn a_kind_with_something_new_opens_as_a_tab_with_a_dot_and_moves_nothing() -> Check {
    use yd_ui::grid_view::view::{Act, act, show_pane};
    let mut runtime = settled("shell/mockup")?;
    runtime.update(|win, tx| {
        act(win, tx, Act::Show(yd_core::grid::saved::BLANK.slug));
        show_pane(win, tx, PaneKind::Code);
    });
    rest(&mut runtime);
    let frames = |runtime: &Runtime<Win>| -> Vec<(String, [f32; 4])> {
        let shown = runtime.elements();
        let live = shown.into_iter().filter(|e| !e.exiting);
        let frame = |e: ElementInfo| Some((e.key.filter(|k| k.ends_with(".pane-frame"))?, e.rect));
        live.filter_map(frame).collect()
    };
    let before = frames(&runtime);
    assert_eq!(kinds(&runtime)?, [PaneKind::Chat, PaneKind::Code]);
    let changes = yd_panes::registry::scenes().find(|scene| scene.name == "changes/mockup");
    (changes.ok_or("no changes scene")?.setup)(&mut *runtime.state.hub.try_borrow_mut()?);
    rest(&mut runtime);
    assert_eq!(frames(&runtime), before, "nothing drawn moved");
    let keyed = |runtime: &Runtime<Win>, end: &str| -> Vec<String> {
        let keys = runtime.elements().into_iter().filter_map(|e| e.key);
        keys.filter(|key| key.starts_with("code-") && key.ends_with(end))
            .collect()
    };
    let [dot] = &keyed(&runtime, ".pane-tab-dot")[..] else {
        return Err("the new tab carries no dot".into());
    };
    let tab = dot.replace(".pane-tab-dot", ".pane-tab");
    tap(&mut runtime, &tab)?;
    assert!(kinds(&runtime)?.contains(&PaneKind::Changes), "pressed");
    let back = runtime.elements().into_iter().filter_map(|e| e.key);
    let mut back = back.filter(|key| key.starts_with("changes-") && key.contains(".tab-pane-"));
    let back = back.find(|key| key.ends_with(".pane-tab"));
    tap(&mut runtime, &back.ok_or("no way back to the code pane")?)?;
    assert_eq!(keyed(&runtime, ".pane-tab-dot"), [] as [String; 0], "seen");
    assert_eq!(
        keyed(&runtime, ".pane-tab").len(),
        2,
        "and it is still a tab"
    );
    Ok(())
}

/// A column too narrow for every tab of its stack keeps the tabs nearest the pane drawn and
/// counts the rest in `+N`, which opens the Panes panel where each can be brought in front.
#[test]
fn a_bar_short_of_room_counts_the_tabs_it_has_no_room_for() -> Check {
    let mut runtime = settled("shell/mockup")?;
    for command in [
        "pane.show-agents",
        "pane.show-findings",
        "pane.show-plugins",
    ] {
        runtime.update(|win, tx| view::run(win, tx, CommandId(command), Target::App));
        rest(&mut runtime);
    }
    let shown = runtime.elements();
    let keys: Vec<&str> = shown.iter().filter_map(|e| e.key.as_deref()).collect();
    let more = keys.iter().find(|key| key.ends_with(".pane-tab-more"));
    let more = (*more.ok_or("no +N on the bar")?).to_owned();
    assert!(
        more.starts_with("plugins-"),
        "on the bar of the pane in front: {more}"
    );
    let chip = by_key(&shown, &more).ok_or("no chip")?;
    let tabs = keys.iter().filter(|key| key.ends_with(".pane-tab"));
    let stacked = tabs.filter(|key| key.starts_with("plugins-") && key.contains(".tab-pane-"));
    let drawn = stacked.count();
    assert!(
        drawn < 3,
        "three panes are behind it, and not all have a tab"
    );
    let hidden = 3 - drawn;
    assert_eq!(
        chip.label.as_deref(),
        Some(format!("{hidden} more tabs, in Panes").as_str())
    );
    tap(&mut runtime, &more)?;
    assert!(live(&runtime, PANEL), "the Panes panel lists every pane");
    Ok(())
}
