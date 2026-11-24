//! The control at the right of a setting row: a toggle, a select, a segmented choice, a
//! slider, a colour or a field to type into.

use rune::{
    Cursor, El, Key, KeyEvent, PopupItem, PopupMenu, TextEdit, Tx, el, row, text, text_input,
};
use yd_core::command::table::Run;
use yd_core::ids::{PaneId, SettingKey};
use yd_core::pane::PaneKind;
use yd_core::settings::schema::{Choice, Control, Setting, SettingValue};
use yd_core::settings::store::value::{reading, shown};
use yd_core::tokens::{Rgba, Tokens};
use yd_ui::a11y;
use yd_ui::grid_view::picker;
use yd_ui::kit::chip::{Chip, chip};
use yd_ui::kit::segmented::segmented;
use yd_ui::kit::select::select;
use yd_ui::kit::slider::slider;
use yd_ui::kit::toggle::toggle;
use yd_ui::menu::command_box;
use yd_ui::theme::look::{CODE, LABEL, NONE, colour};
use yd_ui::theme::scale::LINE_SWATCH;
use yd_ui::win::Win;

use super::act;
use super::state::Part;

/// What a row's control is drawn from.
pub struct Drawn<'a> {
    pub pane: PaneId,
    pub key: &'a str,
    pub tokens: &'a Tokens,
    /// The row's own field, while this row is the one being typed into.
    pub field: Option<&'a TextEdit>,
}

fn flag(setting: &Setting) -> bool {
    matches!(setting.effective(), Some(SettingValue::Flag(true)))
}

fn text_of(setting: &Setting) -> String {
    match setting.effective() {
        Some(SettingValue::Text(text)) => text.to_string(),
        _ => String::new(),
    }
}

fn number_of(setting: &Setting) -> f64 {
    match setting.effective() {
        Some(SettingValue::Number(n)) => *n,
        _ => 0.0,
    }
}

fn pick(choices: &[Choice]) -> Vec<(String, String)> {
    choices
        .iter()
        .map(|c| (c.value.to_string(), c.label.to_string()))
        .collect()
}

/// A select over `choices`: its menu is a native pop-up under the control.
fn menu(d: &Drawn<'_>, setting: &Setting, choices: &[Choice]) -> El {
    let (pane, anchor, key) = (d.pane, d.key.to_owned(), setting.key.clone());
    let (items, now) = (pick(choices), text_of(setting));
    select(d.tokens, &shown(setting))
        .key(anchor.clone())
        .on_press_tx(move |_: &mut Win, tx: &mut Tx| {
            let menu = PopupMenu::new().items(
                items
                    .iter()
                    .map(|(value, label)| PopupItem::new(value, label).checked(*value == now)),
            );
            let key = key.clone();
            tx.popup_menu(
                menu,
                anchor.as_str(),
                move |win: &mut Win, picked: Option<&str>, _: &mut Tx| {
                    if let Some(value) = picked {
                        act::set(
                            win,
                            pane,
                            &key,
                            Some(SettingValue::Text(value.to_owned().into())),
                        );
                    }
                },
            );
        })
}

fn segments(d: &Drawn<'_>, setting: &Setting, choices: &[Choice]) -> El {
    let (pane, key) = (d.pane, setting.key.clone());
    let items = pick(choices);
    let labels: Vec<&str> = items.iter().map(|(_, label)| label.as_str()).collect();
    let at = items
        .iter()
        .position(|(value, _)| *value == text_of(setting))
        .unwrap_or(usize::MAX);
    segmented(d.tokens, d.key, &labels, at, {
        let items = items.clone();
        move |win: &mut Win, index: usize| {
            if let Some((value, _)) = items.get(index) {
                act::set(
                    win,
                    pane,
                    &key,
                    Some(SettingValue::Text(value.clone().into())),
                );
            }
        }
    })
}

