//! The header bar: the adapter in use, the program, where it stands, and the six controls.

use rune::{Cursor, El, TextStyle, Tx, row, text};
use yd_core::command::table::{Run, Target};
use yd_core::command::tools::debug;
use yd_core::ids::{CommandId, PaneId};
use yd_core::row::Tone;
use yd_core::tokens::Tokens;
use yd_ui::kit::bars::{LEAD, header};
use yd_ui::menu::command_box;
use yd_ui::pane::PaneCx;
use yd_ui::theme::look::{LABEL, NONE, colour, tone};
use yd_ui::theme::scale::{ON_OK, PRESS, RADIUS_BUTTON, RADIUS_CHIP, RADIUS_HINT, SEGMENT_ON};
use yd_ui::win::Win;

use super::session::{Control, Session, Status};

/// The debuggers the source can drive, in the order it tries them.
const ADAPTERS: [&str; 2] = ["LLDB", "GDB"];
const GLYPH: TextStyle = TextStyle::new(9.5, 400.0);

/// Glyph, words for the tooltip, the command, and the key part.
const CONTROLS: [(Control, &str, &str, CommandId, &str); 6] = [
    (
        Control::Continue,
        "▶",
        "Continue",
        debug::CONTINUE,
        "continue",
    ),
    (
        Control::StepOver,
        "↷",
        "Step over",
        debug::STEP_OVER,
        "step-over",
    ),
    (Control::StepIn, "↓", "Step in", debug::STEP_IN, "step-in"),
    (
        Control::StepOut,
        "↑",
        "Step out",
        debug::STEP_OUT,
        "step-out",
    ),
    (Control::Restart, "⟲", "Restart", debug::RESTART, "restart"),
    (Control::Stop, "■", "Stop", debug::STOP, "stop"),
];

/// The adapter the state row names is lit; the other is dim. It is a mark, not a switch.
fn adapters(tokens: &Tokens, key: &str, session: Option<&Session>) -> El {
    let used = session
        .map(|session| session.adapter.as_str())
        .unwrap_or_default();
    let item = |name: &&str| {
        let on = name.eq_ignore_ascii_case(used);
        let (fill, ink) = if on {
            (SEGMENT_ON.of(tokens), tokens.fg)
        } else {
            (NONE, tokens.faint)
        };
        row()
            .key(format!("{key}.adapter.{}", name.to_lowercase()))
            .h(17.0)
            .px(7.0)
            .center()
            .radius(RADIUS_CHIP.of(tokens))
            .fill(colour(fill))
            .child(text(*name).font(LABEL).color(colour(ink)))
    };
    row()
        .h(21.0)
        .padding(2.0)
        .center_y()
        .radius(RADIUS_BUTTON.of(tokens))
        .fill(colour(tokens.raised))
        .children(ADAPTERS.iter().map(item))
}

fn control(pane: &PaneCx<'_>, key: &str, session: Option<&Session>, at: usize) -> Option<El> {
    let (which, glyph, words, command, slug) = CONTROLS.get(at).copied()?;
    let tokens = pane.tokens();
    let (fill, ink) = match which {
        Control::Continue => (tokens.ok, ON_OK.of(tokens)),
        Control::Stop => (tokens.raised, tokens.bad),
        Control::StepOver | Control::StepIn | Control::StepOut | Control::Restart => {
            (tokens.raised, tokens.fg)
        }
    };
    let face = row()
        .key(format!("{key}.control.{slug}"))
        .w(20.0)
        .h(16.0)
        .center()
        .radius(RADIUS_HINT.of(tokens))
        .fill(colour(fill))
        .help(words)
        .label(words)
        .child(text(glyph).font(GLYPH).color(colour(ink)));
    if !session.is_some_and(|session| session.can(which)) {
        return Some(face.opacity(0.35).disabled(true));
    }
    let id: PaneId = pane.pane;
    Some(
        face.cursor(Cursor::Pointer)
            .press(colour(PRESS.of(tokens)))
            .on_press_tx(move |win: &mut Win, tx: &mut Tx| {
                command_box::run(win, tx, Run::new(command).on(Target::Pane(id)));
            }),
    )
}

pub fn view(pane: &PaneCx<'_>, key: &str, session: Option<&Session>) -> El {
    let tokens = pane.tokens();
    let program = session
        .map(|session| session.program.as_str())
        .unwrap_or_default();
    let (lead, state) = session.map(Session::headline).unwrap_or_default();
    let ink = match session.map(|session| &session.status) {
        Some(Status::Paused) => tone(tokens, Tone::Need),
        Some(Status::Error | Status::NotInstalled) => tone(tokens, Tone::Bad),
        _ => tokens.dim,
    };
    let left = vec![
        adapters(tokens, key, session),
        text(program).font(LEAD).color(colour(tokens.fg)),
        row()
            .child(text(lead).font(LABEL).color(colour(tokens.dim)))
            .child(text(state).font(LABEL).color(colour(ink))),
    ];
    let controls = (0..CONTROLS.len()).filter_map(|at| control(pane, key, session, at));
    header(tokens, left, controls.collect()).key(format!("{key}.pane-header"))
}
