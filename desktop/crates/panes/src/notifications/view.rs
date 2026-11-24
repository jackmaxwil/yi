//! The notification panel under the bell: what needs you first, answered in place.

mod bell;
mod scenes;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;

use rune::{Cursor, El, Key, KeyEvent, Shadow, TextStyle, Tx, col, el, row, text};
use yd_core::address::uri::Address;
use yd_core::command::table::{Route, Run, Target};
use yd_core::ids::{ApprovalId, CommandId, Millis, ThreadId};
use yd_core::kind::Kind;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_core::reduce::approvals::{Offer, OptionKind};
use yd_core::reduce::state::Wire;
use yd_core::reduce::step::commands::ANSWER;
use yd_core::reduce::threads::Want;
use yd_core::row::Dot;
use yd_core::tokens::{Tokens, rgba};
use yd_ui::grid_view::view::{Act, act};
use yd_ui::kit::button::{Button, button};
use yd_ui::kit::status::dot;
use yd_ui::shown::show;
use yd_ui::theme::look::{BODY, CODE, LABEL, META, colour};
use yd_ui::theme::scale::{LINE_CARD, LINE_DIVIDER, RADIUS_MENU, RING_MENU, SEMIBOLD, SHADOW_MENU};
use yd_ui::win::{Hub, Win, dispatch};

pub use bell::{Bell, Item};
pub use scenes::SCENES;

/// Opens and closes the panel: what the top bar's bell runs.
pub use yd_core::command::table::TOGGLE_NOTIFICATIONS as TOGGLE;

use super::states::WORDS;

const WIDTH: f32 = 322.0;
/// Under the top bar, off the window's right edge.
const TOP: f32 = 27.0;
const RIGHT: f32 = 6.0;
/// Above the grid, its menus and the layout picker.
const OVER: i16 = 60;
const KEY: &str = "notifications.notification-panel";
const TITLE: TextStyle = TextStyle {
    weight: SEMIBOLD,
    ..BODY
};
const CODE_WORDS: TextStyle = TextStyle::new(10.5, 400.0).lh(14.7).family(CODE);
const FILL: (u8, u8, u8, f32) = (20, 22, 28, 0.74);

/// What pressing one of a row's buttons does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Press {
    /// Answers the ask with the option the server offered, by its id.
    Answer(ApprovalId, String),
    Open(ThreadId),
    /// Opens the thread and brings its Changes pane forward.
    Review(ThreadId),
}

/// One of a row's buttons: its words and its look.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub label: String,
    pub look: Button,
    pub press: Press,
}

fn offered(ask: &ApprovalId, offer: &Offer<'_>) -> Choice {
    let (look, label) = match &offer.kind {
        OptionKind::AllowOnce => (Button::Primary, "Allow once"),
        OptionKind::AllowAlways => (Button::Plain, "Always"),
        OptionKind::RejectOnce => (Button::Ghost, "Deny"),
        OptionKind::RejectAlways => (Button::Ghost, "Never"),
        // A kind a newer server offers keeps the server's own word for it.
        OptionKind::Other(_) => (Button::Ghost, offer.name),
    };
    Choice {
        label: label.to_owned(),
        look,
        press: Press::Answer(ask.clone(), offer.id.0.to_owned()),
    }
}

/// The buttons a row carries: an approval's offers, a question's reply and thread, a finished
/// thread's review. A read row has none.
pub fn choices(wire: &Wire, item: &Item) -> Vec<Choice> {
    let open = |look| Choice {
        label: "Open thread".to_owned(),
        look,
        press: Press::Open(item.thread.clone()),
    };
    match (&item.want, item.read) {
        (_, true) => Vec::new(),
        (Want::Approval(id), false) => {
            let ask = wire.waiting().find(|ask| &ask.id == id);
            let offers = ask.into_iter().flat_map(|ask| ask.offers());
            offers.map(|offer| offered(id, &offer)).collect()
        }
        (Want::NeedsYou | Want::Child(_), false) => {
            let answer = Choice {
                label: "Answer".to_owned(),
                look: Button::Plain,
                press: Press::Open(item.thread.clone()),
            };
            vec![answer, open(Button::Ghost)]
        }
        (Want::Done, false) => vec![Choice {
            label: "Review changes".to_owned(),
            look: Button::Plain,
            press: Press::Review(item.thread.clone()),
        }],
        (Want::Failed, false) => vec![open(Button::Plain)],
    }
}

