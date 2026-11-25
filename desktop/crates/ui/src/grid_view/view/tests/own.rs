//! A pane that keeps its own tabs: the one bar draws them, and the pane hears every press.

use rune::{Cx, El, Env, Key, Mods, Runtime, col};
use yd_core::address::uri::Address;
use yd_core::feed::{FILES, OPEN_FILES, SourceCmd};
use yd_core::ids::{PaneId, TabId};
use yd_core::kind::Kind;
use yd_core::msg::Effect;
use yd_core::pane::PaneKind::{Changes, Chat, Code, Files, Terminal};
use yd_core::row::Rows;

use super::{Check, DT, SIZE, Window, click, def, find, key, middle, pane_of, part, press_key};
use super::{rest, window_of};
use crate::chrome::tabs::{Tab, Tabs};
use crate::grid_view::drag::Gesture;
use crate::grid_view::view::{alone, focused, open_file};
use crate::pane::{PaneCx, PaneDef, SCENE_PANE, Stage, TabAsk};
use crate::win::{Hub, Win};

/// What the test pane keeps: the tab in front, and everything the bar asked of it.
#[derive(Debug, Default)]
struct Kept {
    front: Option<TabId>,
    heard: Vec<TabAsk>,
}

fn own(pane: &PaneCx<'_>) -> Tabs {
    let tab = |id: u32, title: &str| Tab {
        id: TabId(id),
        title: title.to_owned(),
        meta: format!("meta of {title}"),
        ..Tab::of(Terminal)
    };
    Tabs {
        tabs: vec![tab(1, "one"), tab(2, "two")],
        active: pane.state::<Kept>().and_then(|kept| kept.front),
        adds: true,
        tool: None,
    }
}

fn hears(win: &mut Win, pane: PaneId, ask: TabAsk) {
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(kept) = hub.panes.get_mut::<Kept>(Some(pane))
    {
        kept.heard.push(ask);
        if let TabAsk::Show(tab) = ask {
            kept.front = Some(tab);
        }
    }
}

static OWN: PaneDef = PaneDef {
    tabs: Some(own),
    on_tab: Some(hears),
    finder: None,
    ..def(Terminal)
};
static DEFS: &[&PaneDef] = &[&def(Files), &def(Changes), &OWN, &def(Code), &def(Chat)];

fn heard(window: &Window, pane: PaneId) -> Result<Vec<TabAsk>, Box<dyn std::error::Error>> {
    let hub = window.state.hub.try_borrow()?;
    let kept = hub.panes.get::<Kept>(Some(pane));
    Ok(kept.map(|kept| kept.heard.clone()).unwrap_or_default())
}

fn press(window: &mut Window, key: &str) -> Check {
    let at = middle(find(&window.elements(), key)?.rect);
    click(window, at);
    rest(window);
    Ok(())
}

#[test]
fn a_panes_own_tabs_and_meta_are_drawn_in_the_one_bar() -> Check {
    let (window, _) = window_of(DEFS, "code")?;
    let pane = key(&window, Terminal, "")?;
    let tree = window.elements();
    let bars = tree.iter().filter(|element| {
        let key = element.key.as_deref().unwrap_or_default();
        key.starts_with(&pane) && key.ends_with("pane-tab-bar")
    });
    assert_eq!(bars.count(), 1, "one bar a pane");
    for (tab, title) in [("tab-1.pane-tab", "one"), ("tab-2.pane-tab", "two")] {
        let tab = find(&tree, &format!("{pane}{tab}"))?;
        let words = tree
            .iter()
            .filter(|element| element.text.as_deref() == Some(title));
        let inside =
            |word: &&rune::ElementInfo| word.rect[0] >= tab.rect[0] && word.rect[1] >= tab.rect[1];
        assert!(
            words.clone().any(|word| inside(&word)),
            "{title} is not in its tab"
        );
    }
    assert_eq!(
        part(&window, Terminal, "pane-meta")?.text.as_deref(),
        Some("meta of one"),
        "the tab in front names the meta"
    );
    find(&tree, &format!("{pane}pane-tab-new"))?;
    assert!(
        !tree
            .iter()
            .any(|element| element.text.as_deref() == Some("zsh")),
        "the tabs kept for the pane give way to its own"
    );
    Ok(())
}

#[test]
fn the_pane_hears_a_press_a_turn_a_close_and_the_plus() -> Check {
    let (mut window, _) = window_of(DEFS, "code")?;
    let pane = pane_of(&window, Terminal)?;
    let second = key(&window, Terminal, "tab-2.pane-tab")?;
    press(&mut window, &second)?;
    assert_eq!(heard(&window, pane)?, [TabAsk::Show(TabId(2))]);
    assert_eq!(
        part(&window, Terminal, "pane-meta")?.text.as_deref(),
        Some("meta of two")
    );
    let hub = window.state.hub.try_borrow()?;
    assert_eq!(
        focused(&hub, window.state.window),
        Some(pane),
        "and has the keys"
    );
    drop(hub);
    let (cmd, shift) = (
        Mods {
            cmd: cfg!(target_os = "macos"),
            logo: true,
            ..Mods::default()
        },
        true,
    );
    press_key(&mut window, Key::Command(']'), Mods { shift, ..cmd });
    press_key(&mut window, Key::Command('w'), cmd);
    let plus = key(&window, Terminal, "pane-tab-new")?;
    press(&mut window, &plus)?;
    assert_eq!(
        heard(&window, pane)?.get(1..),
        Some(&[TabAsk::Show(TabId(1)), TabAsk::Close(TabId(1)), TabAsk::New][..]),
        "the next tab goes round to the first, which then is the one closed"
    );
    Ok(())
}

