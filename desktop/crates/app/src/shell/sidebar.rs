//! The sidebar: the thread tree grouped by project. It is open or hidden, never folded.

use rune::{Cursor, Cx, Drag, El, Icon, Phase, ScrollSpec, Tx, col, el, row, text};
use yd_core::command::table::OPEN_FOLDER;
use yd_core::ids::{CommandId, Millis};
use yd_core::kind::Kind;
use yd_core::reduce::state::Wire;
use yd_core::reduce::step::commands::NEW;
use yd_core::reduce::threads::Thread;
use yd_core::row::Dot;
use yd_core::tokens::{Rgba, Tokens};
use yd_ui::a11y;
use yd_ui::kit::icons::glyph;
use yd_ui::kit::status::dot_colour;
use yd_ui::line_icon;
use yd_ui::menu::command_box;
use yd_ui::theme::look::{BODY, LABEL, META, colour};
use yd_ui::theme::scale::{
    DOT_HOLLOW, HOVER_QUIET, LINE_BAR, LINE_BRANCH, LINE_PICKED, RADIUS_CHIP,
};
use yd_ui::win::{Hub, Win};

use crate::shell::view::{self, Shell, base, change, current, dot, project, roots};

/// The open tree's width (record 2.4), which a double-click on its edge gives back.
pub const TREE_W: f32 = 196.0;
/// The narrowest a drag leaves it: a row still shows its dot, a short name and its age.
pub const MIN_W: f32 = 160.0;
const ROW_H: f32 = 20.0;
/// How far in from the sidebar's edge a press takes the edge; the grid owns what is past it.
const REACH: f32 = 5.0;

static PLUS: Icon = line_icon!("plus", r#"<path d="M12 5v14M5 12h14"/>"#);
static FOLDER: Icon = line_icon!(
    "folder",
    r#"<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>"#
);
/// Tree connectors, drawn in a box as wide as the gutter and cut at the row's height.
static STUB: Icon = Icon {
    name: "tree-stub",
    view: 22.0,
    body: r#"<path d="M10 16V22" fill="none" stroke="currentColor" stroke-width="1.2"/>"#,
};
static MID: Icon = Icon {
    name: "tree-mid",
    view: 34.0,
    body: r#"<path d="M10 0V22M10 0V7q0 4 4 4h7" fill="none" stroke="currentColor" stroke-width="1.2"/>"#,
};
static LAST: Icon = Icon {
    name: "tree-last",
    view: 34.0,
    body: r#"<path d="M10 0V7q0 4 4 4h7" fill="none" stroke="currentColor" stroke-width="1.2"/>"#,
};

/// Where a row sits in the tree, which picks its gutter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    Root,
    /// A root with children under it.
    Parent,
    /// A child with siblings after it.
    Mid,
    Last,
}

/// The threads under `parent`, oldest activity first.
fn children<'a>(wire: &'a Wire, parent: &Thread) -> Vec<&'a Thread> {
    let mut under: Vec<&Thread> = wire
        .threads
        .values()
        .filter(|thread| thread.forked_from.as_ref() == Some(&parent.id))
        .collect();
    under.sort_by_key(|thread| thread.last_activity);
    under
}

fn ink(word: &str, tokens: &Tokens) -> Rgba {
    if word == "now" {
        tokens.need
    } else {
        tokens.faint
    }
}

fn title(thread: &Thread) -> &str {
    thread.title.as_deref().unwrap_or(thread.id.as_str())
}

fn row_key(form: &str, thread: &Thread) -> String {
    ["shell.", form, ".", thread.id.as_str(), ".sidebar-row"].concat()
}

/// A thread row: picking it shows the thread in this window.
fn line(tokens: &Tokens, key: String, picked: bool, thread: &Thread) -> El {
    let id = thread.id.clone();
    let base = a11y::item(row(), tokens, picked)
        .key(key)
        .h(ROW_H)
        .w_full()
        .center_y()
        .radius(RADIUS_CHIP.of(tokens))
        .cursor(Cursor::Pointer)
        .hover(colour(HOVER_QUIET.of(tokens)))
        .on_press(move |win: &mut Win| view::select(win, id.clone()));
    if picked {
        base.fill(colour(tokens.selected))
            .border_sides([0.0, 0.0, 0.0, 2.0], colour(LINE_PICKED.of(tokens)))
    } else {
        base
    }
}

/// How long ago `then` was; nothing for a thread whose list row gave no time at all.
fn age_text(tokens: &Tokens, now: Millis, then: Millis) -> Option<El> {
    let words = (then != Millis(0)).then(|| now.age(then))?;
    let ink = ink(&words, tokens);
    let age = text(words).font(META.features("'tnum' 1"));
    Some(age.color(colour(ink)))
}

