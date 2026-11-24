//! The model's `show`: a file put where the person can see it, and the answer that says how.

use serde_json::Value;
use yi_types::acp::{AcpShowOutcome, AcpShowParams, AcpShowResult, AcpShowStrength};

use crate::address::thing::lines;
use crate::address::uri::Address;
use crate::grid::step::{GridMsg, step};
use crate::ids::{Millis, PaneId, ThreadId};
use crate::kind::Kind;
use crate::model::Model;
use crate::msg::Effect;
use crate::pane::PaneKind;
use crate::wire::request::Request;

/// How long the app must have been left alone before the model's `front` comes forward.
// ponytail: the whole app's stillness, not one place's; per place if that holds shows back.
pub const IDLE: Millis = Millis(10_000);

/// The file an address names and its line: `yi://file/<path>#L<n>`, `<path>:<n>`, or a path.
/// One under `cwd` is named by its place in it, as the files source names a file.
pub fn target(address: &str, cwd: &str) -> Option<(String, Option<u32>)> {
    let (path, line) = match address.strip_prefix("yi://") {
        Some(_) => {
            let named: Address = address.parse().ok()?;
            let file = matches!(named.kind(), Kind::File | Kind::Lines);
            let line = named.part_text().and_then(lines).map(|(first, _)| first);
            (file.then(|| named.id().to_owned())?, line)
        }
        None => match address.rsplit_once(':') {
            Some((path, line)) if !path.is_empty() && line.parse::<u32>().is_ok() => {
                (path.to_owned(), line.parse().ok())
            }
            _ => (address.to_owned(), None),
        },
    };
    let inside = path
        .strip_prefix(cwd)
        .and_then(|rest| rest.strip_prefix('/'));
    let path = inside.filter(|_| !cwd.is_empty()).unwrap_or(&path);
    (!path.is_empty()).then(|| (path.to_owned(), line))
}

fn refused(note: &str) -> AcpShowResult {
    AcpShowResult {
        outcome: AcpShowOutcome::Refused,
        note: Some(note.to_owned()),
    }
}

/// Whether `pane` is drawn in `window`: docked there with no other pane filling the window.
fn drawn(model: &Model, pane: PaneId) -> bool {
    let grid = &model.grid;
    let Some(window) = grid.window_of(pane) else {
        return false;
    };
    grid.fills(window).is_none_or(|filled| filled == pane)
}

fn shown(model: &mut Model, params: &AcpShowParams, effects: &mut Vec<Effect>) -> AcpShowResult {
    let thread = ThreadId::new(params.session_id.as_str());
    let mut shows = model.grid.shows.iter();
    let Some(window) = shows
        .find(|(_, shown)| **shown == thread)
        .map(|(id, _)| *id)
    else {
        return refused("That session is not on screen.");
    };
    let file = target(params.address.trim(), &params.cwd);
    let file = file.and_then(|(path, line)| Some((Address::new(Kind::File, path).ok()?, line)));
    let Some((thing, line)) = file else {
        return refused("The app shows files; this address is not one.");
    };
    // Whatever the person did has been dated by now: the step that hands this over does it.
    let idle = model.now.0.saturating_sub(model.touched.0) >= IDLE.0;
    let front = params.strength == AcpShowStrength::Front && idle;
    let kind = PaneKind::Code;
    model.grid.rules = model.settings.opens();
    let point = GridMsg::Point {
        window,
        kind,
        thing: thing.clone(),
        front,
    };
    effects.extend(step(&mut model.grid, point));
    let about = |pane: &&crate::grid::windows::Pane| {
        pane.kind == kind && pane.thing.as_ref() == Some(&thing)
    };
    let Some(pane) = model.grid.panes.iter().find(about).map(|pane| pane.id) else {
        return refused("There is no room for another pane.");
    };
    if let Some(line) = line {
        model.reveal(pane, line);
    }
    let seen = drawn(model, pane);
    let waits = params.strength == AcpShowStrength::Front && !seen;
    let note = match (waits, idle) {
        (true, false) => Some("The user is busy; it waits as a tab."),
        (true, true) => Some("Another pane fills the window; it waits as a tab."),
        (false, _) => None,
    };
    AcpShowResult {
        outcome: if seen {
            AcpShowOutcome::Front
        } else {
            AcpShowOutcome::Tab
        },
        note: note.map(str::to_owned),
    }
}

/// Carries out one ask and answers it under the id the server gave.
pub(crate) fn answer(model: &mut Model, id: &Value, params: &AcpShowParams) -> Vec<Effect> {
    let mut effects = Vec::new();
    let result = shown(model, params, &mut effects);
    let result = serde_json::to_value(result).unwrap_or_default();
    effects.push(Effect::Wire(Request::answer(id, result)));
    effects
}
