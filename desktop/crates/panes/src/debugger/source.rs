//! The paused source on the code surface: the stop, the breakpoints and the values on its line.

use std::path::PathBuf;

use rune::{El, Tx};
use yd_core::address::thing::Thing;
use yd_core::address::uri::Address;
use yd_core::command::table::{Run, Target};
use yd_core::command::tools::debug::BREAK;
use yd_core::feed::{DEBUGGER, FILES, SourceCmd, Table};
use yd_core::ids::{PaneId, RowId};
use yd_core::kind::Kind;
use yd_core::msg::Msg;
use yd_core::row::{Row, Rows, Value};
use yd_ui::code::colour::{Loaded, Wake, load};
use yd_ui::code::doc::{Doc, Lang, LineNo};
use yd_ui::code::marks::{GutterMark, Marks, Tint};
use yd_ui::code::pointer::CodeEvent;
use yd_ui::code::rows::{Lines, line_of, thing};
use yd_ui::code::view::{CodeView, Gutter};
use yd_ui::list::state::Lists;
use yd_ui::menu::command_box;
use yd_ui::pane::PaneCx;
use yd_ui::win::{Hub, Win, ask};

use super::desc::{FRAME, STACK_PART};
use super::pane::session::Session;

pub mod plain;

/// Where the debugger stands in a file: the stop, or the frame the reader opened.
pub struct Stand {
    /// The path as the adapter names it; breakpoints are set and matched by it.
    pub path: String,
    pub line: Option<u32>,
    pub function: String,
    /// The top frame is the only one whose variables the source sends.
    pub top: bool,
}

fn frames(hub: &Hub) -> Option<&Table> {
    hub.model.tables.get(DEBUGGER)
}

/// The frame the reader opened in the call stack, else the stop itself.
pub fn stand(hub: &Hub, pane: PaneId, session: &Session) -> Option<Stand> {
    let open = hub
        .panes
        .get::<Lists>(Some(pane))
        .and_then(|lists| lists.list(STACK_PART))
        .and_then(|list| list.open());
    let opened = open
        .and_then(|id| frames(hub)?.find(id))
        .filter(|row| row.id.as_str().starts_with(FRAME));
    if let Some(row) = opened {
        let path = row.words("path").unwrap_or_default();
        return (!path.is_empty()).then(|| Stand {
            path: path.to_owned(),
            line: row.words("line").and_then(|line| line.parse().ok()),
            function: row.words("function").unwrap_or_default().to_owned(),
            top: row.get("current") == Some(&Value::Flag(true)),
        });
    }
    let top = frames(hub)?.find(&RowId::new("frame:0"));
    (!session.path.is_empty()).then(|| Stand {
        path: session.path.clone(),
        line: session.line,
        function: top
            .and_then(|row| row.words("function"))
            .unwrap_or_default()
            .to_owned(),
        top: true,
    })
}

/// The path the files source reads: relative to the shown thread's folder when it is under it.
fn relative(hub: &Hub, pane: PaneId, path: &str) -> String {
    let folder = yd_ui::shown::of_pane(hub, pane).map(|thread| yd_ui::shown::works_in(hub, thread));
    let under = folder.and_then(|folder| path.strip_prefix(folder.trim_end_matches('/')));
    under
        .map_or(path, |rest| rest.trim_start_matches('/'))
        .to_owned()
}

fn file_row<'a>(hub: &'a Hub, rel: &str) -> Option<&'a Row> {
    hub.model
        .tables
        .get(FILES)?
        .find(&RowId::new(format!("file:{rel}")))
}

fn loaded(hub: &Hub, row: &Row, rel: &str) -> Option<Loaded> {
    let stamp = hub.model.tables.get(FILES)?.version();
    let wake = hub.wins.first().map(|(_, app)| {
        let app = app.clone();
        Box::new(move || app.update(|_, _| {})) as Wake
    });
    Some(load(
        row.id.as_str(),
        stamp,
        row.words("text")?,
        Lang::for_path(rel),
        wake,
    ))
}

fn line_no(line: u32) -> Option<LineNo> {
    LineNo::try_from(line).ok()
}

/// Whether `line` names `path` as a whole word, not as part of a longer name or field.
fn mentions(line: &str, path: &str) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    line.match_indices(path).any(|(at, _)| {
        let before = line.get(..at).and_then(|head| head.chars().next_back());
        let after = line
            .get(at.saturating_add(path.len())..)
            .and_then(|tail| tail.chars().next());
        !before.is_some_and(|c| word(c) || c == '.') && !after.is_some_and(word)
    })
}

