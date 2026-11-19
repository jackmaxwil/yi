//! Find references: the ⌘-click, the peek panel under the clicked line, attach all.

use std::{collections::HashSet, ops::Range};

use rune::{
    Cursor, Cx, El, Icon, Key, KeyEvent, ScrollSpec, Span, TextStyle, Tx, col, el, rich, row,
    spacer, text,
};
use yd_core::address::attach::{AttachMsg, MAX_ATTACHED};
use yd_core::address::{thing::Thing, uri::Address};
use yd_core::command::table::{Route, Run, Target};
use yd_core::command::{by_kind, run::opened, tools::lsp};
use yd_core::feed::{FILES, FeedEvent, FeedStatus, LSP, OPEN_FILES, Table};
use yd_core::ids::{CommandId, PaneId, RowId};
use yd_core::kind::Kind;
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::row::{Dot, Row, Rows, Value, rows_from_json};
use yd_core::tokens::Tokens;
use yd_ui::code::doc::{Cols, Doc, Lang, LineNo};
use yd_ui::code::pointer::CodeEvent;
use yd_ui::code::view::reveal;
use yd_ui::grid_view::view::{alone, open_file};
use yd_ui::kit::chip::{Chip, chip};
use yd_ui::kit::icons::{SEARCH, glyph};
use yd_ui::pane::{SCENE_PANE, Scene, Stage};
use yd_ui::theme::look::{CODE_LINE, LABEL, META, colour, white};
use yd_ui::theme::scale::{CODE_MATCH, LINE_PICKED, SEMIBOLD, WELL};
use yd_ui::win::{Hub, Win, dispatch};

use super::pane::{DEF, Slot, Viewer};

