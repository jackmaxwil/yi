//! Chat pane: the conversation, the approval card and the composer.

pub mod child;
pub mod environment;
mod finder;
mod scenes;
#[cfg(test)]
mod tests;

use std::cell::RefCell;

use rune::{Cx, El, GrowthSpec, ScrollSpec, Tx, col, el, row, spacer};
use yd_core::ids::{PaneId, ThreadId};
use yd_core::pane::PaneKind;
use yd_core::reduce::threads::{Run as State, Thread};
use yd_core::reduce::turns::CallId;
use yd_core::tokens::Tokens;
use yd_ui::chrome::tabs::{Tab, Tabs};
use yd_ui::grid_view::view::alone;
use yd_ui::kit::chip::{Chip, chip};
use yd_ui::overlay::pick::Finder;
use yd_ui::pane::{PaneCx, PaneDef, Scene, Stage};
use yd_ui::theme::look::{colour, stamp};
use yd_ui::win::{Hub, Win};

use crate::chat::approval::{Card, queue};
use crate::chat::composer::{Composer, Form, Send, drawn};
use crate::chat::messages::{Frame, count};
use crate::chat_doc::view::WIDTH;
use yd_ui::states::status::{Status, bar, view as state};

use crate::copy::empty::{SERVER, THREADLESS, down, threadless};
use crate::copy::labels;

pub use environment::carry;

/// What one chat pane remembers: the ask it has up and where it is read. The thread it
/// shows is its window's (`yd_ui::shown`), which also opens and closes it on the wire.
#[derive(Debug, Default)]
pub struct Chat {
    /// Which waiting ask of the thread the card shows, counted from the oldest.
    pub pick: usize,
    /// The reader scrolled up, so the conversation does not follow its tail.
    pub away: bool,
    /// Earlier turns opened in place, by number.
    pub open: Vec<u32>,
    /// Steps opened in place to show their whole command and what it printed.
    pub steps: Vec<CallId>,
    /// The thread whose waiting card last had the keyboard handed to it; a layout that moves
    /// the pane does not hand it again.
    pub held: Option<ThreadId>,
}

pub const DEF: PaneDef = PaneDef {
    kind: PaneKind::Chat,
    view,
    scenes: &[
        Scene {
            name: "chat/mockup",
            stage: Stage::View(framed),
            setup: scenes::mockup,
        },
        Scene {
            name: "chat/streaming",
            stage: Stage::View(framed),
            setup: scenes::streaming,
        },
        Scene {
            name: "chat/approval-allowed",
            stage: Stage::View(framed),
            setup: scenes::allowed,
        },
        Scene {
            name: "chat/tools-open",
            stage: Stage::Pane(PaneKind::Chat),
            setup: scenes::tools,
        },
        Scene {
            name: "chat/approval-queue",
            stage: Stage::View(framed),
            setup: scenes::queue,
        },
        Scene {
            name: "chat/environment",
            stage: Stage::Pane(PaneKind::Chat),
            setup: environment::scene,
        },
        Scene {
            name: "chat/empty",
            stage: Stage::Pane(PaneKind::Chat),
            setup: scenes::empty,
        },
        Scene {
            name: "context-tray/mockup",
            stage: Stage::View(with_tray),
            setup: |hub| {
                scenes::mockup(hub);
                yd_ui::overlay::boxes::attach_mockup(hub);
            },
        },
    ],
    commands: &[child::COMMAND, environment::COMMAND],
    settings: &[environment::ROW],
    tabs: Some(tab),
    on_tab: None,
    finder: Some(Finder::Keyed(finder::thing_at)),
};

/// The thread pane `pane` shows: the one its window shows.
pub(crate) fn shown(hub: &Hub, pane: PaneId) -> Option<&Thread> {
    yd_ui::shown::of_pane(hub, pane)
}

/// The pane's one tab, with the shown thread's turn, context and cost as its meta; a child's
/// pane names the child and says its state.
fn tab(pane: &PaneCx<'_>) -> Tabs {
    let mut tab = Tab {
        meta: pane.thread().map(meta).unwrap_or_default(),
        ..Tab::of(PaneKind::Chat)
    };
    if let Some(bound) = child::bound(pane.hub, pane.pane) {
        let thread = pane.model().wire.threads.get(&bound.thread);
        (tab.title, tab.meta) = thread.map(|t| child::tab(t, &bound)).unwrap_or_default();
    }
    Tabs {
        tabs: vec![tab],
        tool: Some(environment::tool(pane)),
        ..Tabs::default()
    }
}

