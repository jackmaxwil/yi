//! The middle column: the scope header and the settings of the chosen group, one per row.

use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use rune::{Cursor, El, GrowthSpec, ScrollSpec, TextStyle, col, el, row, text};
use yd_core::ids::{PaneId, SettingKey};
use yd_core::settings::schema::Setting;
use yd_core::tokens::Tokens;
use yd_ui::kit::bars::header;
use yd_ui::kit::chip::{Chip, chip};
use yd_ui::kit::segmented::segmented;
use yd_ui::theme::look::{BODY, CODE, LABEL, META, colour};
use yd_ui::theme::scale::{LINE_DIVIDER, SEMIBOLD};
use yd_ui::win::{Win, field};

use super::act;
use super::controls::{self, Drawn};
use super::state::Part;

pub const ROW: f32 = 64.0;

/// What every row is drawn from.
pub struct Look {
    pub pane: PaneId,
    pub key: String,
    pub tokens: Tokens,
    pub editing: Option<SettingKey>,
}

/// Who set the value and whether it differs: the note under a name.
fn note(setting: &Setting) -> &'static str {
    match (&setting.value, setting.changed()) {
        (None, _) => "default",
        (Some(_), true) => "you · changed",
        (Some(_), false) => "you",
    }
}

fn setting_row(look: &Look, win: &Win, setting: &Setting, first: bool) -> El {
    let (tokens, pane) = (&look.tokens, look.pane);
    let skey = setting.key.as_str();
    let editing = look.editing.as_ref() == Some(&setting.key);
    let field = editing
        .then(|| win.edits.get(&field(pane, Part::Row.name())))
        .flatten();
    let control = controls::control(
        &Drawn {
            pane,
            key: &format!("{}.control.{skey}", look.key),
            tokens,
            field,
        },
        setting,
    );
    let name = TextStyle {
        weight: SEMIBOLD,
        ..BODY
    };
    let code = META.family(CODE);
    let reset = setting.changed().then(|| {
        let key = setting.key.clone();
        row()
            .key(format!("{}.undo.{skey}", look.key))
            .cursor(Cursor::Pointer)
            .child(text("reset").font(code).color(colour(tokens.accent)))
            .on_press(move |win: &mut Win| act::reset(win, pane, vec![key.clone()]))
    });
    let lines = col()
        .grow(1.0)
        .min_w(0.0)
        .gap(2.0)
        .child(
            text(setting.name.to_string())
                .font(name)
                .color(colour(tokens.fg))
                .truncate(),
        )
        .child(
            text(setting.description.to_string())
                .font(LABEL)
                .color(colour(tokens.dim))
                .truncate(),
        )
        .child(
            row()
                .gap(8.0)
                .child(text(skey).font(code).color(colour(tokens.faint)))
                .child(text(note(setting)).font(code).color(colour(tokens.faint)))
                .children(reset),
        );
    let mark = setting.changed().then(|| {
        el().absolute(4.0, 9.0)
            .size(2.0, ROW - 18.0)
            .radius(1.0)
            .fill(colour(tokens.need))
    });
    row()
        .key(format!("{}.row.{skey}", look.key))
        .w_full()
        .h(ROW)
        .pl(12.0)
        .pr(8.0)
        .gap(16.0)
        .center_y()
        .border_sides(
            [if first { 0.0 } else { 1.0 }, 0.0, 0.0, 0.0],
            colour(LINE_DIVIDER.of(tokens)),
        )
        .children(mark)
        .child(lines)
        .child(control)
}

/// A number that moves when a kept row must be drawn again: what it shows, and how.
fn stamp(look: &Look, rows: &[Setting]) -> u64 {
    let mut hasher = DefaultHasher::new();
    (
        format!("{rows:?}"),
        format!("{:?}", look.tokens),
        &look.editing,
    )
        .hash(&mut hasher);
    hasher.finish()
}

/// The rows of a scroll viewport; only those in view are built.
fn list(look: Look, rows: Vec<Setting>) -> El {
    let version = stamp(&look, &rows);
    let (key, count) = (look.key.clone(), rows.len());
    let frame = Rc::new((look, rows));
    let named = Rc::clone(&frame);
    let stack = col()
        .key(format!("{key}.stack"))
        .w_full()
        .growth(GrowthSpec::default())
        .virtual_rows(
            count,
            ROW,
            move |_: &Win, at| {
                let row = named.1.get(at);
                let skey = row.map_or("", |setting| setting.key.as_str());
                Cow::Owned(format!("{}.row.{skey}", named.0.key))
            },
            move |win: &Win, at| match frame.1.get(at) {
                Some(setting) => setting_row(&frame.0, win, setting, at == 0),
                None => el(),
            },
        )
        .virtual_rows_version(version);
    col()
        .key(format!("{key}.rows"))
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(stack)
}

/// What the column says when nothing is listed or something went wrong.
fn sentence(look: &Look, words: String, bad: bool) -> El {
    let ink = if bad {
        look.tokens.bad
    } else {
        look.tokens.faint
    };
    col()
        .key(format!("{}.notice", look.key))
        .w_full()
        .min_h(look.tokens.row)
        .px(look.tokens.gutter)
        .center_y()
        .child(
            text(words)
                .font(META)
                .color(colour(ink))
                .wrap_anywhere()
                .min_w(0.0),
        )
}

/// The scope header, the rows and, under them, whatever has to be said.
pub fn view(
    look: Look,
    shown: Vec<Setting>,
    searching: Option<&str>,
    said: Option<(String, bool)>,
) -> El {
    let (tokens, pane) = (look.tokens.clone(), look.pane);
    let scope = segmented(
        &tokens,
        &format!("{}.scope", look.key),
        &["User"],
        0,
        |_: &mut Win, _| {},
    );
    let found = searching.map(|words| {
        let said = format!("{} found for “{}”", shown.len(), words.trim());
        text(said).font(LABEL).color(colour(tokens.dim))
    });
    let keys: Vec<SettingKey> = shown
        .iter()
        .filter(|s| s.changed())
        .map(|s| s.key.clone())
        .collect();
    let reset = chip(&tokens, Chip::Plain, "Reset these", None, false)
        .key(format!("{}.reset", look.key))
        .on_press(move |win: &mut Win| act::reset(win, pane, keys.clone()));
    let bar = header(
        &tokens,
        vec![scope].into_iter().chain(found).collect(),
        vec![reset],
    )
    .key(format!("{}.pane-header", look.key));
    let empty = shown.is_empty().then(|| {
        let words = match searching {
            Some(words) => format!("No setting matches “{}”", words.trim()),
            None => "No settings here".to_owned(),
        };
        sentence(&look, words, false)
    });
    let said = said.map(|(words, bad)| sentence(&look, words, bad));
    col()
        .key(format!("{}.settings-column", look.key))
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .min_h(0.0)
        .child(bar)
        .children(empty)
        .child(list(look, shown))
        .children(said)
}