const PANEL: &str = "references";
/// The list scrolls inside itself past this height: the record's 176 less header and padding.
const LIST_MAX: f32 = 153.0;
const WAITING: &str = "Looking for references…";
const NOTHING: &str = "No references came back for this name.";
const BOLD: TextStyle = TextStyle {
    weight: SEMIBOLD,
    ..CODE_LINE
};
static CLOSE: Icon = yd_ui::line_icon!("close", r#"<path d="M6 6l12 12M18 6L6 18"/>"#);

#[derive(Default)]
pub struct Refs {
    asked: Option<Asked>,
    /// The name last right-clicked: the menu's item is aimed at its line alone.
    pressed: Option<(LineNo, Cols)>,
}

struct Asked {
    path: String,
    line: LineNo,
    /// The name's first character, counted in characters from 1: what the server is asked.
    col: usize,
    name: String,
    cursor: Option<usize>,
    /// How many questions the servers had answered when this one went out, and the feed's
    /// version then: a status it already had is an older question's.
    served: u64,
    stamp: u64,
    note: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct Reference {
    id: RowId,
    path: String,
    line: LineNo,
    /// The matched characters, counted from 0.
    chars: Range<usize>,
    text: String,
    kind: String,
}

fn rows_of(table: &Table) -> impl Iterator<Item = &Row> {
    (0..table.len()).filter_map(|at| table.get(at))
}

fn reference(row: &Row) -> Option<Reference> {
    let number = |field: &str| row.words(field)?.parse::<usize>().ok();
    let from = number("col")?.checked_sub(1)?;
    let to = number("end_col")?.checked_sub(1)?;
    Some(Reference {
        id: row.id.clone(),
        path: row.words("path")?.to_owned(),
        line: LineNo::try_from(u32::try_from(number("line")?).ok()?).ok()?,
        chars: from..to.max(from),
        text: row.words("text")?.to_owned(),
        kind: row.words("kind").unwrap_or("read").to_owned(),
    })
}

fn servers(table: &Table) -> impl Iterator<Item = &str> {
    let rows = rows_of(table).filter(|row| row.id.as_str().starts_with("server:"));
    rows.filter_map(|row| row.words("status"))
}

/// Questions the servers say they answered, `answered 3 requests`: an empty answer counts.
fn served(table: &Table) -> u64 {
    let count = |said: &str| {
        let (_, after) = said.split_once("answered ")?;
        after.split(' ').next()?.parse::<u64>().ok()
    };
    servers(table)
        .filter_map(count)
        .fold(0, u64::saturating_add)
}

/// The cut a server names for its last references answer, `[…] last answer kept 500 of N`.
fn cut(table: &Table) -> Option<&str> {
    fn named(said: &str) -> Option<&str> {
        said.get(said.find("[…]")?..)
    }
    let mut cuts = servers(table).filter_map(named);
    cuts.find(|cut| cut.contains("references"))
}

/// What the server said to the open question: its references, else the sentence to show:
/// what the source said since it was asked, nothing found, every server down, or waiting.
fn answer(hub: &Hub, asked: &Asked) -> Result<Vec<Reference>, String> {
    let table = hub.model.tables.get(LSP).ok_or(WAITING)?;
    let prefix = format!("ref:{}:{}:{}:", asked.path, asked.line.get(), asked.col);
    let ours = rows_of(table).filter(|row| row.id.as_str().starts_with(&prefix));
    let mut found: Vec<Reference> = ours.filter_map(reference).collect();
    // The definition's file leads, the definition leads its file, the rest run by line.
    let home = found
        .iter()
        .find(|f| f.kind == "definition")
        .map(|f| f.path.clone());
    let order = |f: &Reference| {
        (
            Some(&f.path) != home.as_ref(),
            f.path.clone(),
            f.kind != "definition",
            f.line,
        )
    };
    found.sort_by_key(order);
    let down = |r: &Row| matches!(r.get("state"), Some(Value::Dot(Dot::Stop | Dot::Fail)));
    let mut servers = rows_of(table).filter(|r| r.id.as_str().starts_with("server:"));
    let first = servers.next().filter(|first| down(first));
    Err(match table.status() {
        _ if !found.is_empty() => return Ok(found),
        FeedStatus::Error(why) if table.version() != asked.stamp => why,
        _ if served(table) > asked.served => NOTHING,
        _ => match first.and_then(|first| first.words("status")) {
            Some(why) if servers.all(down) => why,
            _ => WAITING,
        },
    }
    .to_owned())
}

fn shown(hub: &Hub) -> Option<(&str, Option<&str>)> {
    let tab = rows_of(hub.model.tables.get(OPEN_FILES)?).max_by_key(|tab| opened(tab))?;
    let file = hub.model.tables.get(FILES)?.find(&tab.id);
    Some((tab.words("path")?, file.and_then(|file| file.words("text"))))
}

fn asked_of(hub: &Hub, pane: PaneId) -> Option<&Asked> {
    hub.panes.get::<Refs>(Some(pane))?.asked.as_ref()
}

fn asked_mut(hub: &mut Hub, pane: PaneId) -> Option<&mut Asked> {
    hub.panes.get_mut::<Refs>(Some(pane))?.asked.as_mut()
}

fn surface(pane: PaneId) -> String {
    format!("{}-{}", PaneKind::Code.slug(), pane.0)
}

fn code_pane(win: &Win) -> Option<PaneId> {
    let hub = win.hub.try_borrow().ok()?;
    let grid = &hub.model.grid;
    let mut docked = grid.window(win.window)?.layout.docked();
    docked.find_map(|(pane, _)| (grid.kind(pane) == Some(PaneKind::Code)).then_some(pane))
}

/// Puts the panel under `line` and returns the question for the server. The name is the
/// source bytes `cols` of that line; its column goes out in characters.
fn begin(hub: &mut Hub, pane: PaneId, line: LineNo, cols: Cols) -> Option<Run> {
    let (path, text) = shown(hub)?;
    let doc = Doc::new(text?, Lang::for_path(path), 0);
    let source = doc.line(line)?;
    let name = source.get(cols.within(source)?)?.to_owned();
    let col = source
        .get(..usize::try_from(cols.start()).ok()?)?
        .chars()
        .count()
        + 1;
    let at = Address::new(Kind::Lines, path)
        .ok()?
        .part(format!("L{}:C{col}", line.get()));
    let table = hub.model.tables.get(LSP);
    let asked = Asked {
        path: path.to_owned(),
        line,
        col,
        name,
        cursor: None,
        served: table.map_or(0, served),
        stamp: table.map_or(0, Rows::version),
        note: None,
    };
    hub.panes.get_mut::<Refs>(Some(pane))?.asked = Some(asked);
    let slot = Slot {
        after: line,
        name: PANEL,
        build: peek,
    };
    hub.panes.get_mut::<Viewer>(Some(pane))?.slot = Some(slot);
    Some(Run::new(lsp::REFERENCES).on(Target::Thing(at)))
}

fn with<T>(win: &Win, f: impl FnOnce(&mut Hub) -> Option<T>) -> Option<T> {
    let mut hub = win.hub.try_borrow_mut().ok()?;
    f(&mut hub)
}

fn close(win: &mut Win, pane: PaneId) {
    with(win, |hub| {
        hub.panes.get_mut::<Viewer>(Some(pane))?.slot = None;
        hub.panes.get_mut::<Refs>(Some(pane))?.asked = None;
        Some(())
    });
}

fn ask(win: &mut Win, tx: &mut Tx, pane: PaneId, line: LineNo, cols: Cols) {
    if let Some(run) = with(win, |hub| begin(hub, pane, line, cols)) {
        dispatch(win, Msg::Command(Route::Source(LSP), run));
        tx.focus_when_shown(format!("{}.refs", surface(pane)), false);
    }
}

/// A ⌘-click on a name asks the language server for its references and opens the panel under
/// that line; a right-click only remembers its name for the menu's item. False for the rest.
pub fn heard(win: &mut Win, tx: &mut Tx, pane: PaneId, event: CodeEvent) -> bool {
    match event {
        CodeEvent::CommandPress(token) => ask(win, tx, pane, token.line, token.cols),
        CodeEvent::SecondaryPress {
            line,
            token: Some(token),
            ..
        } => {
            with(win, |hub| {
                hub.panes.get_mut::<Refs>(Some(pane))?.pressed = Some((line, token.cols));
                Some(())
            });
            return false;
        }
        _ => return false,
    }
    true
}

/// `Find references` from the right-click menu, aimed at lines of the open file: it asks for
/// the name last right-clicked on that line.
pub fn carry(win: &mut Win, tx: &mut Tx, command: CommandId, target: &Target) -> bool {
    let Target::Thing(address) = target else {
        return false;
    };
    if !matches!(command, by_kind::REFERENCES | lsp::REFERENCES) || address.kind() != Kind::Lines {
        return false;
    }
    let part = address.part_text().unwrap_or("L1");
    let number = part
        .split(['-', ':'])
        .next()
        .and_then(|n| n.strip_prefix('L'));
    let line = number.and_then(|n| LineNo::try_from(n.parse::<u32>().ok()?).ok());
    let Some((line, pane)) = line.zip(code_pane(win)) else {
        return false;
    };
    let pressed = with(win, |hub| hub.panes.get::<Refs>(Some(pane))?.pressed);
    let cols = pressed.filter(|(at, _)| *at == line).map(|(_, cols)| cols);
    cols.inspect(|cols| ask(win, tx, pane, line, *cols))
        .is_some()
}

fn thing_of(found: &Reference) -> Option<Thing> {
    let name = found.path.rsplit('/').next().unwrap_or(&found.path);
    let part = format!("L{}", found.line.get());
    Some(Thing {
        label: format!("{name}:{}", found.line.get()),
        address: Address::new(Kind::Lines, &found.path).ok()?.part(part),
        excerpt: Some(found.text.trim().to_owned()).filter(|text| !text.is_empty()),
    })
}

fn found(hub: &Hub, pane: PaneId) -> Vec<Reference> {
    let asked = asked_of(hub, pane);
    asked
        .and_then(|asked| answer(hub, asked).ok())
        .unwrap_or_default()
}

fn row_key(pane: PaneId, found: &Reference) -> String {
    format!("{}.refs.ref.{}", surface(pane), found.id.as_str())
}

pub fn thing_at(hub: &Hub, pane: PaneId, key: &str) -> Option<Thing> {
    thing_of(
        found(hub, pane)
            .iter()
            .find(|found| row_key(pane, found) == key)?,
    )
}

/// Attaches every reference not yet attached, up to the twelve a prompt carries, and says in
/// the panel how many were left out.
fn attach_all(win: &mut Win, pane: PaneId) {
    let Ok(hub) = win.hub.try_borrow() else {
        return;
    };
    let attached = &hub.model.attached.things;
    let mut seen: HashSet<Address> = attached.iter().map(|t| t.address.clone()).collect();
    let room = MAX_ATTACHED.saturating_sub(attached.len());
    let things: Vec<Thing> = found(&hub, pane).iter().filter_map(thing_of).collect();
    drop(hub);
    let fresh: Vec<Thing> = things
        .into_iter()
        .filter(|t| seen.insert(t.address.clone()))
        .collect();
    let (added, left) = (fresh.len().min(room), fresh.len().saturating_sub(room));
    for thing in fresh.into_iter().take(added) {
        dispatch(win, Msg::Attach(AttachMsg::Toggle(thing)));
    }
    let said =
        format!("Attached {added}; {left} left out. A prompt carries at most {MAX_ATTACHED}:");
    let note = (left > 0).then(|| format!("{said} take some off the tray, then attach again."));
    with(win, |hub| {
        asked_mut(hub, pane).map(|asked| asked.note = note)
    });
}

fn open_at(win: &mut Win, tx: &mut Tx, pane: PaneId, found: &Reference) {
    if let Ok(file) = Address::new(Kind::File, &found.path) {
        open_file(win, tx, file);
        reveal(tx, &surface(pane), found.line);
    }
}

/// The reference picked: the arrow keys', else the first that is not the definition.
fn pick(asked: &Asked, all: &[Reference]) -> usize {
    let first = all.iter().position(|f| f.kind != "definition").unwrap_or(0);
    asked
        .cursor
        .unwrap_or(first)
        .min(all.len().saturating_sub(1))
}

/// Up and Down move the pick, Return opens it, Esc closes the panel.
fn keys(win: &mut Win, event: &KeyEvent, tx: &mut Tx, pane: PaneId) -> bool {
    let read = win.hub.try_borrow().ok().and_then(|hub| {
        let all = found(&hub, pane);
        Some((pick(asked_of(&hub, pane)?, &all), all))
    });
    let Some((at, all)) = read else {
        return false;
    };
    let last = all.len().saturating_sub(1);
    let to = match event.key {
        Key::Down => at.saturating_add(1).min(last),
        Key::Up => at.saturating_sub(1),
        Key::Enter => {
            all.get(at).inspect(|found| open_at(win, tx, pane, found));
            return true;
        }
        Key::Escape => {
            close(win, pane);
            return true;
        }
        _ => return false,
    };
    with(win, |hub| {
        asked_mut(hub, pane).map(|asked| asked.cursor = Some(to))
    });
    all.get(to)
        .inspect(|found| tx.scroll_into_view(row_key(pane, found)));
    true
}

fn pressable(el: El, press: impl Fn(&mut Win, &mut Tx) + 'static) -> El {
    el.cursor(Cursor::Pointer).on_press_tx(press)
}

fn line_row(tokens: &Tokens, pane: PaneId, found: &Reference, on: bool) -> El {
    let lit = colour(CODE_MATCH.of(tokens).alpha(0.32));
    let byte = |n: usize| {
        found
            .text
            .char_indices()
            .nth(n)
            .map_or(found.text.len(), |(b, _)| b)
    };
    let marked = (found.chars.start < found.chars.end)
        .then(|| byte(found.chars.start)..byte(found.chars.end));
    let spans: Vec<Span> = marked
        .map(|r| Span::new(r.start, r.end).bg(lit, 2.0))
        .into_iter()
        .collect();
    let number = text(found.line.get().to_string())
        .font(CODE_LINE)
        .color(colour(tokens.faint));
    let ink = colour(if on { tokens.fg } else { tokens.dim });
    let words = rich(found.text.as_str(), spans).font(CODE_LINE).color(ink);
    let kind = (found.kind != "read").then(|| text(found.kind.as_str()).font(META));
    let target = found.clone();
    let line = row()
        .key(row_key(pane, found))
        .w_full()
        .h(tokens.line)
        .pl(20.0)
        .pr(tokens.gutter)
        .gap(8.0)
        .center_y()
        .child(row().w(22.0).justify_end().child(number))
        .child(words.truncate_fade_width(0.0).grow(1.0))
        .children(kind.map(|kind| kind.color(colour(tokens.accent))));
    let bar = [0.0, 0.0, 0.0, 2.0];
    let line = if on {
        line.fill(colour(tokens.selected))
            .border_sides(bar, colour(LINE_PICKED.of(tokens)))
    } else {
        line
    };
    pressable(line, move |win, tx| open_at(win, tx, pane, &target))
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn head(tokens: &Tokens, pane: PaneId, asked: &Asked, total: Option<(usize, usize)>) -> El {
    let key = surface(pane);
    let count = |(n, files)| {
        format!(
            "{n} reference{} in {files} file{}",
            plural(n),
            plural(files)
        )
    };
    let attach = chip(tokens, Chip::Plain, "Attach all", None, false);
    let attach = pressable(
        attach.key(format!("{key}.refs.attach-all")),
        move |win, _| attach_all(win, pane),
    );
    let shut = row()
        .key(format!("{key}.refs.close"))
        .label("Close references");
    let shut = shut.px(4.0).py(4.0);
    let shut = pressable(
        shut.child(glyph(&CLOSE, 10.0, tokens.faint)),
        move |win, _| close(win, pane),
    );
    let dim = |said: String| text(said).font(LABEL).color(colour(tokens.dim));
    row()
        .key(format!("{key}.refs.head"))
        .w_full()
        .h(tokens.row)
        .px(tokens.gutter)
        .gap(7.0)
        .center_y()
        .fill(colour(tokens.context.alpha(0.12)))
        .child(
            text(asked.name.as_str())
                .font(BOLD)
                .color(colour(tokens.fg)),
        )
        .children(total.map(count).map(dim))
        .child(spacer())
        .children(total.map(|_| attach))
        .child(shut)
}

fn file_row(tokens: &Tokens, pane: PaneId, path: &str, count: usize) -> El {
    let faint = colour(tokens.faint);
    row()
        .key(format!("{}.refs.file.{path}", surface(pane)))
        .w_full()
        .pt(4.0)
        .pb(1.0)
        .px(tokens.gutter)
        .gap(8.0)
        .child(text(path).font(CODE_LINE.lh(14.0)).color(faint))
        .child(
            text(count.to_string())
                .font(CODE_LINE.lh(14.0))
                .color(faint),
        )
}

/// The panel under the clicked line: the answer grouped by file, or why there is none yet.
fn peek(hub: &Hub, pane: PaneId) -> El {
    let Some(asked) = asked_of(hub, pane) else {
        return el();
    };
    if shown(hub).map(|(path, _)| path) != Some(asked.path.as_str()) {
        return el();
    }
    let tokens = &hub.model.tokens;
    let answered = answer(hub, asked);
    let found = answered.is_ok();
    let (top, body) = match answered {
        Ok(all) => {
            let at = pick(asked, &all);
            let groups: Vec<&[Reference]> = all.chunk_by(|a, b| a.path == b.path).collect();
            let (mut body, mut n) = (Vec::new(), 0);
            for group in &groups {
                let path = group.first().map_or("", |first| first.path.as_str());
                body.push(file_row(tokens, pane, path, group.len()));
                for found in *group {
                    body.push(line_row(tokens, pane, found, n == at));
                    n += 1;
                }
            }
            (
                head(tokens, pane, asked, Some((all.len(), groups.len()))),
                body,
            )
        }
        Err(why) => {
            let line = row().w_full().h(tokens.line).px(tokens.gutter).center_y();
            let line = line.child(text(why).font(CODE_LINE).color(colour(tokens.dim)));
            (head(tokens, pane, asked, None), vec![line])
        }
    };
    let key = surface(pane);
    let list = col()
        .key(format!("{key}.refs.list"))
        .w_full()
        .max_h(LIST_MAX)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(col().w_full().children(body));
    let said = |note: &str, ink| {
        let note = text(note).font(META).color(colour(ink)).wrap();
        row().w_full().px(tokens.gutter).py(2.0).child(note)
    };
    let note = asked.note.as_deref().map(|note| said(note, tokens.warn));
    let cut = hub.model.tables.get(LSP).and_then(cut);
    let cut = cut.filter(|_| found).map(|cut| {
        let key = format!("{}.refs.cut", surface(pane));
        said(cut, tokens.faint).key(key)
    });
    col()
        .key(format!("{key}.refs"))
        .w_full()
        .pb(3.0)
        .fill(colour(WELL.of(tokens)))
        .border(1.0, colour(tokens.context.alpha(0.3)))
        .focusable(true)
        .on_key(move |win: &mut Win, event: &KeyEvent, tx: &mut Tx| keys(win, event, tx, pane))
        .child(top)
        .children(note)
        .child(list)
        .children(cut)
}

pub const SCENES: &[Scene] = &[Scene {
    name: "references/mockup",
    stage: Stage::View(mockup),
    setup,
}];

/// The sample's `take` rows as the language source sends them: columns in characters from 1,
/// every kind `read` but the definition, which the sample lists first.
pub fn sample() -> Vec<Row> {
    let rows = rows_from_json(include_str!("sample/refs.rows.json")).unwrap_or_default();
    let name = rows.first().and_then(|row| row.words("name")).unwrap_or("");
    let name = name.to_owned();
    let made = rows.iter().enumerate().filter_map(|(n, row)| {
        let line = row.words("text")?;
        let col = line.get(..line.find(&name)?)?.chars().count() + 1;
        let kind = if n == 0 { "definition" } else { "read" };
        let id = format!("ref:src/middleware/rate_limit.rs:13:12:{n}");
        let new = Row::new(id).text("path", row.group.clone().unwrap_or_default());
        let end = (col + name.chars().count()).to_string();
        Some(
            new.text("line", row.words("line")?)
                .text("col", col.to_string())
                .text("end_col", end)
                .text("text", line)
                .text("kind", kind),
        )
    });
    made.collect()
}

/// Frame 8's right pane: the code viewer's mockup file with `take` clicked on line 13 and
/// its references answered, from the same sample the mockup draws.
fn setup(hub: &mut Hub) {
    if let Some(file) = DEF.scenes.iter().find(|scene| scene.name == "code/mockup") {
        (file.setup)(hub);
    }
    begin(hub, SCENE_PANE, LineNo::from_index(12), Cols::new(11, 15));
    step(&mut hub.model, Msg::Feed(LSP, FeedEvent::Reset(sample())));
}

/// The pane under its tab bar, with the mockup's "Find references" header laid over the
/// breadcrumb's place: that frame has a header bar where the viewer has a path.
fn mockup(win: &Win, hub: &Hub, cx: &mut Cx<Win>) -> El {
    let tokens = &hub.model.tokens;
    let key = format!("{}.pane-header", surface(SCENE_PANE));
    let head = row()
        .key(key)
        .absolute(0.0, 21.0)
        .w_full()
        .h(tokens.bar)
        .px(6.0)
        .gap(6.0)
        .center_y()
        .fill(colour(tokens.window))
        .border_sides([0.0, 0.0, 1.0, 0.0], colour(white(tokens.inner_lines)))
        .child(glyph(&SEARCH, 10.0, tokens.dim))
        .child(text("Find references").font(BOLD).color(colour(tokens.fg)));
    col()
        .w_full()
        .h_full()
        .child(alone(win, hub, cx, PaneKind::Code))
        .child(head)
}

#[cfg(test)]
mod tests;
