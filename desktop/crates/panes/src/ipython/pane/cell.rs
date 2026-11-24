//! One kernel cell: its input highlighted as Python, then its outputs as a notebook draws them.

use std::cell::RefCell;
use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;

use rune::{Color, Cursor, El, Image, ImageCache, Span, SurfaceFit, Tx, col, el, rich, row, text};
use rune_kit::{Markdown, MarkdownStyle};
use yd_core::ids::PaneId;
use yd_core::reduce::cells::{Mime, Output, Picture};
use yd_core::reduce::turns::{ToolCall, ToolState};
use yd_core::tokens::{Rgba, Tokens};
use yd_ui::code::colour::Colours;
use yd_ui::code::doc::{Doc, Lang, LineNo};
use yd_ui::code::view::Look;
use yd_ui::kit::button::{Button, button};
use yd_ui::theme::look::{BODY, CODE_LINE, META, colour};
use yd_ui::theme::scale::{CONSOLE, RADIUS_CHIP};
use yd_ui::win::Win;

use super::Notebook;

/// The prompt column: `In`, `In [*]` while the cell runs, `Out` beside a value.
const PROMPT_W: f32 = 38.0;
/// Characters one output draws, its first ones; the rest is named in a notice where it is cut.
pub const MAX_OUTPUT: usize = 20_000;
/// Pictures kept decoded for the view to draw again.
const KEPT: usize = 32;
/// Bytes of a picture decoded on the UI thread before it is drawn, a millisecond or two, so the
/// first frame (a headless picture) has it; larger ones decode on Rune's workers.
const INLINE: usize = 256 * 1024;

/// What the cells of one pane are drawn with.
pub struct Drawn<'a> {
    pub key: &'a str,
    pub pane: PaneId,
    pub tokens: &'a Tokens,
    pub look: &'a Look,
    pub full: Option<&'a BTreeSet<String>>,
}

