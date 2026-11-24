//! The theme editor: every token, density and glass.

use rune::{
    Cursor, Cx, El, KeyEvent, ScrollSpec, TextEdit, TextStyle, Tx, col, el, row, text, text_input,
};
use yd_core::ids::{PaneId, ThemeId};
use yd_core::tokens::{ThemeValue, Tokens};
use yd_ui::a11y;
use yd_ui::kit::bars::{footer, header, lead};
use yd_ui::kit::label::section;
use yd_ui::kit::segmented::segmented;
use yd_ui::kit::select::select;
use yd_ui::kit::slider::slider;
use yd_ui::kit::toggle::toggle;
use yd_ui::pane::PaneCx;
use yd_ui::theme::files::{self, Library};
use yd_ui::theme::look::{CODE, LABEL, META, colour};
use yd_ui::theme::scale::{INK_GROUPS, Part, RADIUS_BADGE, SEMIBOLD, inks};
use yd_ui::win::{Hub, Win};

/// How one value is shown and changed.
#[derive(Debug, Clone, Copy)]
enum Set {
    Colour(&'static str),
    Strength(&'static str),
    Font(&'static str),
    Flag(&'static str),
    /// Sizes in a row, each under its own word.
    Numbers(&'static [(&'static str, &'static str)]),
}

/// A row of the editor: the role, the value and what it is used for.
type Line = (&'static str, Set, &'static str);

const GROUPS: &[(&str, &[Line])] = &[
    (
        "Colour roles",
        &[
            (
                "Working",
                Set::Colour("accent"),
                "agents at work, context boxes",
            ),
            (
                "Needs you",
                Set::Colour("needs_you"),
                "dots, approval card, counts",
            ),
            (
                "Running, passed",
                Set::Colour("ok"),
                "services, checks, lines added",
            ),
            (
                "Failed, missing",
                Set::Colour("bad"),
                "errors, gaps, lines removed",
            ),
            ("Warning", Set::Colour("warn"), "slow, retried, soft limits"),
            ("Model", Set::Colour("model"), "the model's own spans"),
            ("Context", Set::Colour("context"), "what is attached"),
            ("Idle", Set::Colour("dot.idle"), "waiting, nothing to do"),
            ("Stopped", Set::Colour("dot.stopped"), "stopped, put away"),
            ("Meter", Set::Colour("meter"), "meters and sparklines"),
        ],
    ),
    (
        "Text",
        &[
            ("Bright", Set::Colour("text.1"), "the thing itself"),
            ("Dim", Set::Colour("text.2"), "its value"),
            ("Faint", Set::Colour("text.3"), "everything about it"),
        ],
    ),
    (
        "Surfaces",
        &[
            (
                "Window glass",
                Set::Strength("glass.window"),
                "see-through, blur 18",
            ),
            ("Pane", Set::Strength("glass.pane"), "every pane"),
            (
                "Pane in use",
                Set::Strength("glass.pane_in_use"),
                "the one you are in",
            ),
            ("Lines", Set::Strength("lines"), "dividers between panes"),
            (
                "Inner lines",
                Set::Strength("lines.inner"),
                "dividers inside a pane",
            ),
            ("Glass", Set::Flag("glass"), "off: one solid window"),
            ("Window", Set::Colour("window"), "under the glass"),
            (
                "Window tint",
                Set::Colour("glass.window.tint"),
                "the glass's own colour",
            ),
            (
                "Pane tint",
                Set::Colour("glass.pane.tint"),
                "each pane's own colour",
            ),
            ("Selected", Set::Colour("fill.selected"), "the open row"),
            ("Raised", Set::Colour("fill.raised"), "chips and controls"),
            ("Panel", Set::Colour("fill.panel"), "buttons and cards"),
            (
                "Control edge",
                Set::Colour("line.control"),
                "edges of controls",
            ),
            ("Added", Set::Colour("diff.added"), "lines added in a diff"),
            (
                "Removed",
                Set::Colour("diff.removed"),
                "lines removed in a diff",
            ),
        ],
    ),
    (
        "Code",
        &[
            ("Keyword", Set::Colour("code.keyword"), "pub fn match"),
            ("Type", Set::Colour("code.type"), "Bucket Instant"),
            ("String", Set::Colour("code.string"), "\"/upload\""),
            ("Number", Set::Colour("code.number"), "20 60"),
            ("Comment", Set::Colour("code.comment"), "// why it is so"),
        ],
    ),
    (
        "Type and space",
        &[
            ("Text font", Set::Font("font.text"), "chat, labels"),
            ("Code font", Set::Font("font.code"), "code, logs"),
            (
                "Sizes",
                Set::Numbers(&[
                    ("", "size.title"),
                    ("", "size.body"),
                    ("", "size.label"),
                    ("", "size.meta"),
                ]),
                "",
            ),
            (
                "Space",
                Set::Numbers(&[
                    ("gutter", "space.gutter"),
                    ("row", "space.row"),
                    ("line", "space.line"),
                    ("bar", "space.bar"),
                ]),
                "",
            ),
        ],
    ),
];

/// Row, line and gutter of each density (plan default 19 for Roomy).
const DENSITY: [(&str, [f32; 3]); 3] = [
    ("Dense", [18.0, 15.0, 6.0]),
    ("Compact", [20.0, 16.0, 8.0]),
    ("Roomy", [24.0, 18.0, 10.0]),
];
const DENSITY_NAMES: [&str; 3] = ["space.row", "space.line", "space.gutter"];
/// Pane, pane in use and window glass of Thin and Solid (record 7.4).
const SURFACE: [(&str, [f32; 3]); 2] = [("Thin", [0.2, 0.3, 0.2]), ("Solid", [0.7, 0.82, 0.75])];
const SURFACE_NAMES: [&str; 3] = ["glass.pane", "glass.pane_in_use", "glass.window"];

const ROLE: TextStyle = TextStyle {
    weight: SEMIBOLD,
    ..LABEL
};
const VALUE: TextStyle = LABEL.family(CODE);
/// The one field the editor types a value into, in the window that edits it.
const VALUE_FIELD: &str = "value";

/// The value being typed, and what the last refused edit said.
#[derive(Debug, Default)]
pub struct Editing {
    name: Option<&'static str>,
    said: Option<String>,
}

/// Each group's own rows, then the named colours shown under it; groups of named colours alone
/// come last.
fn groups() -> Vec<(&'static str, Vec<Line>)> {
    let part = |(ink, role, used): &Part| (*role, Set::Colour(ink.0), *used);
    let named = |group: &'static str| {
        let parts = INK_GROUPS.iter().filter(move |(own, _)| *own == group);
        parts.flat_map(|(_, parts)| parts.iter().map(part))
    };
    let own = GROUPS.iter().map(|(group, lines)| {
        let lines = lines.iter().copied().chain(named(group));
        (*group, lines.collect())
    });
    let alone = INK_GROUPS
        .iter()
        .filter(|(group, _)| !GROUPS.iter().any(|(own, _)| own == group))
        .map(|(group, parts)| (*group, parts.iter().map(part).collect()));
    own.chain(alone).collect()
}

/// Every name the editor shows, once each.
pub fn names() -> Vec<&'static str> {
    groups()
        .iter()
        .flat_map(|(_, lines)| lines.iter())
        .flat_map(|(_, set, _)| match set {
            Set::Colour(name) | Set::Strength(name) | Set::Font(name) | Set::Flag(name) => {
                vec![*name]
            }
            Set::Numbers(parts) => parts.iter().map(|(_, name)| *name).collect(),
        })
        .collect()
}

/// Shuts the field without taking what was typed.
pub fn close(hub: &mut Hub, pane: PaneId) {
    if let Some(editing) = hub.panes.get_mut::<Editing>(Some(pane)) {
        *editing = Editing::default();
    }
}

/// A token name as a key part: a dot would read as a key separator.
pub fn slug(name: &str) -> String {
    name.replace('.', "-")
}

/// The value as the theme file and the field write it.
fn written(tokens: &Tokens, name: &str) -> Option<ThemeValue> {
    let table = tokens.values().into_iter().find(|(n, _)| *n == name);
    let named = || {
        inks()
            .find(|ink| ink.0 == name)
            .map(|ink| ink.of(tokens).text())
    };
    table
        .map(|(_, value)| value)
        .or_else(|| named().map(ThemeValue::Text))
}

fn words(value: &ThemeValue) -> String {
    match value {
        ThemeValue::Text(text) if text == "system" => "System".into(),
        ThemeValue::Text(text) => text.clone(),
        ThemeValue::Number(n) => (*n as f32).to_string(),
        ThemeValue::Flag(on) => on.to_string(),
    }
}

fn number(tokens: &Tokens, name: &str) -> f32 {
    match written(tokens, name) {
        Some(ThemeValue::Number(n)) => n as f32,
        _ => 0.0,
    }
}

/// Which preset the three values match, if any.
fn preset(tokens: &Tokens, names: [&str; 3], presets: &[(&str, [f32; 3])]) -> usize {
    presets
        .iter()
        .position(|(_, values)| {
            names
                .iter()
                .zip(values)
                .all(|(name, v)| (number(tokens, name) - v).abs() < 0.005)
        })
        .unwrap_or(usize::MAX)
}

pub fn view(pane: &PaneCx<'_>, _cx: &mut Cx<Win>, key: &str, library: &Library) -> El {
    let id = pane.pane;
    let tokens = pane.tokens();
    let Some(theme) = library.theme(&library.active) else {
        return col().grow(1.0);
    };
    let theme_tokens = theme.tokens();
    let editing = pane.state::<Editing>();
    let open = editing.and_then(|e| e.name);
    let names = names();
    let changed = Tokens::default()
        .values()
        .iter()
        .zip(theme_tokens.values())
        .filter(|(a, b)| **a != *b)
        .count();
    let density = segmented(
        tokens,
        &format!("{key}.density"),
        &DENSITY.map(|(words, _)| words),
        preset(&theme_tokens, DENSITY_NAMES, &DENSITY),
        move |win: &mut Win, at: usize| {
            preset_set(win, id, DENSITY_NAMES, DENSITY.get(at).map(|p| p.1))
        },
    );
    let surface = segmented(
        tokens,
        &format!("{key}.surface"),
        &SURFACE.map(|(words, _)| words),
        preset(&theme_tokens, SURFACE_NAMES, &SURFACE),
        move |win: &mut Win, at: usize| {
            preset_set(win, id, SURFACE_NAMES, SURFACE.get(at).map(|p| p.1))
        },
    );
    let head = header(
        tokens,
        vec![
            lead(tokens, &theme.file.name),
            text(format!("{} values · every one can be changed", names.len()))
                .font(LABEL)
                .color(colour(tokens.dim)),
        ],
        vec![density, surface],
    )
    .key(format!("{key}.editor.pane-header"));
    let said = library
        .refusals
        .iter()
        .map(|refusal| refusal.words())
        .chain(editing.and_then(|e| e.said.clone()))
        .map(|words| {
            text(words)
                .font(META)
                .color(colour(tokens.bad))
                .wrap()
                .px(tokens.gutter)
        });
    let field = Field { pane, key, open };
    let groups = groups();
    let body =
        groups.iter().flat_map(|(group, lines)| {
            std::iter::once(section(tokens, group, None, None)).chain(lines.iter().map(
                |(role, set, used)| value_row(tokens, &theme_tokens, &field, (role, *set, used)),
            ))
        });
    col()
        .key(format!("{key}.editor"))
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .child(head)
        .child(
            col()
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .grow(1.0)
                .min_h(0.0)
                .pt(4.0)
                .children(said)
                .children(body),
        )
        .child(
            footer(
                tokens,
                vec![
                    text("Changes show at once, in every window")
                        .font(META)
                        .color(colour(tokens.faint)),
                ],
                vec![
                    text(format!("{changed} changed from the base"))
                        .font(META)
                        .color(colour(tokens.faint)),
                ],
            )
            .key(format!("{key}.editor.footer")),
        )
}

/// What a value cell needs to open its field.
struct Field<'a, 'b> {
    pane: &'a PaneCx<'b>,
    key: &'a str,
    open: Option<&'static str>,
}

impl Field<'_, '_> {
    /// The value as words a press opens for typing, or the field when it is open.
    fn cell(&self, tokens: &Tokens, name: &'static str, shown: El, now: String) -> El {
        let pane = self.pane.pane;
        if self.open == Some(name) {
            let blank = TextEdit::new("");
            let edit = self
                .pane
                .edits
                .and_then(|edits| edits.get(&yd_ui::win::field(pane, VALUE_FIELD)))
                .unwrap_or(&blank);
            return text_input(edit, VALUE)
                .key(format!("{}.field", self.key))
                .w(124.0)
                .color(colour(tokens.fg))
                .caret(colour(tokens.accent), a11y::selection(tokens))
                .on_edit(move |win: &mut Win| {
                    win.edits
                        .entry(yd_ui::win::field(pane, VALUE_FIELD))
                        .or_default()
                })
                .on_submit(move |win: &mut Win, tx: &mut Tx| submit(win, tx, pane, name))
                .on_input_key(move |win: &mut Win, event: &KeyEvent, _tx: &mut Tx| {
                    let escape = matches!(event.key, rune::Key::Escape);
                    if escape && let Ok(mut hub) = win.hub.try_borrow_mut() {
                        close(&mut hub, pane);
                    }
                    escape
                });
        }
        let field = format!("{}.field", self.key);
        shown
            .key(format!("{}.value.{}.set", self.key, slug(name)))
            .cursor(Cursor::Pointer)
            .on_press_tx(move |win: &mut Win, tx: &mut Tx| {
                win.edits.insert(
                    yd_ui::win::field(pane, VALUE_FIELD),
                    TextEdit::new(now.clone()),
                );
                if let Ok(mut hub) = win.hub.try_borrow_mut()
                    && let Some(editing) = hub.panes.get_mut::<Editing>(Some(pane))
                {
                    *editing = Editing {
                        name: Some(name),
                        said: None,
                    };
                }
                tx.focus(field.clone(), false);
            })
    }
}

fn value_row(
    tokens: &Tokens,
    theme: &Tokens,
    field: &Field<'_, '_>,
    (role, set, used): (&str, Set, &str),
) -> El {
    let pane = field.pane.pane;
    let now = |name: &str| written(theme, name).as_ref().map(words).unwrap_or_default();
    let (first, value) = match set {
        Set::Colour(name) => {
            let ink = written(theme, name).and_then(|v| match v {
                ThemeValue::Text(t) => yd_core::tokens::Rgba::parse(&t),
                _ => None,
            });
            let shown = row()
                .gap(6.0)
                .center_y()
                .child(
                    el().size(12.0, 12.0)
                        .radius(RADIUS_BADGE.of(tokens))
                        .fill(colour(ink.unwrap_or(tokens.fg))),
                )
                .child(text(name).font(VALUE).color(colour(tokens.dim)));
            (name, field.cell(tokens, name, shown, now(name)))
        }
        Set::Strength(name) => {
            let share = number(theme, name);
            let reading = format!("{:.0}%", share * 100.0);
            let key = format!("{}.value.{}.set", field.key, slug(name));
            let drag = move |win: &mut Win, to: f32| {
                let to = (f64::from(to) * 100.0).round() / 100.0;
                change(win, None, pane, name, ThemeValue::Number(to));
            };
            (
                name,
                slider(tokens, &key, share, &reading, drag).label(name),
            )
        }
        Set::Font(name) => (
            name,
            field.cell(tokens, name, select(tokens, &now(name)), now(name)),
        ),
        Set::Flag(name) => {
            let on = matches!(written(theme, name), Some(ThemeValue::Flag(true)));
            let key = format!("{}.value.{}.set", field.key, slug(name));
            let flip = toggle(tokens, &key, on).label(name).on_press_tx(
                move |win: &mut Win, tx: &mut Tx| {
                    change(win, Some(tx), pane, name, ThemeValue::Flag(!on));
                },
            );
            (name, flip)
        }
        Set::Numbers(parts) => {
            let pieces = parts.iter().enumerate().flat_map(|(n, (word, name))| {
                let dot = (n > 0).then(|| text("·").font(VALUE).color(colour(tokens.faint)));
                let word =
                    (!word.is_empty()).then(|| text(*word).font(VALUE).color(colour(tokens.dim)));
                let shown = text(now(name)).font(VALUE).color(colour(tokens.fg));
                [dot, word, Some(field.cell(tokens, name, shown, now(name)))]
            });
            let first = parts.first().map_or("", |(_, name)| *name);
            (first, row().gap(5.0).center_y().children(pieces.flatten()))
        }
    };
    let wide = matches!(set, Set::Numbers(_));
    row()
        .key(format!("{}.values.row.{}", field.key, slug(first)))
        .w_full()
        .h(tokens.row)
        .px(tokens.gutter)
        .gap(8.0)
        .center_y()
        .child(text(role).font(ROLE).color(colour(tokens.fg)).w(112.0))
        .child(if wide {
            value.grow(1.0)
        } else {
            row().w(124.0).center_y().child(value)
        })
        .children((!wide).then(|| {
            text(used)
                .font(META)
                .color(colour(tokens.faint))
                .truncate()
                .grow(1.0)
                .min_w(0.0)
        }))
}

/// Takes what was typed into the open field: refused by name when it does not fit.
fn submit(win: &mut Win, tx: &mut Tx, pane: PaneId, name: &'static str) {
    let typed = win
        .edits
        .get(&yd_ui::win::field(pane, VALUE_FIELD))
        .map(|edit| edit.text.trim().to_owned())
        .unwrap_or_default();
    let value = match typed.parse::<f64>() {
        Ok(n) if typed.chars().all(|c| c.is_ascii_digit() || c == '.') => ThemeValue::Number(n),
        _ if typed.eq_ignore_ascii_case("system") => ThemeValue::Text("system".into()),
        _ => ThemeValue::Text(typed),
    };
    change(win, Some(tx), pane, name, value);
}

/// Sets one value of the theme in use and shows it everywhere.
fn change(win: &mut Win, tx: Option<&mut Tx>, pane: PaneId, name: &'static str, value: ThemeValue) {
    if let Ok(mut hub) = win.hub.try_borrow_mut() {
        let done = files::library(&mut hub).map(|library| {
            let active: ThemeId = library.active.clone();
            library.set(&active, name, value)
        });
        let said = match done {
            Some(Err(why)) => Some(format!("{name}: {why}; kept as it was")),
            Some(Ok(_)) | None => None,
        };
        if let Some(editing) = hub.panes.get_mut::<Editing>(Some(pane)) {
            *editing = Editing { name: None, said };
        }
    }
    files::apply(win, tx);
}

/// The density the tokens draw at, as its setting writes it: `dense`, `compact`, `roomy`.
pub fn density(tokens: &Tokens) -> Option<String> {
    // The poll asks this every frame: read the three sizes, not every token by name.
    let now = [tokens.row, tokens.line, tokens.gutter];
    let same = |sizes: &[f32; 3]| now.iter().zip(sizes).all(|(a, b)| (a - b).abs() < 0.005);
    let found = DENSITY.iter().find(|(_, sizes)| same(sizes));
    found.map(|(name, _)| name.to_lowercase())
}

/// Sets values of the theme in use from outside the editor, as a setting row does. A
/// built-in theme is not changed: the values go to a copy of it, which is put in use.
pub fn put(win: &mut Win, values: &[(&'static str, f64)]) {
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(library) = files::library(&mut hub)
    {
        for (name, value) in values {
            let active = library.active.clone();
            // Every name here is a number token of the table, so the set is taken.
            let _ = library.set(&active, name, ThemeValue::Number(*value));
        }
    }
    files::apply(win, None);
}

/// Puts the three sizes of the density named `name` on the theme in use.
pub fn set_density(win: &mut Win, name: &str) {
    let found = DENSITY
        .iter()
        .find(|(each, _)| each.eq_ignore_ascii_case(name));
    if let Some((_, sizes)) = found {
        let sized = DENSITY_NAMES.into_iter().zip(sizes.map(f64::from));
        put(win, &sized.collect::<Vec<_>>());
    }
}

/// Dense, Compact or Roomy; Thin or Solid: three values at once.
fn preset_set(win: &mut Win, pane: PaneId, names: [&'static str; 3], values: Option<[f32; 3]>) {
    for (name, value) in names.into_iter().zip(values.into_iter().flatten()) {
        change(win, None, pane, name, ThemeValue::Number(f64::from(value)));
    }
}
