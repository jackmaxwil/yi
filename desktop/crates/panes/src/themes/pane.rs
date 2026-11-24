//! Themes pane: the theme list and the editor for every value.

use std::borrow::Cow;

use rune::{Cursor, Cx, El, Icon, ScrollSpec, TextStyle, Tx, col, el, row, text};
use yd_core::address::thing::Thing;
use yd_core::address::uri::Address;
use yd_core::ids::{PaneId, SettingKey, ThemeId};
use yd_core::kind::Kind;
use yd_core::pane::PaneKind;
use yd_core::settings::schema::{Choice, Control, Setting, SettingValue, Store};
use yd_core::tokens::Tokens;
use yd_ui::kit::bars::{footer, header, lead};
use yd_ui::kit::chip::{Chip, chip};
use yd_ui::kit::gallery;
use yd_ui::kit::label::section;
use yd_ui::line_icon;
use yd_ui::overlay::pick::Finder;
use yd_ui::pane::{PaneCx, PaneDef, Scene, Stage};
use yd_ui::theme::files::{self, Library, Theme};
use yd_ui::theme::look::{BODY, LABEL, META, colour, white};
use yd_ui::theme::scale::{LINE_PICKED, SEMIBOLD};
use yd_ui::win::{Hub, Win};

use crate::themes::editor;

#[cfg(test)]
mod tests;

static PLUS: Icon = line_icon!("plus", r#"<path d="M12 5v14M5 12h14"/>"#);

const NAME: TextStyle = TextStyle {
    weight: SEMIBOLD,
    ..BODY
};
/// The list's width; the editor takes the rest (record 7.4).
const LIST_W: f32 = 216.0;

pub const DEF: PaneDef = PaneDef {
    kind: PaneKind::Themes,
    view,
    scenes: &[
        Scene {
            name: "themes/mockup",
            stage: Stage::Pane(PaneKind::Themes),
            setup: |hub| shown(hub, "yi"),
        },
        // The same pane under the editor's own fixture: parity reads a scene by its name.
        Scene {
            name: "theme-tokens/mockup",
            stage: Stage::Pane(PaneKind::Themes),
            setup: |hub| shown(hub, "yi"),
        },
        Scene {
            name: "themes/yi",
            stage: Stage::View(gallery::view),
            setup: |hub| shown(hub, "yi"),
        },
        Scene {
            name: "themes/graphite",
            stage: Stage::View(gallery::view),
            setup: |hub| shown(hub, "graphite"),
        },
        Scene {
            name: "themes/ink",
            stage: Stage::View(gallery::view),
            setup: |hub| shown(hub, "ink"),
        },
        Scene {
            name: "themes/terminal",
            stage: Stage::View(gallery::view),
            setup: |hub| shown(hub, "terminal"),
        },
        Scene {
            name: "themes/contrast",
            stage: Stage::View(gallery::view),
            setup: |hub| shown(hub, "contrast"),
        },
        Scene {
            name: "themes/bad-file",
            stage: Stage::Pane(PaneKind::Themes),
            setup: bad_file,
        },
    ],
    commands: &[],
    settings: SETTINGS,
    tabs: None,
    on_tab: None,
    finder: Some(Finder::Keyed(thing_at)),
};

pub const THEME: &str = "appearance.theme";
pub const DENSITY: &str = "appearance.density";
/// How much of the desktop shows through the window: one less the window glass's strength.
pub const GLASS: &str = "appearance.glass";

const fn choice(label: &'static str, value: &'static str) -> Choice {
    Choice {
        label: Cow::Borrowed(label),
        value: Cow::Borrowed(value),
    }
}

const BUILT_IN: &[Choice] = &[
    choice("Yi Dark", "yi"),
    choice("Graphite", "graphite"),
    choice("Ink", "ink"),
    choice("Terminal", "terminal"),
    choice("High contrast", "contrast"),
];
const DENSITIES: &[Choice] = &[
    choice("Dense", "dense"),
    choice("Compact", "compact"),
    choice("Roomy", "roomy"),
];

const fn setting(
    key: &'static str,
    name: &'static str,
    description: &'static str,
    control: Control,
    default: SettingValue,
) -> Setting {
    Setting {
        key: SettingKey::fixed(key),
        group: Cow::Borrowed("Appearance"),
        name: Cow::Borrowed(name),
        description: Cow::Borrowed(description),
        control,
        store: Store::App,
        default: Some(default),
        value: None,
    }
}

/// The look's rows in Settings (record 7.3). Each shows the theme in use; density and
/// glass are values of that theme (plan default 19), so changing one edits the theme.
const SETTINGS: &[Setting] = &[
    setting(
        THEME,
        "Theme",
        "The whole look. Edit it, or make your own, in Themes.",
        Control::Theme(Cow::Borrowed(BUILT_IN)),
        SettingValue::Text(Cow::Borrowed(files::BASE)),
    ),
    setting(
        DENSITY,
        "Density",
        "Row height and padding, everywhere at once.",
        Control::Segmented(Cow::Borrowed(DENSITIES)),
        SettingValue::Text(Cow::Borrowed("compact")),
    ),
    setting(
        GLASS,
        "See-through",
        "How much of the desktop shows through the window.",
        Control::Slider {
            min: 0.0,
            max: 1.0,
            unit: Cow::Borrowed("%"),
        },
        SettingValue::Number(0.8),
    ),
];

/// The built-in themes with `id` in use, as the app starts with them.
fn shown(hub: &mut Hub, id: &str) {
    if let Some(library) = files::library(hub) {
        *library = Library::default();
        library.pick(&ThemeId::new(id));
    }
    files::settle(hub);
}

/// A theme file edited by hand into a value that does not fit, and a file that is no theme.
fn bad_file(hub: &mut Hub) {
    let Some(library) = files::library(hub) else {
        return;
    };
    *library = Library::default();
    let sea = std::path::Path::new("themes/sea.json");
    let good = r##"{"id": "sea", "name": "Sea", "note": "yours", "dark": true,
        "values": {"accent": "#7fc4e8", "space.row": 22}}"##;
    library.take(sea, good, None);
    library.take(sea, &good.replace("#7fc4e8", "sea blue"), None);
    library.take(
        std::path::Path::new("themes/notes.json"),
        "accent = blue",
        None,
    );
    library.pick(&ThemeId::new("sea"));
    files::settle(hub);
}