impl Drawn<'_> {
    /// The cell keyed `<pane key>.cell.<call id>`: input, what it is doing, then its outputs.
    pub fn cell(&self, call: &ToolCall) -> El {
        let key = format!("{}.cell.{}", self.key, call.id.as_str());
        let outputs = call.outputs();
        let doing = self.doing(&key, call.state, outputs.is_empty());
        let drawn = outputs.iter().enumerate();
        col()
            .key(key.clone())
            .w_full()
            .gap(4.0)
            .child(self.input(&key, call))
            .children(doing)
            .children(drawn.map(|(n, output)| self.output(&key, n, output)))
    }

    fn prompt(&self, words: &str, ink: Color) -> El {
        text(words)
            .font(META.mono())
            .color(ink)
            .w(PROMPT_W)
            .self_start()
            .mt(3.0)
    }

    /// A prompt, then `body` taking the rest of the row.
    fn beside(&self, words: &str, body: El) -> El {
        let prompt = self.prompt(words, colour(self.tokens.faint));
        row()
            .w_full()
            .gap(6.0)
            .child(prompt)
            .child(col().grow(1.0).min_w(0.0).child(body))
    }

    fn input(&self, key: &str, call: &ToolCall) -> El {
        let tokens = self.tokens;
        let code = call.code();
        let doc = Doc::new(code, Lang::PYTHON, 0);
        let colours = Colours::of(&doc);
        let fg = colour(tokens.fg);
        let lines = (0..doc.lines()).map(|index| {
            let no = LineNo::from_index(index);
            let line = doc.line(no).unwrap_or("");
            let runs = colours.line(no).iter().filter_map(|run| {
                let at = run.cols.within(line)?;
                Some(Span::new(at.start, at.end).color(self.look.syntax.color(run.kind)))
            });
            let shown = if line.is_empty() { " " } else { line };
            rich(shown, runs.collect()).font(CODE_LINE).color(fg)
        });
        let source = col()
            .grow(1.0)
            .min_w(0.0)
            .px(6.0)
            .py(3.0)
            .radius(RADIUS_CHIP.of(tokens))
            .fill(colour(CONSOLE.of(tokens)))
            .children(lines);
        let running = call.state == ToolState::Running;
        let words = if running { "In [*]" } else { "In" };
        let took = call.duration().filter(|_| !running).map(|took| {
            let seconds = Duration(took.0);
            text(seconds.to_string())
                .font(META)
                .color(colour(tokens.faint))
        });
        let copied = code.to_owned();
        let copy = button(tokens, Button::Ghost, "Copy", None)
            .key(format!("{key}.copy"))
            .label("Copy the cell's code")
            .on_press_tx(move |_: &mut Win, tx: &mut Tx| tx.copy(copied.clone()));
        let side = row()
            .self_start()
            .gap(4.0)
            .center_y()
            .children(took)
            .child(copy);
        row()
            .key(format!("{key}.input"))
            .w_full()
            .gap(6.0)
            .child(self.prompt(words, colour(tokens.accent)))
            .child(source)
            .child(side)
    }

    /// What a cell that has not finished is doing; nothing once it has, unless it never ran.
    fn doing(&self, key: &str, state: ToolState, nothing: bool) -> Option<El> {
        let tokens = self.tokens;
        let (words, ink) = match state {
            ToolState::Queued => ("Queued", tokens.faint),
            ToolState::Asking => ("Waiting for you to allow it", tokens.need),
            ToolState::Running => ("Running", tokens.accent),
            ToolState::Lost => ("Its approval was lost when the app closed", tokens.warn),
            ToolState::Failed if nothing => ("Did not run", tokens.bad),
            ToolState::Done | ToolState::Failed => return None,
        };
        let said = text(words).key(format!("{key}.state")).font(META);
        Some(self.beside("", said.color(colour(ink))))
    }

    fn output(&self, key: &str, n: usize, output: &Output<'_>) -> El {
        let tokens = self.tokens;
        match output {
            Output::Restarted => self.note(
                &format!("{key}.restarted"),
                "The kernel restarted before this cell: what earlier cells made is gone",
            ),
            Output::Stdout(said) => self.beside("", self.stream(&format!("{key}.stdout"), said)),
            Output::Stderr(said) => {
                let tint = colour(tokens.bad.alpha(0.08));
                let stream = self.stream(&format!("{key}.stderr"), said);
                self.beside("", stream.fill(tint).radius(RADIUS_CHIP.of(tokens)).px(4.0))
            }
            Output::Picture(picture) => {
                self.beside("", self.picture(&format!("{key}.picture.{n}"), picture))
            }
            Output::Value(Mime::Markdown, said) => {
                let style = MarkdownStyle::neutral(true).body_font(BODY);
                let body = Markdown::new(*said).selectable(true).render(&style);
                let value = col().key(format!("{key}.value")).w_full().child(body);
                self.beside("Out", value)
            }
            Output::Value(mime, said) => {
                let value = self.stream(&format!("{key}.value"), said);
                let named = match mime {
                    Mime::Other(named) => Some(format!("{named}, drawn as its text")),
                    Mime::Plain | Mime::Markdown | Mime::Json | Mime::Python => None,
                };
                let named = named.map(|named| text(named).font(META).color(colour(tokens.faint)));
                self.beside("Out", col().w_full().children(named).child(value))
            }
            Output::Error {
                name,
                value,
                traceback,
            } => {
                let first = text(format!("{name}: {value}"))
                    .font(CODE_LINE)
                    .color(colour(tokens.bad))
                    .wrap()
                    .selectable(true);
                // The traceback ends on the line drawn first: the name and the message.
                let last = traceback
                    .last()
                    .filter(|line| line.trim() == format!("{name}: {value}"));
                let lines =
                    traceback.get(..traceback.len().saturating_sub(usize::from(last.is_some())));
                let whole = lines.unwrap_or_default().join("\n");
                let trace = self.text(&format!("{key}.traceback"), &whole, tokens.dim);
                let error = col().key(format!("{key}.error")).w_full().gap(2.0);
                self.beside("", error.child(first).child(trace))
            }
            Output::Aborted => self.note(&format!("{key}.aborted"), "Stopped before it finished"),
        }
    }

    fn note(&self, key: &str, words: &str) -> El {
        let said = text(words).key(key.to_owned()).font(META);
        self.beside("", said.color(colour(self.tokens.warn)))
    }

    fn stream(&self, key: &str, said: &str) -> El {
        self.text(key, said.trim_end_matches('\n'), self.tokens.fg)
    }

    /// `said` in the code font, its first [`MAX_OUTPUT`] characters, with the cut named.
    fn text(&self, key: &str, said: &str, ink: Rgba) -> El {
        let tokens = self.tokens;
        let (shown, total) = head(said, MAX_OUTPUT);
        let body = text(shown)
            .font(CODE_LINE)
            .color(colour(ink))
            .wrap()
            .selectable(true);
        let cut = total.map(|total| {
            let words = format!(
                "[…] {MAX_OUTPUT} of {total} characters shown (the output cap, {MAX_OUTPUT}); press here to copy all {total}"
            );
            let whole = said.to_owned();
            text(words.clone())
                .key(format!("{key}.cut"))
                .font(META)
                .color(colour(tokens.faint))
                .wrap()
                .cursor(Cursor::Pointer)
                .label(words)
                .on_press_tx(move |_: &mut Win, tx: &mut Tx| tx.copy(whole.clone()))
        });
        col()
            .key(key.to_owned())
            .w_full()
            .gap(2.0)
            .child(body)
            .children(cut)
    }

    /// A picture fitted to the pane's width and kept to its aspect; a press shows it at one
    /// point a pixel, scrolling sideways when wider than the pane, and a second press fits it.
    fn picture(&self, key: &str, picture: &Picture<'_>) -> El {
        let Some((image, (w, h))) = load(key, picture) else {
            return self.note(key, "This picture did not decode");
        };
        let full = self.full.is_some_and(|full| full.contains(key));
        let (natural, ratio) = (w as f32, w as f32 / h.max(1) as f32);
        let (pane, toggled) = (self.pane, key.to_owned());
        let label = if full {
            "Fit the picture to the pane"
        } else {
            "Show the picture at full size"
        };
        let face = el().key(key.to_owned()).aspect_ratio(ratio);
        let face = if full {
            face.w(natural)
        } else {
            face.w_full().max_w(natural)
        };
        let face = face
            .pixels_fit(image.pixels(), SurfaceFit::Fit)
            .cursor(Cursor::Pointer)
            .label(label)
            .on_press_tx(move |win: &mut Win, _: &mut Tx| flip(win, pane, &toggled));
        if full {
            row().w_full().scroll_x().child(face)
        } else {
            face
        }
    }
}

