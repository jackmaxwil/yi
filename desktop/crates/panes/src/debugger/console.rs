//! The adapter's console: its newest lines, following the tail, and the line that runs `debug.eval`.

use rune::{Color, El, ScrollSpec, TextEdit, TextStyle, Tx, col, row, text, text_input};
use yd_core::command::table::{Run, Target};
use yd_core::feed::Table;
use yd_core::ids::{CommandId, PaneId};
use yd_core::row::Rows;
use yd_ui::a11y;
use yd_ui::menu::command_box;
use yd_ui::pane::PaneCx;
use yd_ui::theme::look::{CODE_LINE, colour, white};
use yd_ui::theme::scale::CONSOLE;
use yd_ui::win::{Win, field as field_of};

/// The console's input line, as the window's text fields name it.
pub const EVAL: &str = "eval";

/// The most console lines drawn; older ones stay in the feed and a line above says how many.
pub const SHOWN: usize = 200;
pub const HEIGHT: f32 = 92.0;
const PROMPT: TextStyle = TextStyle {
    weight: 500.0,
    ..CODE_LINE
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Input,
    Output,
    Error,
}

struct Line<'a> {
    id: &'a str,
    role: Role,
    text: &'a str,
}

/// One of the pane's text fields as the window holds it.
pub fn field<'a>(win: &'a mut Win, pane: PaneId, name: &'static str) -> &'a mut TextEdit {
    win.edits.entry(field_of(pane, name)).or_default()
}

/// Runs `command` with the words typed in field `name`, then empties the field.
pub fn submit(win: &mut Win, tx: &mut Tx, pane: PaneId, name: &'static str, command: CommandId) {
    let words = field(win, pane, name).text.trim().to_owned();
    if words.is_empty() {
        return;
    }
    field(win, pane, name).clear();
    let run = Run {
        text: Some(words),
        ..Run::new(command).on(Target::Pane(pane))
    };
    command_box::run(win, tx, run);
}

/// A single-line field of the window, drawn from the edit the window holds.
pub fn input(
    pane: &PaneCx<'_>,
    key: &str,
    (name, command): (&'static str, CommandId),
    (label, hint): (&str, &str),
) -> El {
    let blank = TextEdit::new("");
    let edit = pane
        .edits
        .and_then(|edits| edits.get(&field_of(pane.pane, name)));
    let tokens = pane.tokens();
    let id = pane.pane;
    text_input(edit.unwrap_or(&blank), CODE_LINE)
        .key(key)
        .single_line()
        .label(label)
        .placeholder(hint, colour(tokens.faint))
        .grow(1.0)
        .min_w(0.0)
        .color(colour(tokens.fg))
        .caret(colour(tokens.fg), a11y::selection(tokens))
        .on_edit(move |win: &mut Win| field(win, id, name))
        .on_submit(move |win: &mut Win, tx: &mut Tx| submit(win, tx, id, name, command))
}

fn lines(table: &Table) -> Vec<Line<'_>> {
    let rows = (0..table.len()).filter_map(|at| table.get(at));
    rows.filter(|row| row.id.as_str().starts_with("out:"))
        .map(|row| Line {
            id: row.id.as_str(),
            role: match row.words("role") {
                Some("input") => Role::Input,
                Some("error") => Role::Error,
                Some(_) | None => Role::Output,
            },
            text: row.words("text").unwrap_or_default(),
        })
        .collect()
}

fn drawn(pane: &PaneCx<'_>, key: &str, shown: &Line<'_>, adapter: &str) -> El {
    let tokens = pane.tokens();
    let (dim, bad): (Color, Color) = (colour(tokens.dim), colour(tokens.bad));
    let words =
        |said: &str, ink: Color| text(said.to_owned()).font(CODE_LINE).color(ink).truncate();
    let line = row()
        .w_full()
        .h(16.0)
        .gap(0.0)
        .key(format!("{key}.out.{}", shown.id));
    match shown.role {
        Role::Input => line
            .child(
                text(prompt(adapter))
                    .font(PROMPT)
                    .color(colour(tokens.accent)),
            )
            .child(words(&format!(" {}", shown.text), dim)),
        Role::Output => line.child(words(shown.text, dim)),
        Role::Error => line.child(words(shown.text, bad)),
    }
}

/// `(lldb)` for the adapter the state row names.
fn prompt(adapter: &str) -> String {
    match adapter.trim() {
        "" => "(debug)".to_owned(),
        name => format!("({})", name.to_lowercase()),
    }
}

/// The console: the adapter's words, then the line to type on, at the foot of the left column.
pub fn view(pane: &PaneCx<'_>, key: &str, adapter: &str) -> El {
    let tokens = pane.tokens();
    let table = pane.model().tables.get(yd_core::feed::DEBUGGER);
    let all = table.map(lines).unwrap_or_default();
    let left_out = all.len().saturating_sub(SHOWN);
    let key = format!("{key}.console");
    let notice = (left_out > 0).then(|| {
        let said = format!("{left_out} earlier lines are not shown (showing the last {SHOWN})");
        text(said)
            .font(CODE_LINE)
            .color(colour(tokens.faint))
            .truncate()
    });
    let typing = row()
        .key(format!("{key}.prompt"))
        .w_full()
        .h(16.0)
        .gap(6.0)
        .child(
            text(prompt(adapter))
                .font(PROMPT)
                .color(colour(tokens.accent)),
        )
        .child(input(
            pane,
            &format!("{key}.input"),
            (EVAL, yd_core::command::tools::debug::EVAL),
            ("Debugger console", ""),
        ));
    let lines = col()
        .key(format!("{key}.lines"))
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: true })
        .children(notice)
        .children(
            all.iter()
                .skip(left_out)
                .map(|line| drawn(pane, &key, line, adapter)),
        )
        .child(typing);
    col()
        .key(key)
        .w_full()
        .h(HEIGHT)
        .px(8.0)
        .py(4.0)
        .fill(colour(CONSOLE.of(tokens)))
        .border_sides([1.0, 0.0, 0.0, 0.0], colour(white(tokens.inner_lines)))
        .clip_rounded(0.0)
        .child(lines)
}
