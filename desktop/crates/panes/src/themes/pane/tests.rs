//! Tests of the themes pane.

use std::error::Error;

use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, col};
use yd_core::ids::{ThemeId, WindowId};
use yd_core::tokens::{Rgba, ThemeFile, Tokens};
use yd_ui::pane::{PaneCx, SCENE_PANE};
use yd_ui::theme::files::{self, Library};
use yd_ui::theme::scale::{ON_PRIMARY, inks};
use yd_ui::win::{Hub, Win};

use crate::themes::editor::names;

fn window(setup: fn(&mut Hub)) -> Result<Runtime<Win>, Box<dyn Error>> {
    let shared = Hub::new(&[], Vec::new());
    setup(&mut *shared.try_borrow_mut()?);
    let mut rt = Runtime::new(
        Win::new(shared, WindowId(1)),
        alone,
        (1000.0, 1400.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    Ok(rt)
}

fn alone(win: &Win, cx: &mut Cx<Win>) -> El {
    match win.hub.try_borrow() {
        Ok(hub) => (super::DEF.view)(
            &PaneCx {
                hub: &hub,
                pane: SCENE_PANE,
                focused: true,
                edits: Some(&win.edits),
            },
            cx,
        ),
        Err(_) => col(),
    }
}

fn find(rt: &Runtime<Win>, key: &str) -> Result<rune::ElementInfo, Box<dyn Error>> {
    Ok(rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key))
        .ok_or(format!("no {key}"))?)
}

fn texts(rt: &Runtime<Win>) -> Vec<String> {
    rt.elements()
        .into_iter()
        .filter(|e| !e.exiting)
        .filter_map(|e| e.text)
        .collect()
}

fn settle(rt: &mut Runtime<Win>) {
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
}