fn flip(win: &mut Win, pane: PaneId, key: &str) {
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(book) = hub.panes.get_mut::<Notebook>(Some(pane))
        && !book.full.remove(key)
    {
        book.full.insert(key.to_owned());
    }
}

/// The first `cap` characters of `said`, and how many it has when that cut it.
fn head(said: &str, cap: usize) -> (&str, Option<usize>) {
    match said.char_indices().nth(cap) {
        Some((at, _)) => (said.get(..at).unwrap_or(said), Some(said.chars().count())),
        None => (said, None),
    }
}

/// A duration as a notebook's cell timer says it.
struct Duration(u64);

impl std::fmt::Display for Duration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0 < 1000 {
            return write!(f, "{} ms", self.0);
        }
        let tenths = self.0.saturating_add(50) / 100;
        write!(f, "{}.{} s", tenths / 10, tenths % 10)
    }
}

/// One picture kept decoded: its key, which body it is, its handle and its side.
struct Kept {
    key: String,
    body: (usize, String),
    image: Image,
    side: (u32, u32),
}

thread_local! {
    /// Invariant: at most `KEPT` pictures, oldest first; a view cannot keep one itself.
    static SHOWN: RefCell<VecDeque<Kept>> = const { RefCell::new(VecDeque::new()) };
    static CACHE: ImageCache = ImageCache::new(Arc::new(|| {}));
}

/// The picture decoded, found again while its key names the same body; its length and last
/// characters tell two bodies apart, since a faux model's call ids repeat across threads.
fn load(key: &str, picture: &Picture<'_>) -> Option<(Image, (u32, u32))> {
    let tail: String = picture.data.chars().rev().take(64).collect();
    let body = (picture.data.len(), tail);
    SHOWN.with(|kept| {
        let mut kept = kept.try_borrow_mut().ok()?;
        let same = |entry: &&Kept| entry.key == key && entry.body == body;
        if let Some(found) = kept.iter().find(same) {
            return Some((found.image.clone(), found.side));
        }
        let bytes = picture.bytes()?;
        let side = picture.side(&bytes)?;
        let inline = bytes.len() <= INLINE;
        let image = CACHE.with(|cache| cache.load(bytes));
        if inline {
            // A picture that will not decode draws nothing; its error is Rune's to keep.
            let _ = image.wait();
        }
        if kept.len() >= KEPT {
            kept.pop_front();
        }
        kept.push_back(Kept {
            key: key.to_owned(),
            body,
            image: image.clone(),
            side,
        });
        Some((image, side))
    })
}