fn dot_of(item: &Item) -> Dot {
    match (&item.want, item.read) {
        (_, true) => Dot::Idle,
        (Want::Approval(_) | Want::NeedsYou | Want::Child(_), false) => Dot::Need,
        (Want::Failed, false) => Dot::Fail,
        (Want::Done, false) => Dot::Done,
    }
}

/// Carries a press out. The panel closes once a thread is opened, not when an ask is answered.
fn pressed(win: &mut Win, tx: &mut Tx, press: &Press) {
    match press {
        Press::Answer(ask, option) => {
            let Ok(address) = Address::new(Kind::Approval, ask.as_str()) else {
                return;
            };
            let run = Run {
                command: ANSWER,
                target: Target::Thing(address),
                text: Some(option.clone()),
            };
            dispatch(win, Msg::Command(Route::Wire, run));
        }
        Press::Open(thread) | Press::Review(thread) => {
            show(win, thread.clone());
            toggle(win, false);
            if matches!(press, Press::Review(_)) {
                focus_changes(win, tx);
            }
        }
    }
}

fn focus_changes(win: &mut Win, tx: &mut Tx) {
    let pane = win.hub.try_borrow().ok().and_then(|hub| {
        let grid = &hub.model.grid;
        let mut docked = grid.window(win.window)?.layout.docked();
        docked.find_map(|(pane, _)| (grid.kind(pane) == Some(PaneKind::Changes)).then_some(pane))
    });
    if let Some(pane) = pane {
        act(win, tx, Act::Focus(pane));
    }
}

/// Opens or closes the panel.
fn toggle(win: &mut Win, open: bool) {
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(bell) = hub.panes.get_mut::<Bell>(None)
    {
        bell.open = open;
    }
}

/// Runs the bell's command; false when `command` is not the bell's.
pub fn run(win: &mut Win, tx: &mut Tx, command: CommandId) -> bool {
    if command != TOGGLE {
        return false;
    }
    let open = win
        .hub
        .try_borrow()
        .ok()
        .and_then(|hub| hub.panes.get::<Bell>(None).map(|bell| bell.open))
        .unwrap_or(false);
    toggle(win, !open);
    if !open {
        tx.focus_when_shown(KEY, false);
    }
    true
}

/// Esc closes the panel; true when it was open and took the key.
pub fn key(win: &mut Win, event: &KeyEvent) -> bool {
    let open = open(win);
    if open && event.key == Key::Escape {
        toggle(win, false);
    }
    open && event.key == Key::Escape
}

fn open(win: &Win) -> bool {
    let hub = win.hub.try_borrow().ok();
    hub.is_some_and(|hub| hub.panes.get::<Bell>(None).is_some_and(|bell| bell.open))
}

/// Folds the wire's attention list into the panel's rows and returns the rows that are
/// news. The window's poll calls it each frame.
pub fn sync(win: &mut Win) -> Vec<Item> {
    let Ok(mut hub) = win.hub.try_borrow_mut() else {
        return Vec::new();
    };
    let Hub { model, panes, .. } = &mut *hub;
    let bell = panes.get_mut::<Bell>(None);
    bell.map(|bell| bell.sync(&model.wire)).unwrap_or_default()
}

/// The bell's badge: how many threads at the top of the list have news the user has not
/// read. Two rows of one thread count once; a fork's row is its own in the panel only.
pub fn unread(hub: &Hub) -> usize {
    let Some(bell) = hub.panes.get::<Bell>(None) else {
        return 0;
    };
    let tops = yd_ui::shown::roots(&hub.model.wire);
    let top = |id: &&ThreadId| tops.iter().any(|thread| thread.id == **id);
    let news: BTreeSet<&ThreadId> = bell.news().filter(top).collect();
    news.len()
}

fn detail(tokens: &Tokens, words: &str) -> El {
    let line = row().gap(4.0).center_y().min_w(0.0);
    let plain = |words: &str| {
        text(words)
            .font(LABEL)
            .color(colour(tokens.dim))
            .truncate()
            .min_w(0.0)
    };
    match words.strip_prefix("Wants to run ") {
        Some(command) => line.child(plain("Wants to run")).child(
            text(command)
                .font(CODE_WORDS)
                .color(colour(tokens.fg))
                .truncate()
                .min_w(0.0)
                .px(5.0)
                .py(1.0)
                .radius(4.0)
                .fill(colour(tokens.raised)),
        ),
        None => line.child(plain(words)),
    }
}

/// The panel with nothing in it.
fn empty(tokens: &Tokens) -> El {
    col()
        .key("notifications.empty")
        .w_full()
        .py(18.0)
        .gap(3.0)
        .center()
        .child(text(WORDS.empty.what).font(BODY).color(colour(tokens.dim)))
        .child(
            text(WORDS.empty.next)
                .font(META)
                .color(colour(tokens.faint)),
        )
}

