//! Agents pane: parent and child agents as a tree read like a table.

use std::borrow::Cow;

use rune::{Cx, El, Tx, col, el, text};
use yd_core::address::thing::Thing;
use yd_core::command::table::{Route, Run, Target};
use yd_core::feed::Table;
use yd_core::ids::{CommandId, FeedId, PaneId, ThreadId};
use yd_core::kind::Kind;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_core::reduce::step::commands::{ALLOW, CHILD_STOP, child_address};
use yd_core::row::{Rows, Tone};
use yd_core::tokens::Tokens;
use yd_ui::chrome::tabs::{Tab, Tabs};
use yd_ui::kit::bars::{footer, header, lead};
use yd_ui::kit::meter::meter;
use yd_ui::kit::status::badge;
use yd_ui::list::row;
use yd_ui::list::view::worded;
use yd_ui::overlay::pick::Finder;
use yd_ui::pane::{PaneCx, PaneDef};
use yd_ui::theme::look::{LABEL, META, colour};
use yd_ui::win::{Hub, Win, dispatch};

use super::desc::{COMMANDS, DESC, OPEN_THREAD, RUN_COMMAND, STOP};
use fold::{DASH, Summary, count, summarise};

use super::states::WORDS;
use crate::copy::empty::{SERVER, down, threadless};

mod fold;
mod scenes;
#[cfg(test)]
mod tests;

/// The feed a source or a scene may fill with rows; while it holds none, the rows are the
/// shown thread's child agents.
pub const FEED: FeedId = FeedId::new("agents");

/// What the server has not said yet and a scene can: the thread's cost budget and how many
/// agents it may run at once. Unset, the header and footer show a dash.
#[derive(Debug, Default, Clone, Copy)]
pub struct Limits {
    pub cost: Option<f64>,
    pub slots: Option<usize>,
}

pub const DEF: PaneDef = PaneDef {
    kind: PaneKind::Agents,
    view,
    scenes: scenes::SCENES,
    commands: COMMANDS,
    settings: &[],
    tabs: Some(tabs),
    on_tab: None,
    finder: Some(Finder::Keyed(thing_at)),
};

/// The rows to draw: what the feed holds, else the shown thread's agents folded.
fn rows<'a>(hub: &'a Hub, thread: Option<&'a yd_core::reduce::threads::Thread>) -> Cow<'a, Table> {
    if let Some(fed) = hub.model.tables.get(FEED).filter(|table| !table.is_empty()) {
        return Cow::Borrowed(fed);
    }
    let folded = thread.map(|thread| fold::rows(&hub.model, thread));
    Cow::Owned(fold::table(folded.unwrap_or_default()))
}

/// Whether the pane has more to show than the session's own agent: a child, or fed rows.
pub fn has(hub: &Hub, thread: Option<&yd_core::reduce::threads::Thread>) -> bool {
    let fed = hub.model.tables.get(FEED);
    fed.is_some_and(|table| !table.is_empty()) || thread.is_some_and(|t| !t.agents.is_empty())
}

fn thing_at(hub: &Hub, pane: PaneId, key: &str) -> Option<Thing> {
    let id = row::id_of(key)?;
    let thread = yd_ui::shown::of_pane(hub, pane);
    let table = rows(hub, thread);
    row::thing(&DESC, table.find(&id)?).ok()
}

/// What a button of an opened agent comes to: the ask to allow, or the session to show.
enum Does {
    Allow(Target),
    Show(ThreadId),
    Stop(Target),
}

/// Carries out "Run its command", "Open thread" and "Stop" on an agent of the thread this
/// window shows; false for the rest. Pause and more budget need calls the server lacks.
pub fn carry(win: &mut Win, _tx: &mut Tx, command: CommandId, target: &Target) -> bool {
    let Target::Thing(address) = target else {
        return false;
    };
    if address.kind() != Kind::Agent || ![RUN_COMMAND, OPEN_THREAD, STOP].contains(&command) {
        return false;
    }
    let does = {
        let Ok(hub) = win.hub.try_borrow() else {
            return false;
        };
        let thread = yd_ui::shown::thread(&hub, win.window);
        let agents = thread.into_iter().flat_map(|thread| thread.agents.iter());
        let Some(agent) = agents
            .into_iter()
            .find(|agent| agent.id.as_str() == address.id())
        else {
            return false;
        };
        if command == STOP {
            let stop = thread.and_then(|thread| child_address(&thread.id, &agent.id).ok());
            stop.map(|stop| Does::Stop(Target::Thing(stop)))
        } else if command == OPEN_THREAD {
            let session = agent.update.session.as_deref();
            session.map(|session| Does::Show(ThreadId::new(session)))
        } else {
            let gates = |call: &yd_core::reduce::turns::CallId| {
                agent.transcript.tools.contains_key(call.as_str())
            };
            let mut waiting = hub.model.wire.waiting();
            let ask = waiting.find(|ask| ask.tool.as_ref().is_some_and(gates));
            let ask = ask.and_then(|ask| ask.address().ok());
            ask.map(|ask| Does::Allow(Target::Thing(ask)))
        }
    };
    match does {
        Some(Does::Allow(ask)) => dispatch(win, Msg::Command(Route::Wire, Run::new(ALLOW).on(ask))),
        Some(Does::Show(session)) => yd_ui::shown::show(win, session),
        Some(Does::Stop(child)) => dispatch(
            win,
            Msg::Command(Route::Wire, Run::new(CHILD_STOP).on(child)),
        ),
        None => return false,
    }
    true
}