fn press(rt: &mut Runtime<Win>, key: &str) -> Result<(), Box<dyn Error>> {
    let [x, y, w, h] = find(rt, key)?.rect;
    let (x, y) = (x + w / 2.0, y + h / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
    Ok(())
}

fn tokens(rt: &Runtime<Win>) -> Result<Tokens, Box<dyn Error>> {
    Ok(rt.state.hub.try_borrow()?.model.tokens.clone())
}

fn type_in(rt: &mut Runtime<Win>, words: &str) {
    rt.key(KeyEvent {
        key: Key::Text(words.into()),
        mods: Mods::default(),
    });
    rt.key(KeyEvent {
        key: Key::Enter,
        mods: Mods::default(),
    });
    settle(rt);
}

fn yi(hub: &mut Hub) {
    super::shown(hub, "yi");
}

#[test]
fn pressing_a_theme_puts_it_in_use_and_marks_its_row() -> Result<(), Box<dyn Error>> {
    let mut rt = window(yi)?;
    press(&mut rt, "themes-0.row.ink")?;
    let ink = Library::default()
        .theme(&ThemeId::new("ink"))
        .map(|t| t.tokens())
        .ok_or("ink")?;
    assert_eq!(tokens(&rt)?.accent, ink.accent);
    assert_eq!(tokens(&rt)?.window, ink.window);
    let fill = |rt: &Runtime<Win>, key: &str| find(rt, key).map(|e| e.fill);
    assert_ne!(
        fill(&rt, "themes-0.row.ink")?,
        fill(&rt, "themes-0.row.yi")?,
        "the row in use is lit"
    );
    Ok(())
}

#[test]
fn a_light_theme_cannot_be_put_in_use() -> Result<(), Box<dyn Error>> {
    let mut rt = window(yi)?;
    press(&mut rt, "themes-0.row.paper")?;
    assert_eq!(tokens(&rt)?, Tokens::default());
    Ok(())
}

#[test]
fn the_editor_shows_the_chosen_themes_values() -> Result<(), Box<dyn Error>> {
    let mut rt = window(yi)?;
    press(&mut rt, "themes-0.row.terminal")?;
    let shown = texts(&rt);
    for words in ["Terminal", "Geist Mono", "50%", "62%"] {
        assert!(
            shown.iter().any(|t| t == words),
            "{words} missing from {shown:?}"
        );
    }
    assert!(
        !shown.iter().any(|t| t == "System"),
        "the text font still reads System"
    );
    Ok(())
}

#[test]
fn roomy_sets_row_line_and_gutter_on_a_copy_and_counts_them() -> Result<(), Box<dyn Error>> {
    let mut rt = window(yi)?;
    press(&mut rt, "themes-0.density.2")?;
    let now = tokens(&rt)?;
    assert_eq!((now.row, now.line, now.gutter), (24.0, 18.0, 10.0));
    let shown = texts(&rt);
    assert!(
        shown.iter().any(|t| t == "3 changed from the base"),
        "{shown:?}"
    );
    assert!(
        shown.iter().any(|t| t == "Yi Dark copy"),
        "the built-in was edited in place"
    );
    Ok(())
}

#[test]
fn a_typed_value_that_does_not_fit_is_refused_by_name() -> Result<(), Box<dyn Error>> {
    let mut rt = window(yi)?;
    press(&mut rt, "themes-0.value.accent.set")?;
    type_in(&mut rt, "sky");
    assert_eq!(tokens(&rt)?.accent, Tokens::default().accent);
    let shown = texts(&rt);
    assert!(
        shown
            .iter()
            .any(|t| t.starts_with("accent: ") && t.ends_with("kept as it was")),
        "{shown:?}"
    );
    press(&mut rt, "themes-0.value.accent.set")?;
    for _ in 0..12 {
        rt.key(KeyEvent {
            key: Key::Backspace,
            mods: Mods::default(),
        });
    }
    type_in(&mut rt, "#ff8000");
    assert_eq!(Some(tokens(&rt)?.accent), Rgba::parse("#ff8000"));
    Ok(())
}

#[test]
fn a_file_that_does_not_fit_is_named_and_its_old_value_kept() -> Result<(), Box<dyn Error>> {
    let rt = window(super::bad_file)?;
    assert_eq!(Some(tokens(&rt)?.accent), Rgba::parse("#7fc4e8"));
    let shown = texts(&rt);
    for words in [
        "sea.json: accent kept as it was",
        "notes.json: not a theme file",
    ] {
        assert!(
            shown.iter().any(|t| t.starts_with(words)),
            "{words} missing from {shown:?}"
        );
    }
    Ok(())
}

#[test]
fn the_editor_lists_every_token_and_named_colour_once() {
    let mut listed = names();
    listed.sort_unstable();
    let mut all = Tokens::NAMES.to_vec();
    all.extend(inks().map(|ink| ink.0));
    all.sort_unstable();
    assert_eq!(listed, all);
}

fn clear(rt: &mut Runtime<Win>) {
    for _ in 0..30 {
        rt.key(KeyEvent {
            key: Key::Backspace,
            mods: Mods::default(),
        });
    }
}

#[test]
fn a_named_colour_is_edited_written_and_read_back() -> Result<(), Box<dyn Error>> {
    let mut rt = window(yi)?;
    let field = "themes-0.value.text-on_primary.set";
    press(&mut rt, field)?;
    clear(&mut rt);
    type_in(&mut rt, "sky");
    assert_eq!(ON_PRIMARY.of(&tokens(&rt)?), ON_PRIMARY.1);
    let refused = |t: &String| t.starts_with("text.on_primary: ") && t.ends_with("kept as it was");
    assert!(
        texts(&rt).iter().any(refused),
        "a word was taken for a colour"
    );
    press(&mut rt, field)?;
    clear(&mut rt);
    type_in(&mut rt, "#ff8000");
    let orange = Rgba::parse("#ff8000");
    assert_eq!(Some(ON_PRIMARY.of(&tokens(&rt)?)), orange);
    let mut hub = rt.state.hub.try_borrow_mut()?;
    let library = files::library(&mut hub).ok_or("no library")?;
    let file = &library.theme(&library.active).ok_or("no theme")?.file;
    let again = ThemeFile::parse(&file.to_text()?)?;
    assert_eq!(Some(ON_PRIMARY.of(&Tokens::themed(&again).0)), orange);
    Ok(())
}
