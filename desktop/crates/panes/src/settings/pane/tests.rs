//! What a user sees and does in the settings pane, drawn in process with no GPU.

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use rune::{ElementInfo, Env, Key, KeyEvent, Mods, Runtime};
use yd_core::ids::WindowId;
use yd_core::msg::Effect;
use yd_core::row::{Value, rows_from_json};
use yd_core::settings::schema::SettingValue;
use yd_ui::pane::{PaneCx, PaneDef, SCENE_PANE};
use yd_ui::win::{Hub, Win};

use super::{DEF, scenes};

type Check = Result<(), Box<dyn Error>>;

const PANE: &str = "settings-0";
const MOCKUP: &str = include_str!("../../../../../fixtures/mockup/settings.rows.json");

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
        (1200.0, 750.0),
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

fn find<'a>(tree: &'a [ElementInfo], key: &str) -> Result<&'a ElementInfo, Box<dyn Error>> {
    let found = tree.iter().find(|e| e.key.as_deref() == Some(key));
    found.ok_or_else(|| format!("no element keyed {key}").into())
}

fn tap(rt: &mut Runtime<Win>, key: &str) -> Check {
    let [x, y, w, h] = find(&rt.elements(), key)?.rect;
    let (x, y) = (x + w / 2.0, y + h / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
    Ok(())
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

/// Replaces everything in the text view with `words`.
fn type_text(rt: &mut Runtime<Win>, words: &str) -> Check {
    tap(rt, &format!("{PANE}.text.input"))?;
    press(rt, Key::Command('a'), true);
    press(rt, Key::Text(words.to_owned()), false);
    Ok(())
}

/// The words of a keyed element with every run of blanks one space, so alignment is no matter.
fn words(rt: &Runtime<Win>, key: &str) -> Result<String, Box<dyn Error>> {
    let text = find(&rt.elements(), key)?.text.clone().unwrap_or_default();
    Ok(text
        .split(' ')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" "))
}

fn value(rt: &Runtime<Win>, key: &str) -> Result<Option<SettingValue>, Box<dyn Error>> {
    Ok(rt
        .state
        .hub
        .try_borrow()?
        .model
        .settings
        .value(key)
        .cloned())
}

fn sink(rt: &Runtime<Win>) -> Result<Rc<RefCell<Vec<String>>>, Box<dyn Error>> {
    let sent = Rc::new(RefCell::new(Vec::new()));
    let log = Rc::clone(&sent);
    rt.state.hub.try_borrow_mut()?.set_sink(move |effect| {
        if let Effect::Wire(request) = effect {
            log.borrow_mut().push(request.line);
        }
    });
    Ok(sent)
}

fn tab_of(rt: &Runtime<Win>, id: u32) -> Result<(String, Option<u32>), Box<dyn Error>> {
    let hub = rt.state.hub.try_borrow()?;
    let pane = PaneCx {
        hub: &hub,
        pane: SCENE_PANE,
        focused: true,
        edits: None,
    };
    let tabs = (DEF.tabs.ok_or("no tabs")?)(&pane);
    let tab = tabs
        .tabs
        .iter()
        .find(|tab| tab.id.0 == id)
        .ok_or("no such tab")?;
    Ok((tab.title.clone(), tab.count))
}

#[test]
fn the_counts_are_the_rows_that_differ_and_not_the_mockups_nine() -> Check {
    let rt = draw(scenes::SCENES[0].setup)?;
    let rows = rows_from_json(MOCKUP)?;
    let flagged = |prefix: &str| {
        let marked = |row: &&yd_core::row::Row| row.get("changed") == Some(&Value::Flag(true));
        rows.iter()
            .filter(marked)
            .filter(|row| row.id.as_str().starts_with(prefix))
            .count()
    };
    assert_eq!(flagged(""), 7, "the mockup's own rows mark seven");
    assert_eq!(tab_of(&rt, 2)?.1, Some(7), "the Changed tab counts them");
    assert_eq!(
        tab_of(&rt, 1)?.1,
        u32::try_from(rows.len()).ok(),
        "All counts every row"
    );
    let tree = rt.elements();
    let count = |key: &str, want: usize| -> Check {
        let at = find(&tree, key)?.rect;
        let inside =
            |e: &&ElementInfo| e.rect[1] >= at[1] && e.rect[1] < at[1] + at[3] && e.rect[0] > at[0];
        let shown = tree
            .iter()
            .filter(inside)
            .any(|e| e.text.as_deref() == Some(&want.to_string()));
        shown
            .then_some(())
            .ok_or_else(|| format!("{key} does not show {want}").into())
    };
    count(&format!("{PANE}.group.Appearance"), flagged("appearance."))?;
    count(&format!("{PANE}.group.Layout"), flagged("layout."))?;
    count(&format!("{PANE}.group.Motion"), flagged("motion."))?;
    Ok(())
}

#[test]
fn pressing_a_toggle_rewrites_the_matching_line_of_the_text() -> Check {
    let mut rt = draw(scenes::SCENES[0].setup)?;
    let text = format!("{PANE}.text.input");
    assert!(words(&rt, &text)?.contains("moving_light = false"));
    assert!(!words(&rt, &text)?.contains("springs"));
    tap(&mut rt, &format!("{PANE}.control.motion.springs"))?;
    assert!(
        words(&rt, &text)?.contains("springs = false"),
        "{}",
        words(&rt, &text)?
    );
    tap(&mut rt, &format!("{PANE}.control.motion.moving_light"))?;
    assert!(
        !words(&rt, &text)?.contains("moving_light"),
        "back at its default, the line goes"
    );
    assert_eq!(
        value(&rt, "motion.moving_light")?,
        Some(SettingValue::Flag(true))
    );
    Ok(())
}

#[test]
fn a_line_typed_in_the_text_sets_the_setting_and_moves_its_slider() -> Check {
    let mut rt = draw(scenes::SCENES[0].setup)?;
    type_text(&mut rt, "[appearance]\nglass = 0.3\n")?;
    assert_eq!(
        value(&rt, "appearance.glass")?,
        Some(SettingValue::Number(0.3))
    );
    let readout = rt
        .elements()
        .iter()
        .any(|e| e.text.as_deref() == Some("30%"));
    assert!(readout, "the slider's reading follows the line");
    Ok(())
}

#[test]
fn a_line_that_does_not_fit_is_marked_by_name_and_applies_nothing() -> Check {
    let mut rt = draw(scenes::SCENES[0].setup)?;
    type_text(&mut rt, "[appearance]\nglass = 60\n")?;
    assert_eq!(
        value(&rt, "appearance.glass")?,
        Some(SettingValue::Number(0.6))
    );
    let tree = rt.elements();
    let said = tree.iter().any(|e| {
        e.text.as_deref()
            == Some("Line 2 is not applied: appearance.glass takes a number from 0 to 1")
    });
    assert!(said, "the footer names the line and the setting");
    Ok(())
}

#[test]
fn the_bad_line_scene_shows_the_line_it_refused_and_the_value_kept() -> Check {
    let rt = draw(scenes::SCENES[2].setup)?;
    assert!(words(&rt, &format!("{PANE}.text.input"))?.contains("glass = 60"));
    assert_eq!(
        value(&rt, "appearance.glass")?,
        Some(SettingValue::Number(0.6))
    );
    let tree = rt.elements();
    assert!(tree.iter().any(|e| {
        e.text
            .as_deref()
            .is_some_and(|t| t.starts_with("Line 7 is not applied"))
    }));
    Ok(())
}

#[test]
fn a_search_lists_every_match_across_the_groups_and_nothing_else() -> Check {
    let rt = draw(scenes::SCENES[1].setup)?;
    let tree = rt.elements();
    find(&tree, &format!("{PANE}.row.appearance.text_font"))?;
    find(&tree, &format!("{PANE}.row.appearance.code_font"))?;
    assert!(find(&tree, &format!("{PANE}.row.appearance.density")).is_err());
    assert!(
        tree.iter()
            .any(|e| e.text.as_deref() == Some("2 found for “font”"))
    );
    Ok(())
}

#[test]
fn a_reset_puts_the_default_back_and_the_changed_mark_goes() -> Check {
    let mut rt = draw(scenes::SCENES[0].setup)?;
    tap(&mut rt, &format!("{PANE}.undo.appearance.density"))?;
    assert_eq!(
        value(&rt, "appearance.density")?,
        Some(SettingValue::Text("dense".into()))
    );
    assert_eq!(tab_of(&rt, 2)?.1, Some(6));
    assert!(!words(&rt, &format!("{PANE}.text.input"))?.contains("density"));
    Ok(())
}

#[test]
fn typing_into_a_colour_row_sets_it_on_enter_refuses_a_non_colour_and_escape_leaves_it() -> Check {
    let mut rt = draw(scenes::SCENES[0].setup)?;
    let socket = format!("{PANE}.control.appearance.needs_you");
    tap(&mut rt, &socket)?;
    press(&mut rt, Key::Command('a'), true);
    press(&mut rt, Key::Text("#112233".into()), false);
    press(&mut rt, Key::Escape, false);
    assert_eq!(
        value(&rt, "appearance.needs_you")?,
        Some(SettingValue::Text("#78d2c6".into()))
    );
    tap(&mut rt, &socket)?;
    press(&mut rt, Key::Command('a'), true);
    press(&mut rt, Key::Text("red".into()), false);
    press(&mut rt, Key::Enter, false);
    assert_eq!(
        value(&rt, "appearance.needs_you")?,
        Some(SettingValue::Text("#78d2c6".into())),
        "a word that is no colour is refused"
    );
    let said = rt
        .elements()
        .iter()
        .any(|e| e.text.as_deref() == Some("appearance.needs_you takes a colour such as #78d2c6"));
    assert!(said, "and the refusal names the setting");
    press(&mut rt, Key::Command('a'), true);
    press(&mut rt, Key::Text("#112233".into()), false);
    press(&mut rt, Key::Enter, false);
    assert_eq!(
        value(&rt, "appearance.needs_you")?,
        Some(SettingValue::Text("#112233".into()))
    );
    Ok(())
}

#[test]
fn pressing_a_toggle_of_yis_asks_the_server_and_leaves_the_row_to_its_answer() -> Check {
    let mut rt = draw(scenes::SCENES[3].setup)?;
    let sent = sink(&rt)?;
    tap(&mut rt, &format!("{PANE}.control.classifier.approve"))?;
    let lines = sent.borrow();
    let write = lines
        .iter()
        .find(|line| line.contains("_yi/set_default"))
        .ok_or("nothing sent")?;
    assert!(
        write.contains(r#""id":"classifier.approve""#) && write.contains(r#""value":false"#),
        "{write}"
    );
    Ok(())
}

/// A pane's second field is its own: it is no other pane's field, whatever that pane's id.
#[test]
fn the_search_box_of_one_pane_is_not_the_composer_of_another() -> Check {
    use yd_core::ids::PaneId;

    use super::act;
    use super::state::Part;

    let mut win = Win::new(Hub::new(&[], Vec::new()), WindowId(0));
    act::edit(&mut win, PaneId(3), Part::Search).insert("font");
    let hub = win.hub.try_borrow()?;
    let chat = PaneCx {
        hub: &hub,
        pane: PaneId(3 | (1 << 28)),
        focused: true,
        edits: Some(&win.edits),
    };
    let composer = crate::chat::composer::drawn(&chat).map(|field| field.text.clone());
    assert_eq!(composer, None, "another pane's composer holds the search");
    Ok(())
}

fn own_keys(hub: &mut Hub) {
    use yd_core::ids::SettingKey;
    use yd_core::msg::{Msg, step};
    use yd_core::settings::store::SettingsMsg;
    use yd_ui::menu::command_box::table;

    let rows = yd_core::command::keys::settings(&table(hub));
    step(&mut hub.model, Msg::Settings(SettingsMsg::Declare(rows)));
    let text = |text: &str| Some(SettingValue::Text(text.to_owned().into()));
    for (id, key) in [
        ("keys.command.open", "cmd+bogus"),
        ("keys.sidebar.toggle", "cmd+shift+b"),
    ] {
        let set = SettingsMsg::Set(SettingKey::new(id), text(key));
        step(&mut hub.model, Msg::Settings(set));
    }
}

#[test]
fn the_keys_group_lists_the_map_marks_the_users_keys_and_names_a_bad_one() -> Check {
    let mut rt = draw(own_keys)?;
    tap(&mut rt, &format!("{PANE}.group.Keys"))?;
    let tree = rt.elements();
    let row = |id: &str| find(&tree, &format!("{PANE}.row.keys.{id}"));
    let fold = row("sidebar.toggle")?;
    assert!(
        words_of(&tree, fold, "Toggle Sidebar"),
        "a row for each command"
    );
    let open = find(&tree, &format!("{PANE}.control.keys.sidebar.toggle"))?;
    assert_eq!(open.label.as_deref(), Some("Toggle Sidebar"));
    find(&tree, &format!("{PANE}.undo.keys.sidebar.toggle"))?;
    let saw = |text: &str| tree.iter().any(|e| e.text.as_deref() == Some(text));
    assert!(saw("cmd+shift+b"), "the user's key is the row's value");
    assert!(
        saw("alt+cmd+s") || saw("keys.sidebar.toggle"),
        "its default is findable"
    );
    let notice = find(&tree, &format!("{PANE}.notice"))?;
    let said =
        r#"keys.command.open: "cmd+bogus" is not a key, such as cmd+shift+k; the default stays"#;
    assert!(
        words_of(&tree, notice, said),
        "a bad chord is said, not ignored"
    );
    Ok(())
}

/// Whether `inside` holds a text equal to `said`.
fn words_of(tree: &[ElementInfo], inside: &ElementInfo, said: &str) -> bool {
    let [x, y, w, h] = inside.rect;
    let within = |e: &&ElementInfo| e.rect[0] >= x && e.rect[1] >= y;
    let within = |e: &&ElementInfo| within(e) && e.rect[0] < x + w && e.rect[1] < y + h;
    tree.iter()
        .filter(within)
        .any(|e| e.text.as_deref() == Some(said))
}

const LISTS: &str = include_str!("../../../../../fixtures/wire/lists.jsonl");

/// Incident: Settings never asked Yi for its rows, so a search for the effort found nothing.
#[test]
fn settings_asks_yi_for_its_rows_and_a_search_for_effort_finds_thinking() -> Check {
    static DEFS: [&PaneDef; 1] = [&DEF];
    let hub = Hub::new(&DEFS, Vec::new());
    // The recording up to where its client asked for Yi's settings.
    let connected: Vec<&str> = LISTS.lines().take(39).collect();
    yd_core::wire::replay::play(&mut hub.try_borrow_mut()?.model, &connected.join("\n"))?;
    let mut rt = Runtime::new(
        Win::new(hub, WindowId(0)),
        root,
        (1200.0, 750.0),
        Env::new(true, 1.0),
    );
    let sent = sink(&rt)?;
    settle(&mut rt);
    let asked = sent
        .borrow()
        .iter()
        .find(|l| l.contains("_yi/settings"))
        .cloned();
    let asked = asked.ok_or("the pane never asked for Yi's settings")?;
    let id = asked
        .split(r#""id":"#)
        .nth(1)
        .and_then(|rest| rest.split(',').next());
    let answer = LISTS.lines().nth(40).ok_or("no recorded answer")?;
    let answer = answer.replacen(r#""id":9,"#, &format!(r#""id":{},"#, id.unwrap_or("")), 1);
    {
        let mut hub = rt.state.hub.try_borrow_mut()?;
        yd_core::wire::replay::play_line(&mut hub.model, &answer)?;
        let state = hub.panes.get_mut::<super::state::State>(Some(SCENE_PANE));
        state.ok_or("no pane state")?.search = "effort".to_owned();
    }
    rt.invalidate();
    settle(&mut rt);
    find(&rt.elements(), &format!("{PANE}.row.thinking"))?;
    Ok(())
}

/// Dies with a long query that draws past its box and over the bar beside it: the field grew
/// to the width of its words.
#[test]
fn a_long_search_stays_inside_the_search_box() -> Check {
    let mut rt = draw(scenes::SCENES[0].setup)?;
    tap(&mut rt, &format!("{PANE}.search.input"))?;
    let long = "a long search that goes on and on well past the width of the box ".repeat(3);
    press(&mut rt, Key::Text(long), false);
    let tree = rt.elements();
    let [x, _, w, _] = find(&tree, &format!("{PANE}.search"))?.rect;
    let [fx, _, fw, _] = find(&tree, &format!("{PANE}.search.input"))?.rect;
    assert!(
        fx + fw <= x + w,
        "the field ends at {} and its box at {}",
        fx + fw,
        x + w
    );
    let [nx, _, nw, _] = find(&tree, &format!("{PANE}.notice"))?.rect;
    let [cx, _, cw, _] = find(&tree, &format!("{PANE}.settings-column"))?.rect;
    assert!(
        nx + nw <= cx + cw + 0.5,
        "the notice runs {} past its column",
        nx + nw - cx - cw
    );
    let words = tree.iter().find(|e| {
        e.text
            .as_deref()
            .is_some_and(|t| t.starts_with("No setting matches"))
    });
    let [tx, _, tw, _] = words.ok_or("no notice words")?.rect;
    assert!(
        tx + tw <= cx + cw + 0.5,
        "the words run {} past the column",
        tx + tw - cx - cw
    );
    Ok(())
}
