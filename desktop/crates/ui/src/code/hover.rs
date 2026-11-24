//! The hover card under a token: the problem's message, the server's Markdown, then links.

use std::collections::VecDeque;
use std::rc::Rc;

use rune::{Align, Color, Cursor, El, Shadow, Side, TextStyle, Tx, col, el, row, spacer, text};
use rune_kit::{Markdown, MarkdownStyle};
use yd_core::row::Tone;
use yd_core::tokens::Tokens;

use crate::kit::status::badge;
use crate::theme::look::{CODE, LABEL, META, colour};
use crate::theme::scale::{CARD, LINE_SOFT, RADIUS_CARD, RING_CARD, SHADOW_MENU};
use crate::win::Win;

/// The card is 266 wide (record 6.4.2).
pub const WIDTH: f32 = 266.0;
/// Hovers remembered as asked; the oldest is forgotten past it.
const ASKED: usize = 64;
const PROSE: TextStyle = LABEL.lh(14.7);
const SIGNATURE: TextStyle = LABEL.lh(15.0).family(CODE);

/// What pressing a link does, given the window and the pointer's `Tx`.
pub type Press = Rc<dyn Fn(&mut Win, &mut Tx)>;

/// One accent word under the card. A link with no press is shown, not wired.
#[derive(Clone)]
pub struct Link {
    /// Names its key: `<card key>.link-<id>`.
    pub id: &'static str,
    pub label: String,
    pub press: Option<Press>,
}

/// What a card says.
pub struct Card<'a> {
    /// The severity's words and tone, then the message of the problem under the token.
    pub problem: Option<(&'a str, Tone, &'a str)>,
    pub markdown: &'a str,
    pub links: Vec<Link>,
    /// Where the item comes from, faint at the right of the links.
    pub origin: Option<&'a str>,
}

/// The hovers already asked, each with the state of the servers then: one asked while a server
/// started is asked again when that state moves, and never otherwise.
#[derive(Debug, Default)]
pub struct Asked(VecDeque<(String, u64)>);

impl Asked {
    /// Whether `spot` is to be asked now: it was not asked in this `epoch`. Records it.
    pub fn once(&mut self, spot: &str, epoch: u64) -> bool {
        match self.0.iter().position(|(asked, _)| asked == spot) {
            Some(at) if self.0.get(at).is_some_and(|(_, then)| *then == epoch) => return false,
            Some(at) => {
                self.0.remove(at);
            }
            None => {}
        }
        if self.0.len() >= ASKED {
            self.0.pop_front();
        }
        self.0.push_back((spot.to_owned(), epoch));
        true
    }
}

fn prose(tokens: &Tokens) -> MarkdownStyle {
    let none = Color::TRANSPARENT;
    MarkdownStyle::neutral(true)
        .text(colour(tokens.dim))
        .muted(colour(tokens.faint))
        .link(colour(tokens.accent))
        .body_font(PROSE)
        .code_block_font(SIGNATURE)
        .code_block(none, none)
        .code_block_padding(0.0, 0.0)
        .code_block_text(colour(tokens.fg))
        .plain_code_text(colour(tokens.fg))
        .syntax_highlight(false)
        .code_margin(0.0, 0.0)
}

fn link(tokens: &Tokens, key: &str, link: &Link) -> El {
    let word = text(link.label.as_str())
        .font(META)
        .color(colour(tokens.accent))
        .key(format!("{key}.link-{}", link.id));
    match &link.press {
        Some(press) => {
            let press = press.clone();
            word.cursor(Cursor::Pointer)
                .on_press_tx(move |win: &mut Win, tx: &mut Tx| press(win, tx))
        }
        None => word,
    }
}

/// The card, keyed `key`. The problem comes first, because it is what the reader came for.
pub fn card(tokens: &Tokens, key: &str, card: &Card<'_>) -> El {
    let problem = card.problem.map(|(words, tone, message)| {
        row()
            .gap(6.0)
            .items_start()
            .child(badge(tokens, words, tone))
            .child(
                text(message)
                    .font(PROSE)
                    .color(colour(tokens.fg))
                    .wrap()
                    .grow(1.0)
                    .min_w(0.0),
            )
    });
    let said = (!card.markdown.trim().is_empty())
        .then(|| Markdown::new(card.markdown).render(&prose(tokens)));
    let links = (!card.links.is_empty() || card.origin.is_some()).then(|| {
        row()
            .gap(11.0)
            .pt(3.0)
            .center_y()
            .border_sides([1.0, 0.0, 0.0, 0.0], colour(LINE_SOFT.of(tokens)))
            .children(card.links.iter().map(|each| link(tokens, key, each)))
            .child(spacer())
            .children(
                card.origin
                    .map(|origin| text(origin).font(META).color(colour(tokens.faint))),
            )
    });
    col()
        .key(key)
        .w(WIDTH)
        .px(8.0)
        .py(6.0)
        .gap(4.0)
        .radius(RADIUS_CARD.of(tokens))
        .fill(colour(CARD.of(tokens)))
        .box_shadow(Shadow {
            spread: 1.0,
            color: colour(RING_CARD.of(tokens)),
            ..Shadow::default()
        })
        .box_shadow(Shadow {
            y: 14.0,
            blur: 30.0,
            spread: -8.0,
            color: colour(SHADOW_MENU.of(tokens)),
            ..Shadow::default()
        })
        .children(problem)
        .children(said)
        .children(links)
}