fn slide(d: &Drawn<'_>, setting: &Setting, control: &Control) -> El {
    let Control::Slider { min, max, .. } = control else {
        return el();
    };
    let (pane, key, now) = (d.pane, setting.key.clone(), number_of(setting));
    let (min, max) = (*min, *max);
    let share = if max > min {
        (now - min) / (max - min)
    } else {
        0.0
    };
    // Hundredths: a drag sets a setting only when it crosses one, not on every pixel.
    slider(
        d.tokens,
        d.key,
        share as f32,
        &reading(control, now),
        move |win: &mut Win, to: f32| {
            let value = ((min + f64::from(to) * (max - min)) * 100.0).round() / 100.0;
            if (value - now).abs() > f64::EPSILON {
                act::set(win, pane, &key, Some(SettingValue::Number(value)));
            }
        },
    )
}

fn swatch(tokens: &Tokens, hex: &str) -> Option<El> {
    let ink: Rgba = Rgba::parse(hex)?;
    Some(
        el().size(12.0, 12.0)
            .radius(3.0)
            .fill(colour(ink))
            .border(1.0, colour(LINE_SWATCH.of(tokens)))
            .key(format!("{hex}.swatch")),
    )
}

/// The value as a chip of mono text that opens the row's own field when pressed.
fn typed(d: &Drawn<'_>, setting: &Setting, control: &Control) -> El {
    let (pane, tokens) = (d.pane, d.tokens);
    if let Some(edit) = d.field {
        let key = setting.key.clone();
        return text_input(edit, LABEL.family(CODE))
            .single_line()
            .key(d.key.to_owned())
            .w(150.0)
            .h(20.0)
            .px(6.0)
            .radius(5.0)
            .fill(colour(tokens.raised))
            .color(colour(tokens.fg))
            .caret(colour(tokens.accent), a11y::selection(tokens))
            .autofocus()
            .on_edit(move |win: &mut Win| act::field(win, pane, Part::Row))
            .on_submit(move |win: &mut Win, _: &mut Tx| act::commit(win, pane, &key))
            .on_input_key(move |win: &mut Win, event: &KeyEvent, _: &mut Tx| {
                let leave = event.key == Key::Escape;
                if leave {
                    act::cancel(win, pane);
                }
                leave
            });
    }
    let value = text(shown(setting))
        .font(LABEL.family(CODE))
        .color(colour(tokens.dim));
    let swatch = matches!(control, Control::Colour)
        .then(|| swatch(tokens, &text_of(setting)))
        .flatten();
    let editable = !matches!(control, Control::Other(_));
    let chip = row()
        .key(d.key.to_owned())
        .h(20.0)
        .px(6.0)
        .gap(6.0)
        .center_y()
        .radius(5.0)
        .fill(colour(if editable { tokens.raised } else { NONE }))
        .children(swatch)
        .child(value);
    if !editable {
        return chip;
    }
    let setting = setting.clone();
    chip.cursor(Cursor::Pointer)
        .on_press(move |win: &mut Win| act::begin(win, pane, &setting))
}

/// The control for `setting`, keyed `d.key`.
pub fn control(d: &Drawn<'_>, setting: &Setting) -> El {
    named(unnamed(d, setting), setting)
}

/// A control says which setting it sets, whatever words it shows.
fn named(control: El, setting: &Setting) -> El {
    control.label(setting.name.to_string())
}

fn unnamed(d: &Drawn<'_>, setting: &Setting) -> El {
    let (pane, key): (PaneId, SettingKey) = (d.pane, setting.key.clone());
    match &setting.control {
        Control::Toggle => {
            let on = flag(setting);
            toggle(d.tokens, d.key, on).on_press(move |win: &mut Win| {
                act::set(win, pane, &key, Some(SettingValue::Flag(!on)))
            })
        }
        Control::Select(choices) => menu(d, setting, choices),
        Control::Theme(choices) => row()
            .gap(6.0)
            .center_y()
            .child(menu(d, setting, choices))
            .child(
                chip(d.tokens, Chip::Plain, "Edit", None, false)
                    .key(format!("{}.edit", d.key))
                    .on_press_tx(|win: &mut Win, tx: &mut Tx| {
                        if let Some(show) = picker::show(PaneKind::Themes) {
                            command_box::run(win, tx, Run::new(show));
                        }
                    }),
            ),
        Control::Segmented(choices) => segments(d, setting, choices),
        control @ Control::Slider { .. } => slide(d, setting, control),
        control => typed(d, setting, control),
    }
}
