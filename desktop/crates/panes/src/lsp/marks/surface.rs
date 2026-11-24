//! What the code surface shows beside the text: the hover card and the quick-fix bar.

use std::rc::Rc;

use rune::{Cx, El, Tx, row, text};
use yd_core::address::uri::Address;
use yd_core::command::by_kind::ASK;
use yd_core::command::table::{Run, Target};
use yd_core::feed::LSP;
use yd_core::ids::RowId;
use yd_core::kind::Kind;
use yd_core::row::{Dot, Value};
use yd_ui::code::doc::{Lang, LineNo};
use yd_ui::code::hover::{self, Card, Link, Press};
use yd_ui::grid_view::view::open_here;
use yd_ui::kit::button::{Button, button};
use yd_ui::kit::status::{badge, dot};
use yd_ui::menu::command_box;
use yd_ui::theme::look::{LABEL, colour};
use yd_ui::theme::scale::{CARD, RADIUS_CARD};
use yd_ui::win::{Hub, Win};

use super::{
    APPLY_FIX, DEFINITION, FIXES, File, Kept, REFERENCES, ask, kept, lines_at, number, problems,
    rows, spot_of,
};

/// Where the bar starts along its row, clear of the gutter.
const BAR_AT: f32 = 40.0;

/// Asks for the fixes of the problem on `line` the first time it is looked at.
pub(super) fn fixes(win: &mut Win, file: &File<'_>, line: usize) {
    let found = win.hub.try_borrow().ok().and_then(|hub| {
        let table = hub.model.tables.get(LSP)?;
        let has = rows(table).any(|row| {
            spot_of(row.id.as_str(), "fix:").is_some_and(|(p, l, _)| p == file.path && l == line)
        });
        let problem = problems(table, file.path).find(|problem| problem.line == line)?;
        (!has).then_some(problem.col)
    });
    let id = format!("fixes:{}:{line}", file.path);
    let Some(at) = found.and_then(|col| lines_at(file.path, line, col)) else {
        return;
    };
    if kept(win, |kept, epoch| kept.asked.once(&id, epoch)) == Some(true) {
        ask(win, FIXES, at);
    }
}

/// Asks where the name at `line` and `col` is defined, and waits for the answer's row.
pub(super) fn go_to(win: &mut Win, path: &str, line: usize, col: usize) {
    let Some(at) = lines_at(path, line, col) else {
        return;
    };
    let id = RowId::new(format!("def:{path}:{line}:{col}"));
    kept(win, |kept, _| kept.pending = Some(id));
    ask(win, DEFINITION, at);
}

/// The place the awaited definition row names, once it is in the feed.
fn answer(hub: &Hub) -> Option<Address> {
    let pending = hub.panes.get::<Kept>(None)?.pending.as_ref()?;
    let row = hub.model.tables.get(LSP)?.find(pending)?;
    let (path, line) = (row.words("path")?, number(row, "line")?);
    Some(
        Address::new(Kind::Lines, path)
            .ok()?
            .part(format!("L{line}")),
    )
}

/// Opens the answer to a go-to-definition. True when a file was opened.
pub(super) fn arrived(win: &mut Win, tx: &mut Tx) -> bool {
    let place = win.hub.try_borrow().ok().and_then(|hub| answer(&hub));
    let Some(place) = place else {
        return false;
    };
    kept(win, |kept, _| kept.pending = None);
    open_here(win, tx, place);
    true
}

/// The card of the token the pointer rests on in the surface keyed `surface`, the problem
/// first, then the server's words. Add it last in the pane; `cx` gives the surface's width.
pub fn card(hub: &Hub, cx: &Cx<Win>, surface: &str) -> Option<El> {
    let resting = hub.panes.get::<Kept>(None)?.resting.as_ref();
    let resting = resting.filter(|resting| resting.surface == surface)?;
    let table = hub.model.tables.get(LSP)?;
    let (path, line) = (
        resting.path.as_str(),
        resting.line.index().saturating_add(1),
    );
    let col = resting.col;
    let id = format!("{path}:{line}:{col}");
    let told = table.find(&RowId::new(format!("hover:{id}")));
    let problem = problems(table, path).find(|problem| {
        problem.line == line
            && (problem.col..problem.end.max(problem.col.saturating_add(1))).contains(&col)
    });
    if told.is_none() && problem.is_none() {
        return None;
    }
    let said = told.and_then(|row| row.words("text")).unwrap_or_default();
    let (origin, markdown) = origin_of(said);
    let known = format!("ref:{id}:");
    let known = rows(table)
        .filter(|row| row.id.as_str().starts_with(&known))
        .count();
    let places = |label: String, id: &'static str, command| {
        let (owned, at) = (path.to_owned(), lines_at(path, line, col)?);
        let press: Press = match command {
            DEFINITION => Rc::new(move |win: &mut Win, _: &mut Tx| go_to(win, &owned, line, col)),
            _ => Rc::new(move |win: &mut Win, _: &mut Tx| ask(win, command, at.clone())),
        };
        Some(Link {
            id,
            label,
            press: Some(press),
        })
    };
    let references = match known {
        0 => "Find references".to_owned(),
        n => format!("{n} references"),
    };
    let links: Vec<Link> = told
        .map(|_| {
            let rename = Link {
                id: "rename",
                label: "Rename".to_owned(),
                press: None,
            };
            let found = [
                places("Go to definition".to_owned(), "definition", DEFINITION),
                places(references, "references", REFERENCES),
            ];
            found.into_iter().flatten().chain([rename]).collect()
        })
        .unwrap_or_default();
    let problem = problem.map(|problem| {
        let message = problem.row.words("message").unwrap_or_default();
        (problem.words(), problem.tone(), message)
    });
    let key = format!("{surface}.hover");
    let shown = Card {
        problem,
        markdown: &markdown,
        links,
        origin: origin.as_deref(),
    };
    let anchor = format!("{surface}.row.L{}", resting.line.get());
    let room = cx
        .layout_size(&format!("{surface}.code"))
        .map(|[width, _]| width);
    let card = hover::card(&hub.model.tokens, &key, &shown);
    Some(hover::under(card, &anchor, resting.shift, room))
}