/// The gutter: connector lines, and the dot filled for need and done, a ring otherwise.
fn gutter(tokens: &Tokens, place: Place, state: Dot) -> El {
    let child = matches!(place, Place::Mid | Place::Last);
    let (width, centre, radius) = if child {
        (34.0, 25.0, 2.8)
    } else {
        (22.0, 10.0, 3.6)
    };
    let ink = dot_colour(tokens, state);
    let filled = matches!(state, Dot::Need | Dot::Done);
    let fill = if filled { ink } else { DOT_HOLLOW.of(tokens) };
    let mark = el()
        .absolute(centre - radius, ROW_H / 2.0 - radius)
        .size(radius * 2.0, radius * 2.0)
        .radius(radius)
        .fill(colour(fill))
        .border(1.5, colour(ink));
    let lines = match place {
        Place::Root => None,
        Place::Parent => Some(&STUB),
        Place::Mid => Some(&MID),
        Place::Last => Some(&LAST),
    };
    let lines =
        lines.map(|drawing| glyph(drawing, width, LINE_BRANCH.of(tokens)).absolute(0.0, 0.0));
    el().w(width)
        .h(ROW_H)
        .clip_rounded(0.0)
        .children(lines)
        .child(mark)
}

fn tree_row(tokens: &Tokens, wire: &Wire, now: Millis, picked: bool, at: (&Thread, Place)) -> El {
    let (thread, place) = at;
    let ink = if picked { tokens.fg } else { tokens.dim };
    line(tokens, row_key("thread", thread), picked, thread)
        .pr(5.0)
        .child(gutter(tokens, place, dot(wire, thread)))
        .child(
            text(title(thread))
                .font(BODY)
                .color(colour(ink))
                .truncate()
                .grow(1.0)
                .min_w(0.0),
        )
        .children(age_text(tokens, now, thread.last_activity).map(|age| age.ml(5.0)))
}

/// A project's label row, with its root count and the branch its repository is based on.
fn label(tokens: &Tokens, name: &str, count: usize, base: Option<&str>) -> El {
    let noun = if count == 1 { "thread" } else { "threads" };
    let summary = match base {
        Some(base) => format!("{count} {noun} · {base}"),
        None => format!("{count} {noun}"),
    };
    row()
        .key(["shell.group.", name, ".sidebar-label"].concat())
        .h(ROW_H)
        .w_full()
        .pt(4.0)
        .px(5.0)
        .pb(1.0)
        .center_y()
        .child(
            text(name)
                .font(LABEL.tracking(0.21))
                .color(colour(tokens.faint))
                .truncate()
                .grow(1.0)
                .min_w(0.0),
        )
        .child(text(summary).font(META).color(colour(tokens.faint)))
}

fn new_row(tokens: &Tokens, place: Option<String>) -> El {
    row()
        .key("shell.new")
        .h(ROW_H)
        .w_full()
        .px(5.0)
        .gap(5.0)
        .center_y()
        .radius(RADIUS_CHIP.of(tokens))
        .cursor(Cursor::Pointer)
        .hover(colour(HOVER_QUIET.of(tokens)))
        .on_press_tx(move |win: &mut Win, tx: &mut Tx| {
            let target = place
                .as_deref()
                .and_then(|cwd| view::thing(Kind::Place, cwd));
            view::run(win, tx, NEW, target.unwrap_or_default());
        })
        .child(glyph(&PLUS, 10.0, tokens.faint))
        .child(text("New").font(LABEL).color(colour(tokens.dim)))
}

/// The widest a drag leaves it in a window `across` points wide: a third of the window.
fn widest(across: f32) -> f32 {
    (across / 3.0).max(MIN_W)
}

/// How wide the sidebar draws: the drag under way, else the width kept, within its bounds.
pub fn width(shell: &Shell, across: f32) -> f32 {
    let wanted = shell.held.or(shell.width).unwrap_or(TREE_W);
    wanted.clamp(MIN_W, widest(across))
}

/// The edge's drag: the width follows the pointer, a release keeps it, Escape puts it back,
/// and a double-click gives the default again.
fn resize(win: &mut Win, drag: &mut Drag, from: f32, across: f32) {
    let window = win.window;
    let Ok(mut hub) = win.hub.try_borrow_mut() else {
        return;
    };
    change(&mut hub, window, |shell| match drag.phase {
        Phase::Down if drag.clicks >= 2 => (shell.width, shell.held) = (None, None),
        Phase::Down => {
            drag.data = from;
            shell.held = Some(from);
        }
        Phase::Move if shell.held.is_some() => {
            let to = (drag.data + drag.dx).clamp(MIN_W, widest(across));
            shell.held = Some(to);
        }
        Phase::Move => {}
        Phase::Up => {
            let held = shell.held.take();
            if !drag.cancelled() {
                shell.width = held.or(shell.width);
            }
        }
    });
}