/// The room a card needs left of a line's text: the gutter and a margin.
const GUTTER: f32 = 48.0;
/// The room a card leaves to the surface's edges when it is narrowed to fit.
const MARGIN: f32 = 8.0;

/// Puts `card` under the element keyed `anchor`, `shift` points along it, and inside `room`
/// (the surface's width). Add the result last in the pane, so it draws over the lines.
pub fn under(card: El, anchor: &str, shift: f32, room: Option<f32>) -> El {
    let most = room.map_or(f32::MAX, |room| (room - WIDTH - GUTTER).max(0.0));
    // A surface narrower than the card cut it off; the card is never wider than its surface.
    let card = match room {
        Some(room) => card.max_w((room - MARGIN).max(MARGIN)),
        None => card,
    };
    row().child(el().w(shift.min(most))).child(card).anchored(
        anchor,
        Side::Bottom,
        Align::Start,
        2.0,
    )
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use rune::{Cx, El, Env, Runtime};
    use yd_core::row::Tone;
    use yd_core::tokens::Tokens;

    use super::{ASKED, Asked, Card, Link, card, under};

    type Check = Result<(), Box<dyn Error>>;

    #[test]
    fn a_hover_is_asked_once_until_the_servers_change() {
        let mut asked = Asked::default();
        assert!(asked.once("a.rs:4:7", 1));
        assert!(!asked.once("a.rs:4:7", 1));
        assert!(asked.once("a.rs:4:9", 1));
        assert!(asked.once("a.rs:4:7", 2));
        assert!(!asked.once("a.rs:4:7", 2));
    }

    #[test]
    fn the_cache_forgets_the_oldest_spot_past_its_bound() {
        let mut asked = Asked::default();
        for at in 0..=ASKED {
            assert!(asked.once(&format!("a.rs:{at}:1"), 1));
        }
        assert!(asked.once("a.rs:0:1", 1));
        assert!(!asked.once(&format!("a.rs:{ASKED}:1"), 1));
    }

    fn view(_: &(), _: &mut Cx<()>) -> El {
        let tokens = Tokens::default();
        let links = vec![Link {
            id: "definition",
            label: "Go to definition".to_owned(),
            press: None,
        }];
        let said = Card {
            problem: Some(("Warn", Tone::Warn, "can panic if the clock steps back")),
            markdown: "```rust\npub const fn from_secs(secs: u64) -> Duration\n```\n\nMakes a length of time.",
            links,
            origin: Some("core"),
        };
        card(&tokens, "hover", &said)
    }

    #[test]
    fn the_problem_comes_before_the_servers_words_and_the_links_last() -> Check {
        let mut rt = Runtime::new((), view, (400.0, 300.0), Env::new(true, 1.0));
        for _ in 0..3 {
            rt.frame(0.016);
        }
        let all = rt.elements();
        let top = |needle: &str| {
            all.iter()
                .filter(|e| !e.exiting)
                .find(|e| e.text.as_deref().is_some_and(|t| t.contains(needle)))
                .map(|e| e.rect[1])
                .ok_or(format!("no text with {needle}"))
        };
        let (problem, signature) = (top("clock steps back")?, top("from_secs")?);
        let (prose, links) = (top("Makes a length")?, top("Go to definition")?);
        assert!(problem < signature && signature < prose && prose < links);
        Ok(())
    }

    fn narrow(_: &(), _: &mut Cx<()>) -> El {
        let line = rune::el().key("line").w(180.0).h(16.0);
        let said = Card {
            problem: None,
            markdown: "no language server for .md files",
            links: Vec::new(),
            origin: None,
        };
        let shown = under(
            card(&Tokens::default(), "hover", &said),
            "line",
            0.0,
            Some(180.0),
        );
        rune::col().w(180.0).h_full().child(line).child(shown)
    }

    #[test]
    fn a_card_over_a_surface_narrower_than_it_fits_inside_the_surface() -> Check {
        let mut rt = Runtime::new((), narrow, (400.0, 300.0), Env::new(true, 1.0));
        for _ in 0..3 {
            rt.frame(0.016);
        }
        let all = rt.elements();
        let shown = all.iter().find(|e| e.key.as_deref() == Some("hover"));
        let [x, _, w, _] = shown.ok_or("no card")?.rect;
        assert!(
            x >= 0.0 && x + w <= 180.0,
            "the card runs from {x} to {}",
            x + w
        );
        Ok(())
    }
}