/// The pane's meta text for its tab bar: turn, context used, cost (record 3.4.2).
pub fn meta(thread: &Thread) -> String {
    let turns = thread.transcript.turns().len();
    if thread.unreported() {
        return format!("turn {turns} · no usage reported");
    }
    let used = count(thread.usage.used.0);
    format!("turn {turns} · {used} · ${:.2}", thread.cost())
}

pub(crate) fn chat_mut(win: &mut Win, pane: PaneId, change: impl FnOnce(&mut Chat)) {
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(chat) = hub.panes.get_mut::<Chat>(Some(pane))
    {
        change(chat);
    }
}

/// The way back to the newest line once the reader scrolled away from it: a chip that
/// scrolls `place` to its end and follows it again.
pub(crate) fn jump(
    tokens: &Tokens,
    key: &str,
    id: PaneId,
    place: String,
    away: bool,
) -> Option<El> {
    away.then(|| {
        let chip = chip(tokens, Chip::Plain, labels::JUMP_TO_LATEST, None, false)
            .key(format!("{key}.jump"))
            .on_press_tx(move |win: &mut Win, tx: &mut Tx| {
                tx.scroll_to_bottom(place.clone());
                chat_mut(win, id, |chat| chat.away = false);
            });
        row().w_full().justify_center().child(chip)
    })
}

fn view(pane: &PaneCx<'_>, cx: &mut Cx<Win>) -> El {
    let key = pane.key(PaneKind::Chat);
    let tokens = pane.tokens();
    let model = pane.model();
    let id = pane.pane;
    let bound = child::bound(pane.hub, id);
    let bound = bound.as_ref();
    let thread = match bound {
        Some(bound) => model.wire.threads.get(&bound.thread),
        None => shown(pane.hub, id),
    };
    let agent = bound
        .zip(thread)
        .and_then(|(bound, t)| child::agent(t, bound));
    let chat = pane.state::<Chat>();
    let running = match (bound, agent) {
        (Some(_), agent) => agent.is_some_and(|agent| agent.ended().is_none()),
        (None, _) => thread.is_some_and(|t| matches!(t.state, State::Running | State::NeedsYou)),
    };
    let (mut waiting, pick) = match thread {
        Some(thread) => queue(&model.wire.asks, thread, chat.map_or(0, |chat| chat.pick)),
        None => (Vec::new(), 0),
    };
    if bound.is_some() {
        waiting.retain(|ask| agent.is_some_and(|agent| child::asked(agent, ask)));
    }
    let pick = pick.min(waiting.len().saturating_sub(1));
    let queued: Vec<&str> = thread
        .map(|t| model.wire.queued(&t.id).collect())
        .unwrap_or_default();
    let opened = chat.map(|chat| chat.open.as_slice()).unwrap_or_default();
    let transcript = match (bound, thread) {
        (Some(_), _) => agent
            .filter(|agent| !agent.gone)
            .map(|agent| &agent.transcript),
        (None, thread) => thread.map(|thread| &thread.transcript),
    };
    let steps = chat.map(|chat| chat.steps.as_slice()).unwrap_or_default();
    let items = match (thread, transcript) {
        (Some(thread), Some(transcript)) => Frame {
            key: &key,
            pane: id,
            opened,
            folder: yd_ui::shown::works_in(pane.hub, thread),
            steps,
            queued: &queued,
            tokens,
            thread,
            transcript,
            now: model.now,
            held: !waiting.is_empty(),
            moving: running && !cx.is_reduced_motion(),
            stamp: stamp(tokens),
            names: RefCell::default(),
        }
        .conversation(),
        (Some(_), None) => vec![child::missing(&key, tokens, agent)],
        (None, _) => {
            let (status, words) = threadless(pane).unwrap_or((Status::Empty, &THREADLESS));
            vec![state(pane, &key, &status, words).py(24.0)]
        }
    };
    let place = format!("{key}.conversation");
    let follow = place.clone();
    let conversation = col()
        .key(place.clone())
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        // Invariant: while a step is open the pane does not follow its tail, so it opens down.
        .scroll(ScrollSpec {
            stick_bottom: chat.is_none_or(|chat| chat.steps.is_empty()),
        })
        .on_at_bottom(move |win: &mut Win, at_tail| chat_mut(win, id, |chat| chat.away = !at_tail))
        // The framework's timeline: a row that grows glides, and the pinned tail follows it.
        .child(
            reading(col())
                .key(format!("{key}.timeline"))
                .py(6.0)
                .gap(8.0)
                .growth(GrowthSpec::default())
                .children(items),
        );
    let jump = jump(tokens, &key, id, follow, chat.is_some_and(|chat| chat.away));
    let card = thread.and_then(|thread| {
        Card {
            key: &key,
            pane: id,
            tokens,
            thread,
            now: model.now,
            wide: false,
        }
        .view(&waiting, pick)
    });
    let head = bound.zip(thread).map(|(bound, thread)| {
        let header = child::header(pane, &key, thread, agent);
        col()
            .w_full()
            .child(header)
            .children(child::replay(&key, thread, bound))
    });
    let foot = match bound {
        Some(bound) => child::foot(pane, &key, bound, agent),
        None => composer(pane, &key, thread),
    };
    let tint = if pane.focused {
        tokens.glass_pane_focus
    } else {
        tokens.glass_pane
    };
    let body = col()
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .children(thread.map(|thread| hold(&key, id, thread)))
        .children(head)
        .child(conversation)
        .children(jump)
        .child(
            reading(col())
                .pb(6.0)
                .gap(8.0)
                .children(
                    thread
                        .and(down(pane))
                        .map(|down| bar(pane, &key, &down, &SERVER)),
                )
                .children(
                    bound
                        .is_none()
                        .then(|| crate::chat::tray::view(pane))
                        .flatten(),
                )
                .children(card)
                .child(foot),
        );
    let fill = colour(tokens.pane_tint.alpha(tint));
    environment::beside(pane, cx, &key, body, fill)
}