fn card(hub: &Hub, bell: &Bell) -> El {
    let tokens = &hub.model.tokens;
    let now = hub.model.now;
    let head = row()
        .key("notifications.header")
        .h(27.0)
        .px(9.0)
        .pt(6.0)
        .pb(5.0)
        .gap(8.0)
        .center_y()
        .border_sides([0.0, 0.0, 1.0, 0.0], colour(LINE_DIVIDER.of(tokens)))
        .child(text("Notifications").font(TITLE).color(colour(tokens.fg)))
        .child(
            text(format!("{} new", bell.unread()))
                .font(LABEL)
                .color(colour(tokens.faint)),
        )
        .child(el().grow(1.0))
        .child(
            text("Mark all read")
                .key("notifications.mark-read")
                .font(LABEL)
                .color(colour(tokens.faint))
                .cursor(Cursor::Pointer)
                .on_press_tx(|win: &mut Win, _: &mut Tx| {
                    if let Ok(mut hub) = win.hub.try_borrow_mut()
                        && let Some(bell) = hub.panes.get_mut::<Bell>(None)
                    {
                        bell.mark_all_read();
                    }
                }),
        );
    let rows = bell
        .shown()
        .into_iter()
        .map(|item| line(hub, item, now))
        .collect::<Vec<_>>();
    let body = if rows.is_empty() {
        empty(tokens)
    } else {
        col().children(rows)
    };
    col()
        .key(KEY)
        .w(WIDTH)
        .radius(RADIUS_MENU.of(tokens))
        .clip_rounded(RADIUS_MENU.of(tokens))
        .fill(colour(rgba(FILL.0, FILL.1, FILL.2, FILL.3)))
        .backdrop_blur(30.0)
        .paint_order(OVER)
        .focusable(true)
        .on_key(|win: &mut Win, event: &KeyEvent, _: &mut Tx| key(win, event))
        .box_shadow(Shadow {
            spread: 1.0,
            color: colour(RING_MENU.of(tokens)),
            ..Shadow::default()
        })
        .box_shadow(Shadow {
            y: 22.0,
            blur: 50.0,
            spread: -14.0,
            color: colour(SHADOW_MENU.of(tokens)),
            ..Shadow::default()
        })
        .child(head)
        .child(body)
}

fn line(hub: &Hub, item: &Item, now: Millis) -> El {
    let tokens = &hub.model.tokens;
    let id = item.id();
    let buttons = choices(&hub.model.wire, item);
    let actions = (!buttons.is_empty()).then(|| {
        let made = buttons.into_iter().enumerate().map(|(at, choice)| {
            let press = choice.press;
            button(tokens, choice.look, &choice.label, None)
                .key(format!("notifications.row.{id}.button-{at}"))
                .on_press_tx(move |win: &mut Win, tx: &mut Tx| pressed(win, tx, &press))
        });
        row().ml(17.0).mt(3.0).gap(5.0).children(made)
    });
    col()
        .key(format!("notifications.row.{id}"))
        .px(9.0)
        .py(6.0)
        .gap(1.0)
        .border_sides([1.0, 0.0, 0.0, 0.0], colour(LINE_CARD.of(tokens)))
        .opacity(if item.read { 0.6 } else { 1.0 })
        .child(
            row()
                .gap(7.0)
                .center_y()
                .child(row().w(10.0).center().child(dot(tokens, dot_of(item))))
                .child(
                    text(&item.title)
                        .font(TITLE)
                        .color(colour(tokens.fg))
                        .truncate()
                        .grow(1.0)
                        .min_w(0.0),
                )
                .child(
                    text(now.age(item.since))
                        .font(META)
                        .color(colour(tokens.faint)),
                ),
        )
        .child(row().ml(17.0).child(detail(tokens, &item.detail)))
        .children(actions)
}

/// The panel and the scrim behind it that closes it, hung from the window's top right; empty
/// while the panel is closed. The shell lays these over everything it draws.
pub fn layer(hub: &Hub) -> Vec<El> {
    let Some(bell) = hub.panes.get::<Bell>(None).filter(|bell| bell.open) else {
        return Vec::new();
    };
    let scrim = el()
        .key("notifications.scrim")
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .paint_order(OVER)
        .on_background_press_tx(|win: &mut Win, _: &mut Tx| toggle(win, false));
    let at = card(hub, bell).pin(None, Some(TOP), Some(RIGHT), None);
    vec![scrim, at]
}