/// The values of the variables a line names, as `self.tokens = 0`.
fn values(vars: &[(String, String)], line: &str) -> Option<String> {
    let said: Vec<String> = vars
        .iter()
        .filter(|(path, _)| mentions(line, path))
        .map(|(path, value)| format!("{path} = {value}"))
        .collect();
    (!said.is_empty()).then(|| said.join("   "))
}

/// The line that opens the function the stop is in: the nearest above that calls its name.
fn opens(doc: &Doc, stop: LineNo, function: &str) -> Option<LineNo> {
    let name = function
        .rsplit("::")
        .next()
        .filter(|name| !name.is_empty())?;
    let call = format!("{name}(");
    (0..stop.index())
        .rev()
        .take(400)
        .map(LineNo::from_index)
        .find(|line| doc.line(*line).is_some_and(|text| text.contains(&call)))
}

fn marks(table: &Table, doc: &Doc, at: &Stand) -> Marks {
    let rows: Vec<&Row> = (0..table.len()).filter_map(|n| table.get(n)).collect();
    let mut marks = Marks::new();
    let stop = at.line.and_then(line_no);
    if let Some(stop) = stop {
        marks = marks.tint(stop, Tint::Current);
    }
    for row in rows.iter().filter(|row| row.id.as_str().starts_with("bp:")) {
        let on = row.get("enabled") == Some(&Value::Flag(true));
        let line = row
            .words("line")
            .and_then(|line| line.parse().ok())
            .and_then(line_no);
        if let (true, true, Some(line)) = (on, row.words("path") == Some(at.path.as_str()), line) {
            marks = marks.gutter(line, GutterMark::Breakpoint);
        }
    }
    let Some(stop) = stop.filter(|_| at.top) else {
        return marks;
    };
    let vars: Vec<(String, String)> = rows
        .iter()
        .filter_map(|row| {
            let path = row.id.as_str().strip_prefix("var:")?;
            let value = row.words("value").filter(|value| !value.is_empty())?;
            Some((path.to_owned(), value.to_owned()))
        })
        .collect();
    for line in [Some(stop), opens(doc, stop, &at.function)]
        .into_iter()
        .flatten()
    {
        if let Some(said) = doc.line(line).and_then(|text| values(&vars, text)) {
            marks = marks.value(line, said);
        }
    }
    marks
}

/// A press in the gutter sets a breakpoint on the line, or takes the one there away.
fn toggle(win: &mut Win, tx: &mut Tx, path: &str, line: LineNo) {
    let Ok(address) = Address::new(Kind::Lines, path) else {
        return;
    };
    let line = address.part(format!("L{}", line.get()));
    command_box::run(win, tx, Run::new(BREAK).on(Target::Thing(line)));
}

/// The source area: the file on the code surface, or the plain words of why there is none.
pub fn view(pane: &PaneCx<'_>, key: &str, session: Option<&Session>) -> El {
    let tokens = pane.tokens();
    let key = format!("{key}.source");
    let state = format!("{key}.state");
    let nothing = || {
        let (heading, line) = plain::words(session);
        plain::plain(tokens, &state, &heading, &line)
    };
    let Some(at) = session.and_then(|session| stand(pane.hub, pane.pane, session)) else {
        return nothing();
    };
    let rel = relative(pane.hub, pane.pane, &at.path);
    let Some(row) = file_row(pane.hub, &rel) else {
        let read = Msg::Source(FILES, SourceCmd::Read(PathBuf::from(&rel)));
        let note = plain::plain(tokens, &state, &format!("Reading {rel}"), "");
        return note.child(ask(format!("{key}.read"), read));
    };
    if let Some(why) = row.words("error") {
        return plain::plain(tokens, &state, &format!("Cannot show {rel}"), why);
    }
    let Some(file) = loaded(pane.hub, row, &rel) else {
        return nothing();
    };
    let table = frames(pane.hub).cloned().unwrap_or_default();
    let (marks, path) = (marks(&table, &file.doc, &at), at.path.clone());
    CodeView::new(&key, tokens, file.doc, file.colours)
        .marks(std::rc::Rc::new(marks))
        .gutter(Gutter {
            marks: true,
            signs: false,
        })
        .on_event(move |win, event, tx| {
            if let CodeEvent::GutterPress(line) = event {
                toggle(win, tx, &path, line);
            }
        })
        .build()
}

/// What a keyed line of the source stands for, for pick mode and the right-click menu.
pub fn thing_at(hub: &Hub, pane: PaneId, key: &str) -> Option<Thing> {
    let line = line_of(key)?;
    let session = Session::of(frames(hub))?;
    let at = stand(hub, pane, &session)?;
    let rel = relative(hub, pane, &at.path);
    let file = loaded(hub, file_row(hub, &rel)?, &rel)?;
    thing(&at.path, &file.doc, Lines::new(line, line))
}
