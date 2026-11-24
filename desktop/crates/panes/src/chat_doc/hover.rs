//! Details on hover: facts in the margin, actions at the corner (record 3.2).

use std::time::Duration;

use rune::{Cursor, El, Shadow, TextStyle, Tx, col, row, text};
use yd_core::address::attach::AttachMsg;
use yd_core::address::thing::Thing;
use yd_core::address::uri::Address;
use yd_core::command::table::{Route, Run, Target};
use yd_core::ids::{EntryId, PaneId, ThreadId};
use yd_core::kind::Kind;
use yd_core::msg::Msg;
use yd_core::reduce::step::commands::FORK;
use yd_core::reduce::turns::Message;
use yd_core::tokens::Tokens;
use yd_ui::theme::look::{CODE, LABEL, colour};
use yd_ui::theme::scale::{HOVER, LINE_MENU, SHADOW_TIP, TIP};
use yd_ui::win::{Win, dispatch};

use crate::chat::messages::line;
use crate::chat_doc::view::Doc;

/// One hover-intent delay for every detail: a pass quicker than this shows nothing.
pub const INTENT: Duration = Duration::from_millis(350);
/// How long details stay once the pointer has left, so it can cross onto them.
pub const LEAVE: Duration = Duration::from_millis(120);

/// 10.5 mono on a 15 line, the margin's facts.
const FACT: TextStyle = TextStyle::new(10.5, 400.0).lh(15.0).family(CODE);
const GUTTER: f32 = 170.0;
/// Characters of a block's text an attached thing carries as its excerpt.
const EXCERPT: usize = 200;

/// A block of the document that has details: which one the pointer rests on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Block {
    /// An earlier turn's folded line, by its number.
    Turn(u32),
    Ask,
    Steps,
    /// A finished answer, by its index in the transcript.
    Answer(usize),
    /// The files the turn changed: the list and its cards.
    Changes,
    Writing,
}

impl Block {
    fn part(self) -> String {
        match self {
            Block::Turn(n) => format!("turn.{n}"),
            Block::Ask => "ask".to_owned(),
            Block::Steps => "steps".to_owned(),
            Block::Answer(at) => format!("answer.{at}"),
            Block::Changes => "changes".to_owned(),
            Block::Writing => "writing".to_owned(),
        }
    }
}

/// What a block's actions act on: its words and the message they belong to.
pub struct Said {
    pub words: String,
    pub thread: ThreadId,
    pub entry: Option<EntryId>,
}

/// The dollars an answer cost, as the server counted them.
pub fn cost(message: &Message) -> f64 {
    message.spent().map_or(0.0, |spent| spent.cost)
}

/// What every block of one document shares.
pub struct Hover<'a> {
    pub key: &'a str,
    pub pane: PaneId,
    pub tokens: &'a Tokens,
    pub doc: Option<&'a Doc>,
}

impl Hover<'_> {
    pub fn key_of(&self, block: Block) -> String {
        format!("{}.block.{}", self.key, block.part())
    }

    /// Whether `block` shows its details now.
    pub fn shown(&self, block: Block) -> bool {
        self.doc.is_some_and(|doc| doc.shows(block))
    }

    /// `body` as a block: the pointer resting on it shows `facts` in the left margin and,
    /// when `said` is some, the action strip at its corner. Neither moves anything.
    pub fn block(&self, block: Block, facts: &[String], said: Option<Said>, body: El) -> El {
        let pane = self.pane;
        let key = self.key_of(block);
        let shown = self.shown(block);
        let gutter = shown.then(|| {
            col()
                .absolute(-(GUTTER + 16.0), 2.0)
                .w(GUTTER)
                .children(facts.iter().map(|fact| {
                    row().w_full().justify_end().child(
                        text(fact.as_str())
                            .font(FACT)
                            .color(colour(self.tokens.faint)),
                    )
                }))
        });
        let strip = said.filter(|_| shown).map(|said| self.strip(&key, said));
        col()
            .key(key)
            .w_full()
            .on_hover_change(move |win: &mut Win, inside, _tx: &mut Tx| {
                point(win, pane, block, inside)
            })
            .hover_delays(INTENT, LEAVE)
            .child(body)
            .children(gutter)
            .children(strip)
    }

    /// Copy, Attach and Fork from here, straddling the block's top edge.
    fn strip(&self, key: &str, said: Said) -> El {
        let tokens = self.tokens;
        let segment = |words: &'static str, part: &str| {
            row()
                .key(format!("{key}.{part}"))
                .h(18.0)
                .px(7.0)
                .center_y()
                .cursor(Cursor::Pointer)
                .hover(colour(HOVER.of(tokens)))
                .child(text(words).font(LABEL).color(colour(tokens.dim)))
        };
        let address = said
            .entry
            .as_ref()
            .and_then(|entry| {
                let id = format!("{}/{}", said.thread.as_str(), entry.as_str());
                Address::new(Kind::Message, id).ok()
            })
            .or_else(|| Address::new(Kind::Thread, said.thread.as_str()).ok());
        let words = said.words;
        let thing = address.map(|address| Thing {
            label: line(&words, 40),
            address,
            excerpt: Some(line(&words, EXCERPT)),
        });
        let copied = words.clone();
        let fork = said.entry.is_some().then(|| thing.clone()).flatten();
        row()
            .pin(None, Some(-11.0), Some(0.0), None)
            .h(18.0)
            .radius(6.0)
            .fill(colour(TIP.of(tokens)))
            .border(1.0, colour(LINE_MENU.of(tokens)))
            .box_shadow(Shadow {
                y: 2.0,
                blur: 6.0,
                color: colour(SHADOW_TIP.of(tokens)),
                ..Shadow::default()
            })
            .child(
                segment("Copy", "copy")
                    .on_press_tx(move |_: &mut Win, tx: &mut Tx| tx.copy(copied.clone())),
            )
            .child(segment("Attach", "attach").on_press(move |win: &mut Win| {
                if let Some(thing) = thing.clone() {
                    dispatch(win, Msg::Attach(AttachMsg::Toggle(thing)));
                }
            }))
            .child(
                segment("Fork from here", "fork").on_press(move |win: &mut Win| {
                    if let Some(thing) = fork.clone() {
                        let run = Run {
                            command: FORK,
                            target: Target::Thing(thing.address),
                            text: Some("at".to_owned()),
                        };
                        dispatch(win, Msg::Command(Route::Wire, run));
                    }
                }),
            )
    }
}

/// The pointer came onto `block` or left it, after the intent delay.
pub fn point(win: &mut Win, pane: PaneId, block: Block, inside: bool) {
    let Ok(mut hub) = win.hub.try_borrow_mut() else {
        return;
    };
    let Some(doc) = hub.panes.get_mut::<Doc>(Some(pane)) else {
        return;
    };
    if inside {
        doc.hover = Some(block);
    } else if doc.hover == Some(block) {
        doc.hover = None;
    }
}

/// The pointer came onto the steps pop-over or left it.
pub fn over(win: &mut Win, pane: PaneId, inside: bool) {
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(doc) = hub.panes.get_mut::<Doc>(Some(pane))
    {
        doc.over = inside;
    }
}
