//! The side panel: variables, watches, call stack and breakpoints, and the rule in its foot.

use rune::{El, TextCase, TextStyle, col, el, row, text};
use yd_core::address::uri::Address;
use yd_core::command::table::{Route, Run, Target};
use yd_core::command::tools::debug;
use yd_core::feed::{DEBUGGER, Table};
use yd_core::kind::Kind;
use yd_core::list::ListDesc;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_core::row::{Row, Rows, Value};
use yd_ui::list::state::Lists;
use yd_ui::list::view::part;
use yd_ui::pane::PaneCx;
use yd_ui::theme::look::{META, colour, white};
use yd_ui::win::ask;

use crate::debugger::console;
use crate::debugger::desc::{
    BREAKPOINT, BREAKPOINTS, BREAKPOINTS_PART, FRAME, STACK, STACK_PART, VARIABLE, VARIABLES,
    VARIABLES_PART, WATCH, WATCH_PART, WATCHES, breakpoint_row, frame_row, shown, variable_row,
    watch_row,
};

pub const WIDTH: f32 = 252.0;
/// A heading's height, and what an open row's detail adds under it.
const HEADING_H: f32 = 24.0;
const DETAIL_H: f32 = 150.0;
/// The window's name for the field that adds a watch.
pub const WATCH_FIELD: &str = "watch";
/// The rule the pane states (record 6.2): said in the foot of the side panel.
pub const RULE: &str = "The agent can read this state. It cannot step.";
const HEADING: TextStyle = TextStyle {
    weight: 500.0,
    tracking: 0.57,
    ..META
};

struct Section {
    name: &'static str,
    title: &'static str,
    desc: &'static ListDesc,
    prefix: &'static str,
    shape: fn(Row) -> Row,
}

const SECTIONS: [Section; 4] = [
    Section {
        name: VARIABLES_PART,
        title: "Variables",
        desc: &VARIABLES,
        prefix: VARIABLE,
        shape: variable_row,
    },
    Section {
        name: WATCH_PART,
        title: "Watch",
        desc: &WATCHES,
        prefix: WATCH,
        shape: watch_row,
    },
    Section {
        name: STACK_PART,
        title: "Call stack",
        desc: &STACK,
        prefix: FRAME,
        shape: frame_row,
    },
    Section {
        name: BREAKPOINTS_PART,
        title: "Breakpoints",
        desc: &BREAKPOINTS,
        prefix: BREAKPOINT,
        shape: breakpoint_row,
    },
];

fn heading(pane: &PaneCx<'_>, key: &str, words: &str) -> El {
    let tokens = pane.tokens();
    row()
        .key(format!("{key}.heading"))
        .w_full()
        .px(tokens.gutter)
        .pt(8.0)
        .pb(2.0)
        .child(
            text(words)
                .font(HEADING)
                .text_case(TextCase::Uppercase)
                .color(colour(tokens.faint)),
        )
}

fn section(pane: &PaneCx<'_>, key: &str, feed: &Table, at: &Section) -> El {
    let drawn = shown(feed, at.prefix, at.shape);
    let key = format!("{key}.{}", at.name);
    let rows = u16::try_from(drawn.len()).unwrap_or(u16::MAX);
    let open = pane
        .state::<Lists>()
        .and_then(|lists| lists.list(at.name))
        .is_some_and(|list| list.open().is_some());
    let adding = (at.name == WATCH_PART).then(|| {
        let tokens = pane.tokens();
        row()
            .key(format!("{key}.add"))
            .w_full()
            .h(tokens.row)
            .px(tokens.gutter)
            .center_y()
            .child(console::input(
                pane,
                &format!("{key}.add.input"),
                (WATCH_FIELD, debug::WATCH),
                ("Add a watch", "Watch an expression"),
            ))
    });
    // The room the rows want, so spare height stays at the foot of the panel as the mockup has it.
    let wants = HEADING_H + f32::from(rows) * pane.tokens().row;
    let wants = wants + if open { DETAIL_H } else { 0.0 };
    let wants = wants
        + if adding.is_some() {
            pane.tokens().row
        } else {
            0.0
        };
    col()
        .key(format!("{key}.section"))
        .w_full()
        .grow(f32::from(rows.max(1)))
        .min_h(0.0)
        .max_h(wants)
        .child(heading(pane, &key, at.title))
        .child(part(
            pane,
            PaneKind::Debugger,
            at.name,
            at.desc,
            Some(&drawn),
        ))
        .children(adding)
}

/// The variable the reader opened if it can hold more and has been shown none of it.
fn unexpanded(pane: &PaneCx<'_>, feed: &Table) -> Option<Address> {
    let open = pane
        .state::<Lists>()
        .and_then(|lists| lists.list(VARIABLES_PART))
        .and_then(|list| list.open())?;
    let row = feed.find(open)?;
    let shown = (0..feed.len())
        .filter_map(|at| feed.get(at))
        .any(|other| other.parent.as_ref() == Some(open));
    let more = row.get("expandable") == Some(&Value::Flag(true));
    (more && !shown && open.as_str().starts_with(VARIABLE))
        .then(|| Address::new(Kind::Variable, open.as_str()).ok())
        .flatten()
}

pub fn view(pane: &PaneCx<'_>, key: &str) -> El {
    let tokens = pane.tokens();
    let blank = Table::default();
    let feed = pane.model().tables.get(DEBUGGER).unwrap_or(&blank);
    let sections = SECTIONS.iter().map(|at| section(pane, key, feed, at));
    let expand = unexpanded(pane, feed).map(|variable| {
        let run = Run::new(debug::EXPAND).on(Target::Thing(variable.clone()));
        ask(
            format!("{key}.expand.{}", variable.id()),
            Msg::Command(Route::Source(DEBUGGER), run),
        )
    });
    let dot = el().size(7.0, 7.0).radius(3.5).fill(colour(tokens.accent));
    let rule = text(RULE).font(META).color(colour(tokens.faint)).truncate();
    let rule = row()
        .key(format!("{key}.footer"))
        .w_full()
        .h(tokens.row)
        .px(8.0)
        .gap(6.0)
        .center_y()
        .border_sides([1.0, 0.0, 0.0, 0.0], colour(white(tokens.inner_lines)))
        .child(dot)
        .child(rule);
    col()
        .key(format!("{key}.side"))
        .w(WIDTH)
        .h_full()
        .border_sides([0.0, 0.0, 0.0, 1.0], colour(white(tokens.inner_lines)))
        .clip_rounded(0.0)
        .child(
            col()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .children(sections)
                .children(expand),
        )
        .child(rule)
}