/// A strip along the inside of the sidebar's right edge that takes a drag.
fn edge(from: f32, across: f32) -> El {
    el().key("shell.sidebar.edge")
        .pin(None, Some(0.0), Some(0.0), Some(0.0))
        .w(REACH)
        .cursor(Cursor::ResizeColumn)
        .on_drag(move |win: &mut Win, drag: &mut Drag, _: &mut Tx| {
            resize(win, drag, from, across);
        })
}

/// A button heading the sidebar: its icon and name, and its key as the map holds it on hover.
fn head(hub: &Hub, part: &str, icon: &'static Icon, said: &str, command: CommandId) -> El {
    let tokens = &hub.model.tokens;
    let key = command_box::keymap(hub).hint(command);
    let help = key.map_or_else(|| said.to_owned(), |key| format!("{said} {key}"));
    row()
        .key(["shell.sidebar.", part].concat())
        .label(said)
        .help(help)
        .h(ROW_H)
        .w_full()
        .px(5.0)
        .gap(6.0)
        .center_y()
        .radius(RADIUS_CHIP.of(tokens))
        .cursor(Cursor::Pointer)
        .hover(colour(HOVER_QUIET.of(tokens)))
        .on_press_tx(move |win: &mut Win, tx: &mut Tx| {
            let place = win.hub.try_borrow().ok().and_then(|hub| {
                let cwd = current(&hub, win.window)?.cwd.clone();
                view::thing(Kind::Place, &cwd)
            });
            view::run(win, tx, command, place.unwrap_or_default());
        })
        .child(glyph(icon, 11.0, tokens.dim))
        .child(text(said).font(BODY).color(colour(tokens.dim)))
}

fn tree(win: &Win, hub: &Hub) -> Vec<El> {
    let tokens = &hub.model.tokens;
    let wire = &hub.model.wire;
    let picked = current(hub, win.window).map(|thread| thread.id.clone());
    let now = hub.model.now;
    let roots = roots(wire);
    let mut projects: Vec<&str> = Vec::new();
    let chosen = yd_ui::shown::chosen(hub, win.window).map(project);
    for name in roots
        .iter()
        .map(|thread| project(&thread.cwd))
        .chain(chosen)
    {
        if !projects.contains(&name) {
            projects.push(name);
        }
    }
    let mut out = Vec::new();
    for name in projects {
        let members: Vec<&&Thread> = roots
            .iter()
            .filter(|thread| project(&thread.cwd) == name)
            .collect();
        let base = base(&hub.model, name);
        out.push(label(tokens, name, members.len(), base));
        for root in members {
            let under = children(wire, root);
            let place = if under.is_empty() {
                Place::Root
            } else {
                Place::Parent
            };
            let chosen = |thread: &Thread| picked.as_ref() == Some(&thread.id);
            out.push(tree_row(tokens, wire, now, chosen(root), (root, place)));
            let last = under.len().saturating_sub(1);
            for (index, child) in under.into_iter().enumerate() {
                let place = if index == last {
                    Place::Last
                } else {
                    Place::Mid
                };
                out.push(tree_row(tokens, wire, now, chosen(child), (child, place)));
            }
        }
    }
    out
}

/// The tree, `top` points below the window's top: room for the traffic lights over a page
/// that draws its own title row.
pub fn view(win: &Win, hub: &Hub, cx: &mut Cx<Win>, top: f32) -> El {
    let tokens = &hub.model.tokens;
    let across = cx.layout_size("shell").map_or(f32::MAX, |[w, _]| w);
    let wide = width(&view::shell(hub, win.window), across);
    let heads = col()
        .w_full()
        .px(2.0)
        .pt(4.0)
        .pb(3.0)
        .child(head(hub, "new-chat", &PLUS, "New chat", NEW))
        .child(head(
            hub,
            "new-project",
            &FOLDER,
            "New project",
            OPEN_FOLDER,
        ));
    let place = current(hub, win.window).map(|thread| thread.cwd.clone());
    let list = a11y::list(col(), "Threads")
        .w_full()
        .px(2.0)
        .py(1.0)
        .children(tree(win, hub))
        .child(new_row(tokens, place));
    let scroll = col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(list);
    col()
        .key("shell.sidebar")
        .w(wide)
        .pt(top)
        .h_full()
        .border_sides([0.0, 1.0, 0.0, 0.0], colour(LINE_BAR.of(tokens)))
        .child(heads)
        .child(scroll)
        .child(edge(wide, across))
}
