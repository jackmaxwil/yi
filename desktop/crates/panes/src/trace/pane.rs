//! Trace pane: one turn as a waterfall of steps.

mod scenes;
#[cfg(test)]
mod tests;

use std::borrow::Cow;

use rune::{Align, Cx, El, TextCase, TextStyle, Tx, col, el, row, spacer, text};
use yd_core::address::attach::ATTACH as ATTACH_THING;
use yd_core::command::table::{Route, Run, Target};
use yd_core::feed::{FeedEvent, Table};
use yd_core::ids::{CommandId, FeedId, PaneId};
use yd_core::kind::Kind;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_core::reduce::step::commands::{ALLOW, DENY as DENY_ASK};
use yd_core::row::{Rows, Tone};
use yd_core::trace::{Header, Trace};
use yd_ui::chrome::tabs::{Tab, Tabs};
use yd_ui::kit::bars::{header, lead};
use yd_ui::list::row::{id_of, thing};
use yd_ui::list::view::worded;
use yd_ui::overlay::pick::Finder;
use yd_ui::pane::{PaneCx, PaneDef, Scene, Stage};
use yd_ui::theme::look::{LABEL, META, colour, tone};
use yd_ui::win::{Hub, Win, dispatch};

use super::desc::{ATTACH, COMMANDS, DENY, DESC, RUN};
use yd_ui::states::status::Status;

use super::states::WORDS;
use crate::copy::empty::threadless;

/// The rows of a turn a scene or a source puts in the model; without any, the pane derives
/// them from the thread its window shows.
pub const FEED: FeedId = FeedId::new("trace");

pub const DEF: PaneDef = PaneDef {
    kind: PaneKind::Trace,
    view,
    scenes: &[
        Scene {
            name: "trace/mockup",
            stage: Stage::Pane(PaneKind::Trace),
            setup: scenes::mockup,
        },
        Scene {
            name: "trace/live",
            stage: Stage::Pane(PaneKind::Trace),
            setup: scenes::live,
        },
    ],
    commands: COMMANDS,
    settings: &[],
    tabs: Some(tabs),
    on_tab: None,
    finder: Some(Finder::Keyed(thing_at)),
};

/// A column title: uppercase and tracked, as every list's are.
const HEADING: TextStyle = TextStyle {
    weight: 500.0,
    tracking: 0.57,
    ..META
};

/// The rows the pane draws and the header read from them: the model's own when something
/// fills the feed, else the last turn of the shown thread.
fn shown(hub: &Hub, pane: PaneId) -> Option<(Cow<'_, Table>, Header)> {
    let fed = hub.model.tables.get(FEED).filter(|table| table.len() > 0);
    let table = match fed {
        Some(table) => Cow::Borrowed(table),
        None => {
            let thread = yd_ui::shown::of_pane(hub, pane)?;
            let mut table = Table::default();
            let rows = Trace::of(thread, None, hub.model.now)?.rows();
            table.apply(FeedEvent::Reset(rows));
            Cow::Owned(table)
        }
    };
    let header = Header::of(table.as_ref())?;
    Some((table, header))
}

fn thing_at(hub: &Hub, pane: PaneId, key: &str) -> Option<yd_core::address::thing::Thing> {
    let (table, _) = shown(hub, pane)?;
    thing(&DESC, table.find(&id_of(key)?)?).ok()
}

/// The ask that gates the waiting step `step`: the one whose tool call the step is named for.
fn ask_of(hub: &Hub, step: &str) -> Option<Target> {
    let call = step.strip_prefix("wait-")?;
    let named = |ask: &&yd_core::reduce::approvals::Ask| {
        let tool = ask.tool.as_ref();
        tool.is_some_and(|tool| tool.as_str().replace('/', ":") == call)
    };
    let ask = hub.model.wire.waiting().find(named)?;
    Some(Target::Thing(ask.address().ok()?))
}