/// A theme row as a thing: `yi://theme/<id>`. A value's row in the editor is no theme.
fn thing_at(hub: &Hub, _: PaneId, key: &str) -> Option<Thing> {
    let (_, id) = key
        .split_once(".row.")
        .filter(|(list, _)| !list.ends_with(".values"))?;
    let theme = hub.panes.get::<Library>(None)?.theme(&ThemeId::new(id))?;
    Some(Thing {
        label: theme.file.name.clone(),
        address: Address::new(Kind::Theme, id).ok()?,
        excerpt: Some(theme.file.note.clone()),
    })
}

fn view(pane: &PaneCx<'_>, cx: &mut Cx<Win>) -> El {
    let key = pane.key(PaneKind::Themes);
    let fallback = Library::default();
    let library = pane.hub.panes.get::<Library>(None).unwrap_or(&fallback);
    let tokens = pane.tokens();
    row()
        .key(key.clone())
        .w_full()
        .h_full()
        .fill(colour(tokens.pane_tint.alpha(tokens.glass_pane)))
        .child(list(pane, &key, library, tokens))
        .child(editor::view(pane, cx, &key, library))
}

fn list(pane: &PaneCx<'_>, key: &str, library: &Library, tokens: &Tokens) -> El {
    let id = pane.pane;
    let new = chip(tokens, Chip::Plain, "New", Some(&PLUS), false)
        .key(format!("{key}.new"))
        .on_press_tx(move |win: &mut Win, tx: &mut Tx| copy(win, tx, id));
    let count = format!("{} themes", library.themes.len());
    let head = header(
        tokens,
        vec![
            lead(tokens, &count),
            text("click one").font(LABEL).color(colour(tokens.dim)),
        ],
        vec![new],
    )
    .key(format!("{key}.pane-header"));
    let rows = library
        .themes
        .iter()
        .map(|theme| theme_row(tokens, key, id, theme, *theme.id() == library.active));
    let share = [
        "Duplicate",
        "Import a file",
        "Export",
        "Follow light and dark from the Mac",
    ]
    .into_iter()
    .enumerate()
    .map(|(n, words)| {
        let chip = chip(tokens, Chip::Plain, words, None, false).key(format!("{key}.share.{n}"));
        match n {
            0 => chip.on_press_tx(move |win: &mut Win, tx: &mut Tx| copy(win, tx, id)),
            _ => chip.help("not built yet"),
        }
    });
    col()
        .key(format!("{key}.list"))
        .w(LIST_W)
        .h_full()
        .border_sides([0.0, 1.0, 0.0, 0.0], colour(white(tokens.lines)))
        .child(head)
        .child(
            col()
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .grow(1.0)
                .min_h(0.0)
                .children(rows)
                .child(section(tokens, "Share", None, None).pt(8.0))
                .child(
                    row()
                        .w_full()
                        .min_w(0.0)
                        .flex_wrap()
                        .gap(4.0)
                        .px(8.0)
                        .pb(4.0)
                        .children(share),
                ),
        )
        .child(
            footer(
                tokens,
                vec![
                    text("A theme is one small file of values")
                        .font(META)
                        .color(colour(tokens.faint)),
                ],
                Vec::new(),
            )
            .key(format!("{key}.footer")),
        )
}

