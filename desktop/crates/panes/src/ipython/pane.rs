//! IPython pane: the shown thread's kernel cells, each its input and its outputs, newest last.

mod cell;
mod scenes;

use std::collections::BTreeSet;

use rune::{Cursor, Cx, El, ScrollSpec, Tx, col, el, row, text};
use yd_core::ids::PaneId;
use yd_core::pane::PaneKind;
use yd_core::reduce::cells::TOOL;
use yd_core::reduce::threads::Thread;
use yd_core::reduce::turns::{CallId, ToolCall};
use yd_core::tokens::Tokens;
use yd_ui::code::view::Look;
use yd_ui::grid_view::view::show_pane;
use yd_ui::kit::bars::{header, lead};
use yd_ui::pane::{PaneCx, PaneDef, Scene, Stage};
use yd_ui::states::status::{self, Status};
use yd_ui::theme::look::{LABEL, META, colour};
use yd_ui::win::Win;

use super::states::WORDS;
use crate::copy::empty::{down, threadless};

pub const DEF: PaneDef = PaneDef {
    kind: PaneKind::IPython,
    view,
    scenes: &[
        Scene {
            name: "ipython/cells",
            stage: Stage::Pane(PaneKind::IPython),
            setup: scenes::cells,
        },
        Scene {
            name: "ipython/running",
            stage: Stage::Pane(PaneKind::IPython),
            setup: scenes::running,
        },
        Scene {
            name: "ipython/picture",
            stage: Stage::Pane(PaneKind::IPython),
            setup: scenes::picture,
        },
    ],
    commands: &[],
    settings: &[],
    tabs: None,
    on_tab: None,
    finder: None,
};

/// Cells drawn, the newest; the older ones are named in a notice where they are left out.
pub const MAX_CELLS: usize = 200;

/// What the pane keeps: the cell a link asked it to show, and the pictures shown full size.
#[derive(Debug, Default)]
pub struct Notebook {
    aim: Option<CallId>,
    full: BTreeSet<String>,
}

/// Under an `ipython` step of the thread's own conversation: the way to that cell here. A
/// child's step is not one: its cells ran in the child's own kernel.
pub fn link(key: &str, tokens: &Tokens, thread: &Thread, call: &str) -> Option<El> {
    let found = thread
        .transcript
        .tools
        .get(call)
        .filter(|c| c.name == TOOL)?;
    let id = found.id.clone();
    let words = "Open in IPython";
    let link = text(words)
        .key(format!("{key}.ipython.{call}"))
        .font(META)
        .color(colour(tokens.accent))
        .cursor(Cursor::Pointer)
        .label(words)
        .on_press_tx(move |win: &mut Win, tx: &mut Tx| open(win, tx, &id));
    Some(row().h(16.0).pl(19.0).center_y().child(link))
}

/// Shows the window's IPython pane, opening one when it has none, scrolled to cell `call`.
pub fn open(win: &mut Win, tx: &mut Tx, call: &CallId) {
    let Some(pane) = show_pane(win, tx, PaneKind::IPython) else {
        return;
    };
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(book) = hub.panes.get_mut::<Notebook>(Some(pane))
    {
        book.aim = Some(call.clone());
    }
}

/// A zero-size element whose first layout centres the aimed cell's input, then lets go.
fn aimed(key: &str, pane: PaneId, call: &CallId) -> El {
    let scope = format!("{key}.cells");
    let input = format!("{key}.cell.{}.input", call.as_str());
    el().key(format!("{key}.aim.{}", call.as_str()))
        .absolute(0.0, 0.0)
        .size(0.0, 0.0)
        .on_layout_tx(move |win: &mut Win, _: [f32; 4], tx: &mut Tx| {
            tx.scroll_center_in(scope.clone(), input.clone());
            if let Ok(mut hub) = win.hub.try_borrow_mut()
                && let Some(book) = hub.panes.get_mut::<Notebook>(Some(pane))
            {
                book.aim = None;
            }
        })
}

fn view(pane: &PaneCx<'_>, _cx: &mut Cx<Win>) -> El {
    let key = pane.key(PaneKind::IPython);
    let tokens = pane.tokens();
    let tint = if pane.focused {
        tokens.glass_pane_focus
    } else {
        tokens.glass_pane
    };
    let body = match pane.thread() {
        Some(thread) => notebook(pane, &key, thread),
        None => {
            let (shown, words) = threadless(pane).unwrap_or((Status::Empty, &WORDS));
            status::view(pane, &key, &shown, words)
        }
    };
    col()
        .key(key)
        .w_full()
        .h_full()
        .fill(colour(tokens.pane_tint.alpha(tint)))
        .child(body)
}

/// The header naming the thread and its cells, then the cells, following the newest.
fn notebook(pane: &PaneCx<'_>, key: &str, thread: &Thread) -> El {
    let tokens = pane.tokens();
    let cells: Vec<&ToolCall> = thread.transcript.cells().collect();
    let title = thread.title.as_deref().unwrap_or("Untitled");
    let count = match cells.len() {
        1 => "1 cell".to_owned(),
        n => format!("{n} cells"),
    };
    let count = text(count).font(LABEL).color(colour(tokens.dim));
    let bar = header(tokens, vec![lead(tokens, title)], vec![count]);
    let linked = down(pane).map(|link| status::bar(pane, key, &link, &WORDS));
    let page = col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(bar.key(format!("{key}.pane-header")))
        .children(linked);
    if cells.is_empty() {
        return page.child(status::view(pane, key, &Status::Empty, &WORDS));
    }
    let book = pane.state::<Notebook>();
    let look = Look::of(tokens);
    let drawn = cell::Drawn {
        key,
        pane: pane.pane,
        tokens,
        look: &look,
        full: book.map(|book| &book.full),
    };
    let left_out = cells.len().saturating_sub(MAX_CELLS);
    let notice = (left_out > 0).then(|| {
        let words = format!(
            "[…] {MAX_CELLS} of {} cells shown, the newest (the pane's cap, {MAX_CELLS}); the older ones are in the chat",
            cells.len()
        );
        text(words)
            .key(format!("{key}.cells.cut"))
            .font(META)
            .color(colour(tokens.faint))
    });
    let shown = cells.iter().skip(left_out).map(|call| drawn.cell(call));
    let aim = book.and_then(|book| book.aim.as_ref());
    let aim = aim.filter(|aim| cells.iter().any(|call| &call.id == *aim));
    let list = col()
        .key(format!("{key}.cells"))
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: true })
        .child(
            col()
                .w_full()
                .px(tokens.gutter)
                .py(6.0)
                .gap(10.0)
                .children(notice)
                .children(shown),
        )
        .children(aim.map(|aim| aimed(key, pane.pane, aim)));
    page.child(list)
}