/// A server names where an item comes from in a first block of its own (`core::time`): that
/// block becomes the card's origin, its first part, and the rest is the Markdown.
fn origin_of(said: &str) -> (Option<String>, String) {
    let mut parts = said.splitn(2, "```\n");
    let first = parts.next().unwrap_or_default();
    let path = first.split_once('\n').map(|(_, path)| path.trim());
    match (path, parts.next()) {
        (Some(path), Some(rest)) if path.contains("::") && !path.contains(' ') => {
            let origin = path.split("::").next().map(str::to_owned);
            (origin, rest.trim_start().to_owned())
        }
        _ => (None, said.to_owned()),
    }
}

/// The server that reads the file at `path`, as its row's dot and name (record 6.4.2); none
/// while no server of the file's language has a row.
pub fn server(hub: &Hub, key: &str, path: &str) -> Vec<El> {
    let tokens = &hub.model.tokens;
    let language = Lang::for_path(path).title();
    let all = hub.model.tables.get(LSP).into_iter().flat_map(rows);
    let mut servers = all.filter(|row| row.id.as_str().starts_with("server:"));
    let Some(server) = servers.find(|row| row.words("language") == Some(language)) else {
        return Vec::new();
    };
    let status = server.words("status").unwrap_or_default();
    let (state, running) = match server.get("state") {
        Some(Value::Dot(state)) => (Some(dot(tokens, *state)), *state == Dot::Run),
        _ => (None, false),
    };
    let state = state.map(|dot| dot.key(format!("{key}.breadcrumb.server")).help(status));
    let name = server.words("server").unwrap_or_default();
    let name = text(name).font(LABEL).color(colour(tokens.dim));
    // A server that is not running says why on the line, not only in its tooltip.
    let why = text(status).font(LABEL).color(colour(tokens.faint));
    let why = (!running).then(|| why.truncate().grow(1.0).min_w(0.0));
    state.into_iter().chain([name]).chain(why).collect()
}

/// The first line of `path` with a problem that has a fix: where the bar goes.
pub fn bar_line(hub: &Hub, path: &str) -> Option<LineNo> {
    let table = hub.model.tables.get(LSP)?;
    let fixed = rows(table).filter_map(|row| spot_of(row.id.as_str(), "fix:"));
    let fixed: Vec<usize> = fixed
        .filter(|(p, _, _)| *p == path)
        .map(|(_, l, _)| l)
        .collect();
    let mut with: Vec<usize> = problems(table, path)
        .map(|problem| problem.line)
        .filter(|line| fixed.contains(line))
        .collect();
    with.sort_unstable();
    with.first()
        .map(|line| LineNo::from_index(line.saturating_sub(1)))
}

/// The bar under `line`: the problem, the fix to apply and a way to ask the agent instead.
pub fn bar(hub: &Hub, surface: &str, path: &str, line: LineNo) -> El {
    let number = line.index().saturating_add(1);
    let table = hub.model.tables.get(LSP);
    let problem = table.and_then(|t| problems(t, path).find(|p| p.line == number));
    let prefix = format!("fix:{path}:{number}:");
    let fix = table.and_then(|t| rows(t).find(|row| row.id.as_str().starts_with(&prefix)));
    let (Some(problem), Some(fix)) = (problem, fix) else {
        return row();
    };
    let tokens = &hub.model.tokens;
    let (apply, ask_agent) = (
        fix.id.as_str().to_owned(),
        problem.row.id.as_str().to_owned(),
    );
    let message = problem.row.words("message").unwrap_or_default();
    let words = format!("Apply: {}", fix.words("title").unwrap_or_default());
    let apply = button(tokens, Button::Primary, "Fix", None)
        .key(format!("{surface}.fix-bar.apply"))
        .help(words)
        .on_press_tx(move |win: &mut Win, _: &mut Tx| {
            if let Ok(at) = Address::new(Kind::Problem, apply.as_str()) {
                ask(win, APPLY_FIX, at);
            }
        });
    let other = button(tokens, Button::Ghost, "Ask agent", None)
        .key(format!("{surface}.fix-bar.ask"))
        .on_press_tx(move |win: &mut Win, tx: &mut Tx| {
            if let Ok(at) = Address::new(Kind::Problem, ask_agent.as_str()) {
                command_box::run(win, tx, Run::new(ASK).on(Target::Thing(at)));
            }
        });
    let card = row()
        .key(format!("{surface}.fix-bar"))
        .self_start()
        .h(26.0)
        .px(7.0)
        .gap(7.0)
        .center_y()
        .radius(RADIUS_CARD.of(tokens))
        .fill(colour(CARD.of(tokens)))
        .child(badge(tokens, problem.words(), problem.tone()))
        .child(
            text(message)
                .font(LABEL)
                .color(colour(tokens.dim))
                .truncate(),
        )
        .child(apply)
        .child(other);
    row().w_full().pl(BAR_AT).py(2.0).child(card)
}