fn tabs(pane: &PaneCx<'_>) -> Tabs {
    let table = rows(pane.hub, pane.thread());
    let sum = summarise(&table);
    let tab = Tab {
        count: u32::try_from(sum.rows).ok().filter(|rows| *rows > 0),
        meta: if sum.rows > 0 {
            format!("depth {}", sum.depth)
        } else {
            String::new()
        },
        ..Tab::of(PaneKind::Agents)
    };
    Tabs {
        tabs: vec![tab],
        ..Tabs::default()
    }
}

/// `2 working · 2 need you · 2 done`: the states there are, in that order.
fn states(sum: &Summary) -> String {
    let counts = [
        (sum.working, "working"),
        (sum.need, "need you"),
        (sum.done, "done"),
        (sum.failed, "failed"),
    ];
    let said: Vec<String> = counts
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, what)| format!("{n} {what}"))
        .collect();
    said.join(" · ")
}

fn headline(pane: &PaneCx<'_>, key: &str, sum: &Summary) -> El {
    let tokens = pane.tokens();
    let title = pane.thread().and_then(|thread| thread.title.as_deref());
    let limit = pane.state::<Limits>().and_then(|limits| limits.cost);
    let spent = format!("${:.2}", sum.cost);
    let (reading, ratio) = match limit {
        Some(limit) if limit > 0.0 => (
            format!("{spent} of ${limit:.2}"),
            Some((sum.cost / limit) as f32),
        ),
        Some(_) | None => (format!("{spent} of {DASH}"), None),
    };
    let right = ratio
        .map(|ratio| meter(tokens, &format!("{key}.cost-meter"), ratio))
        .into_iter()
        .chain([text(reading).font(LABEL).color(colour(tokens.dim))])
        .collect();
    let left = vec![
        lead(tokens, title.unwrap_or(DASH)),
        text(states(sum)).font(LABEL).color(colour(tokens.dim)),
    ];
    header(tokens, left, right).key(format!("{key}.pane-header"))
}

/// The models a row's letter stands for (record 5.3).
const MODELS: [&str; 3] = ["Opus", "Sonnet", "Haiku"];
/// A letter's badge, and the room from it to its name and on to the next letter.
const LETTER: (f32, f32, f32) = (12.0, 4.0, 8.0);

/// Each model's letter, then its name. Invariant: the three names come first in the tree
/// and the letters after them, so the mockup's one string of names is read in one piece.
fn legend(tokens: &Tokens) -> El {
    let (badge_w, gap, apart) = LETTER;
    let mut at = 0.0;
    let mut names = Vec::new();
    let mut letters = Vec::new();
    for name in MODELS {
        let letter: String = name.chars().take(1).collect();
        letters.push(
            badge(tokens, &letter, Tone::Model)
                .min_w(badge_w)
                .w(badge_w)
                .px(0.0)
                .absolute(at, 0.0),
        );
        let wide = rune::text::advance(name, META);
        names.push(
            text(name)
                .font(META)
                .color(colour(tokens.faint))
                .absolute(at + badge_w + gap, 0.0),
        );
        at += badge_w + gap + wide + apart;
    }
    el().w(at).h(13.0).children(names).children(letters)
}

fn base(pane: &PaneCx<'_>, key: &str, sum: &Summary) -> El {
    let tokens = pane.tokens();
    let slots = pane.state::<Limits>().and_then(|limits| limits.slots);
    let slots = slots.map_or_else(|| DASH.to_owned(), |slots| slots.to_string());
    let left = if sum.rows == 0 {
        String::new()
    } else {
        format!(
            "{} tokens · {} of {slots} agent slots · depth {}",
            count(sum.tokens),
            sum.slots,
            sum.depth
        )
    };
    let counts = text(left).font(META).color(colour(tokens.faint));
    let legend = (sum.rows > 0).then(|| legend(tokens));
    footer(tokens, legend.into_iter().collect(), vec![counts]).key(format!("{key}.footer"))
}

fn view(pane: &PaneCx<'_>, _cx: &mut Cx<Win>) -> El {
    let key = pane.key(PaneKind::Agents);
    let table = rows(pane.hub, pane.thread());
    let sum = summarise(&table);
    let tint = pane.tokens().pane_tint.alpha(pane.tokens().glass_pane);
    let lists = pane.state::<yd_ui::list::state::Lists>();
    let open = lists.and_then(|lists| lists.list("")?.open());
    let opens = pane.thread().and_then(|thread| {
        crate::chat::pane::child::opened(&key, thread, open.map(|row| row.as_str()))
    });
    col()
        .key(key.clone())
        .w_full()
        .h_full()
        .fill(colour(tint))
        .children(opens)
        .child(headline(pane, &key, &sum))
        .children(down(pane).map(|down| yd_ui::states::status::bar(pane, &key, &down, &SERVER)))
        .child(match threadless(pane) {
            Some((status, words)) => yd_ui::states::status::view(pane, &key, &status, words),
            None => worded(
                pane,
                PaneKind::Agents,
                "",
                &DESC,
                Some(&*table),
                &WORDS,
                false,
            ),
        })
        .child(base(pane, &key, &sum))
}