/// The thread's composer: prompts, with what is attached.
fn composer(pane: &PaneCx<'_>, key: &str, thread: Option<&Thread>) -> El {
    let model = pane.model();
    Composer {
        key,
        pane: pane.pane,
        tokens: pane.tokens(),
        thread,
        draft: drawn(pane),
        queued: thread
            .map(|t| model.wire.queued(&t.id).collect())
            .unwrap_or_default(),
        held: thread.is_some_and(|t| model.wire.holds(&t.id)),
        files: model.tables.get(yd_core::feed::FILES),
        hint: if model.attached.things.is_empty() {
            "Reply, @ a file, / a command"
        } else {
            "Ask about what you attached"
        },
        form: Form::Pane,
        send: Send::Prompt,
    }
    .view()
}

/// The reading column: as wide as Just chat's and in the pane's middle once the pane is wider.
fn reading(column: El) -> El {
    column.w_full().max_w(WIDTH).self_center().px(8.0)
}

/// Gives the keyboard to the waiting card as the pane switches to a thread, unless the
/// composer has it. Incident: after a switch, Return pressed the sidebar row, not Run.
fn hold(key: &str, pane: PaneId, thread: &Thread) -> El {
    let (key, id) = (key.to_owned(), thread.id.clone());
    el().key(format!("{key}.shows.{}", thread.id.as_str()))
        .absolute(0.0, 0.0)
        .size(0.0, 0.0)
        .on_layout_tx(move |win: &mut Win, _: [f32; 4], tx: &mut Tx| {
            let mut first = false;
            // Only a switch hands it: on the pane's first thread the user has pressed nothing.
            chat_mut(win, pane, |chat| {
                first = chat.held.as_ref().is_some_and(|held| held != &id);
                chat.held = Some(id.clone());
            });
            let typing = win.edits.get(&yd_ui::win::field(pane, yd_ui::shown::DRAFT));
            if !first || typing.is_some_and(|edit| edit.focused) {
                return;
            }
            if let Some(ask) = crate::chat::approval::showing(win, pane) {
                tx.focus(format!("{key}.asks.row.{}", ask.as_str()), false);
            }
        })
}

/// The pane as the mockup's frame 23 has it: 280 wide, so its tray is the mockup's 264.
fn with_tray(win: &Win, hub: &Hub, cx: &mut Cx<Win>) -> El {
    let pane = col().w(280.0).h_full();
    row()
        .w_full()
        .h_full()
        .child(spacer())
        .child(pane.child(alone(win, hub, cx, PaneKind::Chat)))
}

/// The pane as the mockup frames it: 336 wide at the window's right, under its tab bar.
fn framed(win: &Win, hub: &Hub, cx: &mut Cx<Win>) -> El {
    let pane = col().w(336.0).h_full();
    row()
        .w_full()
        .h_full()
        .child(spacer())
        .child(pane.child(alone(win, hub, cx, PaneKind::Chat)))
}