fn theme_row(tokens: &Tokens, key: &str, pane: PaneId, theme: &Theme, active: bool) -> El {
    let strip = row().clip_rounded(4.0).children(
        theme
            .swatches()
            .into_iter()
            .map(|ink| el().size(11.0, 26.0).fill(colour(ink))),
    );
    let words = col()
        .grow(1.0)
        .min_w(0.0)
        .gap(1.0)
        .child(
            text(theme.file.name.as_str())
                .font(NAME)
                .color(colour(tokens.fg))
                .truncate(),
        )
        .child(
            text(theme.file.note.as_str())
                .font(META)
                .color(colour(tokens.faint))
                .wrap(),
        );
    let line = row()
        .key(format!("{key}.row.{}", theme.id()))
        .w_full()
        .min_h(40.0)
        .px(8.0)
        .py(5.0)
        .gap(8.0)
        .center_y()
        .child(strip)
        .child(words);
    let line = if active {
        line.fill(colour(tokens.selected))
            .border_sides([0.0, 0.0, 0.0, 2.0], colour(LINE_PICKED.of(tokens)))
    } else {
        line
    };
    match theme.unavailable {
        Some(why) => line.opacity(0.6).help(why),
        None => {
            let id = theme.id().clone();
            line.cursor(Cursor::Pointer)
                .on_press_tx(move |win: &mut Win, tx: &mut Tx| pick(win, tx, pane, &id))
        }
    }
}

/// Puts `id` in use in every window.
fn pick(win: &mut Win, tx: &mut Tx, pane: PaneId, id: &ThemeId) {
    if let Ok(mut hub) = win.hub.try_borrow_mut() {
        if let Some(library) = files::library(&mut hub) {
            library.pick(id);
        }
        editor::close(&mut hub, pane);
    }
    files::apply(win, Some(tx));
}

/// New and Duplicate: a copy of the theme in use, put in use to be edited.
fn copy(win: &mut Win, tx: &mut Tx, pane: PaneId) {
    let made = win.hub.try_borrow_mut().ok().and_then(|mut hub| {
        let library = files::library(&mut hub)?;
        let active = library.active.clone();
        library.copy(&active)
    });
    if let Some(id) = made {
        pick(win, tx, pane, &id);
    }
}
