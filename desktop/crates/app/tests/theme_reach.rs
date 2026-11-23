//! A theme restyles everything: under a theme that gives every colour its own odd value, no
//! element of the whole window still draws a colour a view used to spell for itself.

mod support;

use std::error::Error;

use rune::Color;
use yd_core::msg::Msg;
use yd_core::tokens::{Rgba, ThemeValue, Tokens, rgba};
use yd_ui::theme::look::colour;
use yd_ui::theme::scale::inks;
use yd_ui::win::{Win, dispatch};

use support::scene;

type Check = Result<(), Box<dyn Error>>;

/// Every colour value a theme names: the shared table's, then each named colour's.
fn colour_names() -> Vec<String> {
    let table = Tokens::default()
        .values()
        .into_iter()
        .filter_map(|(name, value)| {
            matches!(value, ThemeValue::Text(text) if Rgba::parse(&text).is_some())
                .then(|| name.to_owned())
        });
    table.chain(inks().map(|ink| ink.0.to_owned())).collect()
}

/// Colour `n` of the odd theme: no two alike, and no view's old colour among them.
fn odd(n: usize) -> Rgba {
    let red = u8::try_from(n.saturating_mul(2).saturating_add(3)).unwrap_or(u8::MAX);
    rgba(red, 77, 151, 0.61)
}

#[test]
fn no_element_of_the_whole_window_draws_a_colour_the_theme_cannot_set() -> Check {
    let old: Vec<Color> = inks()
        .map(|ink| colour(ink.1))
        .filter(|old| old.a > 0.0)
        .collect();
    let mut tokens = Tokens::default();
    let names = colour_names();
    for (n, name) in names.iter().enumerate() {
        tokens.set(name, &ThemeValue::Text(odd(n).text()))?;
    }
    for (name, strength) in [("lines", 0.0713), ("lines.inner", 0.0457)] {
        tokens.set(name, &ThemeValue::Number(strength))?;
    }
    let odds: Vec<Color> = (0..names.len()).map(|n| colour(odd(n))).collect();
    assert!(
        !odds.iter().any(|c| old.contains(c)),
        "an odd colour is an old one"
    );

    let mut runtime = scene::runtime("shell/everything", scene::WINDOW)?;
    runtime.update(|win: &mut Win, _| dispatch(win, Msg::Theme(Box::new(tokens))));
    // A theme switch blends colours over frames; two seconds lets every blend land.
    for _ in 0..120 {
        runtime.frame(1.0 / 60.0);
    }
    let elements = runtime.elements();
    // A web page and a remote screen draw the far side's own colours: the theme is the app's.
    let theirs = |path: &str| path.contains(".page.words/") || path.contains(".screen/");
    let stale: Vec<String> = elements
        .iter()
        .filter(|e| !theirs(&e.path))
        .filter(|e| old.contains(&e.fill) || e.text_color.is_some_and(|c| old.contains(&c)))
        .map(|e| {
            format!(
                "{} {:?} {:?}",
                e.key.as_deref().unwrap_or(&e.path),
                e.fill,
                e.text_color
            )
        })
        .collect();
    assert!(
        stale.is_empty(),
        "{} elements still draw an old colour: {stale:#?}",
        stale.len()
    );
    let themed = elements
        .iter()
        .filter(|e| odds.contains(&e.fill) || e.text_color.is_some_and(|c| odds.contains(&c)))
        .count();
    assert!(
        themed > 300,
        "the odd theme was not drawn: {themed} elements in its colours"
    );
    Ok(())
}