fn lone(win: &Win, cx: &mut Cx<Win>) -> El {
    win.hub
        .try_borrow()
        .map_or_else(|_| col(), |hub| alone(win, &hub, cx, Terminal))
}

#[test]
fn a_pane_drawn_alone_has_the_same_bar_and_is_heard_the_same() -> Check {
    let hub = Hub::new(DEFS, Vec::new());
    hub.try_borrow_mut()?.stage = Stage::Pane(Terminal);
    let win = Win::new(hub, super::WINDOW);
    let mut window: Window = Runtime::new(win, lone, SIZE, Env::new(true, 1.0));
    window.frame(DT);
    let bar = find(&window.elements(), "terminal-0.pane-tab-bar")?.rect;
    assert_eq!((bar[1], bar[2], bar[3]), (0.0, SIZE.0, 20.0));
    press(&mut window, "terminal-0.tab-2.pane-tab")?;
    assert_eq!(heard(&window, SCENE_PANE)?, [TabAsk::Show(TabId(2))]);
    Ok(())
}

#[test]
fn opening_a_file_gives_it_a_tab_asks_for_its_text_and_focuses_the_viewer() -> Check {
    let (mut window, asked) = window_of(DEFS, "code")?;
    let files = middle(part(&window, Files, "pane-frame")?.rect);
    click(&mut window, files);
    let file = Address::new(Kind::File, "src/routes/upload.rs")?;
    window.update(|win, tx| open_file(win, tx, file));
    rest(&mut window);
    let hub = window.state.hub.try_borrow()?;
    let open = hub.model.tables.get(OPEN_FILES).map_or(0, Rows::len);
    assert_eq!(open, 1, "the file has its tab");
    let read = SourceCmd::Read("src/routes/upload.rs".into());
    assert!(asked.borrow().contains(&Effect::Source(FILES, read)));
    let viewer = hub.model.grid.panes.iter().find(|pane| pane.kind == Code);
    assert_eq!(
        focused(&hub, window.state.window),
        viewer.map(|pane| pane.id)
    );
    Ok(())
}

#[test]
fn the_cross_on_an_own_tab_closes_that_tab_whichever_is_in_front() -> Check {
    let (mut window, _) = window_of(DEFS, "code")?;
    let pane = pane_of(&window, Terminal)?;
    let cross = key(&window, Terminal, "tab-2.pane-tab-close")?;
    press(&mut window, &cross)?;
    assert_eq!(
        heard(&window, pane)?,
        [TabAsk::Close(TabId(2))],
        "tab one stays in front; the pane is asked to close two"
    );
    Ok(())
}

/// The owner: "clicking a file tab starts the move panel animation, it shouldnt". A press and (codespell:ignore shouldnt)
/// release with no move only shows the tab: nothing lifts, dims or springs home.
#[test]
fn a_click_on_a_tab_only_shows_it_and_never_carries_the_pane() -> Check {
    let (mut window, _) = window_of(DEFS, "code")?;
    let pane = pane_of(&window, Terminal)?;
    // A pane whose tabs are all its own keeps none in the hub, as the code viewer does.
    window.update(|win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut()
            && let Some(kept) = hub.panes.get_mut::<Tabs>(Some(pane))
        {
            *kept = Tabs::default();
        }
    });
    rest(&mut window);
    for (kind, tab) in [(Terminal, "tab-2.pane-tab"), (Changes, "tab-0.pane-tab")] {
        let home = part(&window, kind, "pane-frame")?.rect;
        let (x, y) = middle(part(&window, kind, tab)?.rect);
        window.pointer_move(x, y);
        window.pointer_down(x, y);
        for _ in 0..12 {
            window.frame(DT);
        }
        let held = super::read(&window.state).gesture;
        assert!(
            !matches!(held, Gesture::Pane(_)),
            "{kind:?}: a press holds {held:?}"
        );
        let frame = part(&window, kind, "pane-frame")?;
        assert!(
            (frame.scale - 1.0).abs() < 0.001,
            "{kind:?} lifts to {}",
            frame.scale
        );
        assert!((part(&window, Files, "pane-frame")?.opacity - 1.0).abs() < 0.01);
        window.pointer_up(x, y);
        window.frame(DT);
        let after = super::read(&window.state).gesture;
        assert_eq!(after, Gesture::Idle, "{kind:?}: let go it is {after:?}");
        rest(&mut window);
        assert_eq!(part(&window, kind, "pane-frame")?.rect, home);
    }
    assert_eq!(heard(&window, pane)?, [TabAsk::Show(TabId(2))]);
    Ok(())
}
