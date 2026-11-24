//! The tray of attached things: a header, one chip a thing, and the address of the lit one.

use rune::{Cursor, El, Icon, TextStyle, Tx, col, row, spacer, text};
use yd_core::address::attach::AttachMsg;
use yd_core::address::thing::{Thing, gone};
use yd_core::address::uri::Address;
use yd_core::kind::Kind;
use yd_core::msg::Msg;
use yd_core::tokens::Tokens;

use super::{NAME, NUMBER, ON_CONTEXT};
use crate::chrome::tabs::{BRANCH, DIFF, FILE, GLOBE, MONITOR, TARGET, TERM};
use crate::kit::button::PRESS_SCALE;
use crate::kit::icons::glyph;
use crate::overlay::pick::{self, Context};
use crate::theme::look::{LABEL, META, colour};
use crate::theme::scale::{NOTE, STEP};
use crate::win::{Hub, Win, dispatch};

const COUNT: TextStyle = TextStyle {
    weight: 600.0,
    ..META
};

/// The icon a chip carries for its kind (record 4.8): lines a diff, a thread or an agent a
/// branch, a screen or a machine a monitor, the web a globe, a file a file, a log a terminal.
fn icon_of(kind: Kind) -> &'static Icon {
    match kind {
        Kind::Lines => &DIFF,
        Kind::Thread | Kind::Agent => &BRANCH,
        Kind::Screen | Kind::Vm => &MONITOR,
        Kind::Element | Kind::Property => &GLOBE,
        Kind::File | Kind::Reference => &FILE,
        Kind::LogLine => &TERM,
        Kind::Comment
        | Kind::Step
        | Kind::Command
        | Kind::Approval
        | Kind::Answer
        | Kind::Message
        | Kind::Turn
        | Kind::Card
        | Kind::Page
        | Kind::Span
        | Kind::Finding
        | Kind::Asset
        | Kind::Container
        | Kind::Service
        | Kind::StackFrame
        | Kind::Variable
        | Kind::Breakpoint
        | Kind::Notification
        | Kind::Plugin
        | Kind::Setting
        | Kind::Theme
        | Kind::ThemeValue
        | Kind::Number
        | Kind::Check
        | Kind::Place
        | Kind::StandingCheck
        | Kind::Problem
        | Kind::LanguageServer
        | Kind::Tab
        | Kind::Pane => &TARGET,
    }
}

/// The key of a thing's chip in the tray keyed `tray`: `<tray>.chip.<address>`.
pub fn chip_key(tray: &str, address: &Address) -> String {
    [tray, ".chip.", &address.to_string()].concat()
}

/// A quiet word in the header or the address line that does something when pressed.
fn word(tokens: &Tokens, key: String, says: &'static str, dim: bool) -> El {
    let ink = if dim { tokens.dim } else { tokens.faint };
    text(says)
        .key(key)
        .font(META)
        .color(colour(ink))
        .cursor(Cursor::Pointer)
}

struct Chip<'a> {
    tokens: &'a Tokens,
    tray: &'a str,
    number: usize,
    thing: &'a Thing,
    /// Lit by the pointer or pressed last: drawn stronger.
    on: bool,
    /// Its source is no longer there: the name is struck through.
    gone: bool,
}

