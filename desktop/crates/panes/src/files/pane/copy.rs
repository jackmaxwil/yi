//! The line over a thread's files that says which copy of the project they are.

use rune::{El, col, row, text};
use yd_core::address::uri::Address;
use yd_core::command::run::opened;
use yd_core::command::table::{OPEN_FILE, Route, Run, Target};
use yd_core::feed::{CHANGES, OPEN_FILES};
use yd_core::ids::RowId;
use yd_core::kind::Kind;
use yd_core::msg::Msg;
use yd_core::row::Rows;
use yd_ui::kit::button::{Button, button};
use yd_ui::pane::PaneCx;
use yd_ui::shown;
use yd_ui::theme::look::{META, colour, white};
use yd_ui::win::{Win, dispatch};

/// What the line says: `Agent's copy · yi/0f3a9c21 · from tally/main` while the panes read
/// the thread's own checkout, and the words of the press that shows the other folder.
fn said(pane: &PaneCx<'_>) -> Option<(String, &'static str)> {
    let thread = pane.thread()?;
    let project = thread.cwd.trim_end_matches('/');
    if shown::works_in(pane.hub, thread).trim_end_matches('/') == project {
        return None;
    }
    let name = project.rsplit('/').next().unwrap_or(project);
    if shown::in_project(pane.hub, &thread.id) {
        return Some((format!("Project folder · {name}"), "Show agent's copy"));
    }
    let repo = pane.model().tables.get(CHANGES);
    let repo = repo.and_then(|table| table.find(&RowId::new("repo")));
    let base = repo
        .and_then(|repo| repo.words("base"))
        .filter(|b| !b.is_empty());
    let branch = thread.branch();
    let short: String = branch.chars().take("yi/".len().saturating_add(8)).collect();
    let line = format!(
        "Agent's copy · {short} · from {name}/{}",
        base.unwrap_or("main")
    );
    Some((line, "Show project folder"))
}

/// Shows the other folder, and asks for the open file's text again from there: moving the
/// files source drops what it read.
fn flip(win: &mut Win) {
    shown::flip_project(win);
    let open = win.hub.try_borrow().ok().and_then(|hub| {
        let tabs = hub.model.tables.get(OPEN_FILES)?;
        let tab = (0..tabs.len())
            .filter_map(|at| tabs.get(at))
            .max_by_key(|t| opened(t))?;
        Address::new(Kind::File, tab.words("path")?).ok()
    });
    if let Some(file) = open {
        let run = Run::new(OPEN_FILE).on(Target::Thing(file));
        dispatch(win, Msg::Command(Route::Model, run));
    }
}

/// Invariant: the line goes at the foot, since it comes once the checkout is known and must
/// not move the lines under a resting pointer; none when the panes read the project itself.
pub fn under(pane: &PaneCx<'_>, key: &str, body: El) -> El {
    let Some((words, press)) = said(pane) else {
        return body;
    };
    let tokens = pane.tokens();
    let show = button(tokens, Button::Ghost, press, None)
        .key(format!("{key}.copy.show"))
        .on_press(flip);
    let line = row()
        .key(format!("{key}.copy"))
        .w_full()
        .h(22.0)
        .px(tokens.gutter)
        .gap(6.0)
        .center_y()
        .border_sides([1.0, 0.0, 0.0, 0.0], colour(white(tokens.inner_lines)))
        .child(
            text(words)
                .font(META)
                .color(colour(tokens.dim))
                .truncate()
                .grow(1.0)
                .min_w(0.0),
        )
        .child(show);
    let rest = col().w_full().grow(1.0).min_h(0.0).child(body);
    col().w_full().h_full().child(rest).child(line)
}