/// Carries out a step's "Run it", "Deny" and "Attach": the two answers go to the ask that
/// gates the step, as the approval card sends them. False for any other command or step.
pub fn carry(win: &mut Win, _tx: &mut Tx, command: CommandId, target: &Target) -> bool {
    let Target::Thing(step) = target else {
        return false;
    };
    if step.kind() != Kind::Span {
        return false;
    }
    if command == ATTACH {
        let attach = Run::new(ATTACH_THING).on(target.clone());
        dispatch(win, Msg::Command(Route::Model, attach));
        return true;
    }
    let answer = match command {
        RUN => ALLOW,
        DENY => DENY_ASK,
        _ => return false,
    };
    let ask = win
        .hub
        .try_borrow()
        .ok()
        .and_then(|hub| ask_of(&hub, step.id()));
    let Some(ask) = ask else { return false };
    dispatch(win, Msg::Command(Route::Wire, Run::new(answer).on(ask)));
    true
}

/// The pane's one tab: how many steps the turn has, and how many ran at once.
fn tabs(pane: &PaneCx<'_>) -> Tabs {
    let shown = shown(pane.hub, pane.pane);
    let (count, meta) = shown.map_or((None, String::new()), |(_, header)| {
        (Some(header.steps), header.meta)
    });
    let tab = Tab {
        count,
        meta,
        ..Tab::of(PaneKind::Trace)
    };
    Tabs {
        tabs: vec![tab],
        ..Tabs::default()
    }
}

/// A 7 px dot in a bar colour, then what it means.
fn legend(pane: &PaneCx<'_>, role: Tone, said: &str) -> El {
    let tokens = pane.tokens();
    let dot = el()
        .size(7.0, 7.0)
        .radius(3.5)
        .fill(colour(tone(tokens, role)));
    row()
        .gap(4.0)
        .center_y()
        .child(dot)
        .child(text(said).font(LABEL).color(colour(tokens.dim)))
}

/// The column titles on the lane's grid, and over the lane the time ruler.
fn ruler(pane: &PaneCx<'_>, key: &str, ticks: &[String; 4]) -> El {
    let tokens = pane.tokens();
    let faint = colour(tokens.faint);
    let title = |said: &str| {
        text(said)
            .font(HEADING)
            .text_case(TextCase::Uppercase)
            .color(faint)
    };
    let mark = |said: &String| text(said.as_str()).font(META.mono()).color(faint);
    let [first, second, third, last] = ticks;
    let lane = row()
        .grow(1.0)
        .min_w(0.0)
        .child(mark(first))
        .child(spacer())
        .child(mark(second))
        .child(spacer())
        .child(mark(third))
        .child(spacer())
        .child(mark(last));
    row()
        .key(format!("{key}.table-header"))
        .w_full()
        .h(tokens.row)
        .px(tokens.gutter)
        .gap(8.0)
        .center_y()
        .child(title("Step").w(150.0))
        .child(title("Start").w(34.0).align(Align::End))
        .child(title("Took").w(44.0).align(Align::End))
        .child(lane)
}

fn view(pane: &PaneCx<'_>, _cx: &mut Cx<Win>) -> El {
    let key = pane.key(PaneKind::Trace);
    let tokens = pane.tokens();
    let tint = if pane.focused {
        tokens.glass_pane_focus
    } else {
        tokens.glass_pane
    };
    let body = match shown(pane.hub, pane.pane) {
        Some((table, head)) => {
            let bar = header(
                tokens,
                vec![
                    lead(tokens, &head.title),
                    text(head.summary.as_str())
                        .font(LABEL)
                        .color(colour(tokens.dim)),
                ],
                vec![
                    legend(pane, Tone::Model, "model"),
                    legend(pane, Tone::Ok, "tool"),
                    legend(pane, Tone::Accent, "child"),
                    legend(pane, Tone::Need, "waiting"),
                ],
            );
            col()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .child(bar.key(format!("{key}.pane-header")))
                .child(ruler(pane, &key, &head.ticks))
                .child(worded(
                    pane,
                    PaneKind::Trace,
                    "",
                    &DESC,
                    Some(&table),
                    &WORDS,
                    false,
                ))
        }
        None => {
            let (status, words) = threadless(pane).unwrap_or((Status::Empty, &WORDS));
            yd_ui::states::status::view(pane, &key, &status, words)
        }
    };
    col()
        .key(key)
        .w_full()
        .h_full()
        .fill(colour(tokens.pane_tint.alpha(tint)))
        .child(body)
}