impl Chip<'_> {
    fn view(&self) -> El {
        let tokens = self.tokens;
        let context = tokens.context;
        let key = chip_key(self.tray, &self.thing.address);
        let (fill, ring) = if self.on { (0.22, 0.5) } else { (0.1, 0.18) };
        let ink = match (self.gone, self.on) {
            (true, _) => tokens.faint,
            (false, true) => tokens.fg,
            (false, false) => tokens.dim,
        };
        let label = &self.thing.label;
        let name = text(label.as_str())
            .font(LABEL)
            .color(colour(ink))
            .truncate()
            .min_w(0.0);
        let name = if self.gone {
            name.strike(0, label.len(), None)
        } else {
            name
        };
        let number = row()
            .size(14.0, 14.0)
            .center()
            .radius(4.0)
            .fill(colour(context))
            .child(
                text(self.number.to_string())
                    .font(NUMBER)
                    .color(colour(ON_CONTEXT.of(tokens))),
            );
        let lit = self.thing.address.clone();
        let (active, removed) = (self.thing.address.clone(), self.thing.clone());
        let remove = text("×")
            .key(format!("{key}.remove"))
            .font(LABEL)
            .color(colour(tokens.faint))
            .ml(1.0)
            .cursor(Cursor::Pointer)
            .on_press(move |win: &mut Win| {
                dispatch(win, Msg::Attach(AttachMsg::Toggle(removed.clone())));
            });
        row()
            .key(key)
            .h(18.0)
            .min_w(0.0)
            .pl(2.0)
            .pr(5.0)
            .gap(4.0)
            .center_y()
            .radius(5.0)
            .fill(colour(context.alpha(fill)))
            .border(1.0, colour(context.alpha(ring)))
            .cursor(Cursor::Pointer)
            .press_scale(PRESS_SCALE)
            .on_hover_change(move |win: &mut Win, inside: bool, _: &mut Tx| {
                pick::point_chip(win, lit.clone(), inside);
            })
            .on_press(move |win: &mut Win| pick::activate(win, active.clone()))
            .child(number)
            .child(glyph(icon_of(self.thing.kind()), 10.0, ink.alpha(0.8)))
            .child(name)
            .child(remove)
    }
}

/// The tray keyed `key`, for above a composer: nothing while nothing is attached. Pointing
/// at a chip lights its box, and pointing at a box lights its chip.
pub fn tray(hub: &Hub, key: &str) -> Option<El> {
    let tokens = &hub.model.tokens;
    let things = &hub.model.attached.things;
    let last = things.last()?;
    let context = pick::context(hub);
    let on = |address: &&Address| things.iter().any(|thing| thing.address == **address);
    let lit = context.and_then(Context::lit).filter(on);
    let active = context.and_then(|context| context.active.as_ref());
    let active = active.filter(on).unwrap_or(&last.address);
    let shown = lit.unwrap_or(active).to_string();
    let links: Vec<String> = things.iter().map(|t| t.address.to_string()).collect();
    let header = row()
        .w_full()
        .gap(9.0)
        .center_y()
        .pt(1.0)
        .px(2.0)
        .pb(3.0)
        .child(
            text(format!("{} attached", things.len()))
                .font(COUNT)
                .color(colour(tokens.dim)),
        )
        .child(spacer())
        .child(
            word(tokens, format!("{key}.copy-links"), "Copy links", false)
                .on_press_tx(move |_: &mut Win, tx: &mut Tx| tx.copy(links.join("\n"))),
        )
        .child(
            word(tokens, format!("{key}.clear"), "Clear", false)
                .on_press(|win: &mut Win| dispatch(win, Msg::Attach(AttachMsg::Clear))),
        );
    let chips = things.iter().enumerate().map(|(index, thing)| {
        Chip {
            tokens,
            tray: key,
            number: index.saturating_add(1),
            thing,
            on: lit.unwrap_or(active) == &thing.address,
            gone: gone(&hub.model, &thing.address),
        }
        .view()
    });
    let copied = shown.clone();
    let address = row()
        .key(format!("{key}.address"))
        .w_full()
        .mt(4.0)
        .px(4.0)
        .py(2.0)
        .gap(6.0)
        .center_y()
        .radius(5.0)
        .fill(colour(NOTE.of(tokens)))
        .child(
            text(shown)
                .font(TextStyle {
                    weight: 400.0,
                    ..NAME
                })
                .color(colour(tokens.faint))
                .truncate()
                .grow(1.0)
                .min_w(0.0),
        )
        .child(
            word(tokens, format!("{key}.address.copy"), "copy", true)
                .on_press_tx(move |_: &mut Win, tx: &mut Tx| tx.copy(copied.clone())),
        );
    // The mockup's 4 by 6 padding and its 1 px border, which here draws inside the box.
    let tray = col()
        .key(key.to_owned())
        .w_full()
        .px(7.0)
        .py(5.0)
        .radius(8.0)
        .fill(colour(STEP.of(tokens)))
        .border(1.0, colour(tokens.control_line))
        .child(header)
        .child(
            row()
                .flex_wrap()
                .w_full()
                .min_w(0.0)
                .gap(3.0)
                .children(chips),
        )
        .child(address);
    Some(tray)
}
