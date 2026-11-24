//! Syntax colour, in pieces past the 128 KiB cap, on a worker thread for a long text, and
//! the texts made ready to draw, kept so a view finds its text again.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, OnceLock};

use rune_kit::syntax::{Paint, Syntax, builtin};

use super::doc::{Cols, Doc, Lang, LineNo};

/// Invariant: `rune_kit::syntax` colours only the first 128 KiB of one source (its
/// `MAX_SOURCE`), cut at a line end; a longer source is lexed here in pieces of this size.
const RUNE_CAP: usize = 128 * 1024;

/// Lines lexed again ahead of a piece's first new line, shortest first, so the lexer has left
/// any comment or string the cut fell in before its output is used.
const OVERLAPS: [usize; 3] = [48, 192, 768];

/// Bytes coloured on the UI thread. Incident: 300 KiB took 2.2 ms in the spike, so this much
/// is about half a frame; a longer text draws plain until a worker has coloured it.
pub const INLINE_MAX: usize = 512 * 1024;

/// Texts kept ready to draw: the open files of a few panes.
const KEPT: usize = 8;

/// A coloured run of one line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tok {
    pub cols: Cols,
    pub kind: Syntax,
}

/// The coloured runs of every line of a text.
#[derive(Debug, Clone, Default)]
pub struct Colours {
    toks: Vec<Tok>,
    /// Where each line's runs start in `toks`.
    first: Vec<usize>,
    pieces: usize,
    seams: usize,
}

impl Colours {
    /// Colours `doc` with Rune's lexers, past their 128 KiB cap.
    pub fn of(doc: &Doc) -> Colours {
        Colours::in_pieces(doc, RUNE_CAP)
    }

    /// The coloured runs of `line`, in order.
    pub fn line(&self, line: LineNo) -> &[Tok] {
        self.at(line.index())
    }

    /// How many sources the text was lexed as.
    pub fn pieces(&self) -> usize {
        self.pieces
    }

    /// Cuts where the lexer never agreed with the piece before: colour may be wrong after one,
    /// up to the end of the comment or string the cut fell in.
    pub fn seams(&self) -> usize {
        self.seams
    }

    fn at(&self, i: usize) -> &[Tok] {
        let Some(&a) = self.first.get(i) else {
            return &[];
        };
        let b = self
            .first
            .get(i.saturating_add(1))
            .copied()
            .unwrap_or(self.toks.len());
        self.toks.get(a..b).unwrap_or(&[])
    }

    fn in_pieces(doc: &Doc, limit: usize) -> Colours {
        let mut out = Colours {
            first: Vec::with_capacity(doc.lines()),
            ..Colours::default()
        };
        while out.first.len() < doc.lines() {
            let done = out.first.len();
            let (from, piece, joined) = out.next_piece(doc, done, limit);
            for i in done.saturating_sub(from)..piece.first.len() {
                out.first.push(out.toks.len());
                out.toks.extend_from_slice(piece.at(i));
            }
            out.pieces = out.pieces.saturating_add(1);
            out.seams = out.seams.saturating_add(usize::from(!joined));
        }
        out
    }

    /// The piece that colours lines from `done` on: where it starts, its runs, and whether it
    /// agreed with the lines already coloured.
    fn next_piece(&self, doc: &Doc, done: usize, limit: usize) -> (usize, Colours, bool) {
        let mut last = None;
        for overlap in OVERLAPS {
            let from = done.saturating_sub(overlap);
            let to = doc.lines_within(from, limit);
            if to <= done {
                continue;
            }
            let piece = Colours::lexed(doc, from, to);
            let check = done.saturating_sub(done.saturating_sub(from).div_ceil(2))..done;
            let same = check
                .clone()
                .all(|i| piece.at(i.saturating_sub(from)) == self.at(i));
            if from == 0 || (same && check.clone().any(|i| !self.at(i).is_empty())) {
                return (from, piece, true);
            }
            last = Some((from, piece, false));
        }
        last.unwrap_or_else(|| {
            (
                done,
                Colours::lexed(doc, done, doc.lines_within(done, limit)),
                done == 0,
            )
        })
    }

    /// The runs of 0-based lines `from..to`, lexed as one source.
    fn lexed(doc: &Doc, from: usize, to: usize) -> Colours {
        let mut out = Colours {
            first: Vec::with_capacity(to.saturating_sub(from)),
            ..Colours::default()
        };
        let base = doc.span(from, to).start;
        let found = builtin(doc.lang().name(), doc.slice(from, to));
        let mut found = found.iter().peekable();
        for i in from..to {
            out.first.push(out.toks.len());
            let line = doc.span(i, i.saturating_add(1));
            while let Some((range, paint)) = found.peek() {
                let (start, end) = (
                    range.start.saturating_add(base),
                    range.end.saturating_add(base),
                );
                if start >= line.end {
                    break;
                }
                let a = u32::try_from(start.max(line.start).saturating_sub(line.start));
                let b = u32::try_from(end.min(line.end).saturating_sub(line.start));
                if let (Paint::Kind(kind), Ok(a), Ok(b)) = (paint, a, b) {
                    out.toks.push(Tok {
                        cols: Cols::new(a, b),
                        kind: *kind,
                    });
                }
                if end > line.end {
                    break;
                }
                found.next();
            }
        }
        out
    }
}

/// Something to call once colours a worker made are ready: it makes the view run again.
pub type Wake = Box<dyn FnOnce() + Send>;

/// The colours of one text, which a worker thread may still be making.
#[derive(Debug, Clone, Default)]
pub struct Pending(Arc<OnceLock<Colours>>);

impl Pending {
    /// Colours `doc` here when it is short or nobody can be woken, else on a worker thread
    /// that calls `wake` when it is done. Until then the text draws plain.
    pub fn of(doc: &Arc<Doc>, wake: Option<Wake>) -> Pending {
        Pending::past(doc, wake, INLINE_MAX)
    }

    /// The colours, once they are made.
    pub fn get(&self) -> Option<&Colours> {
        self.0.get()
    }

    fn past(doc: &Arc<Doc>, wake: Option<Wake>, inline: usize) -> Pending {
        let pending = Pending::default();
        let Some(wake) = wake.filter(|_| doc.text().len() > inline) else {
            pending.fill(doc);
            return pending;
        };
        let (slot, text) = (pending.clone(), doc.clone());
        // Invariant: the worker owns its text and only fills the slot, so it is never joined;
        // one that outlives the window colours a text nobody draws.
        let worker = std::thread::Builder::new()
            .name("yd-colour".to_owned())
            .spawn(move || {
                slot.fill(&text);
                wake();
            });
        if worker.is_err() {
            pending.fill(doc);
        }
        pending
    }

    fn fill(&self, doc: &Doc) {
        self.0.get_or_init(|| Colours::of(doc));
    }
}

/// A text ready to draw: its lines, and its colours when they are made.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub doc: Arc<Doc>,
    pub colours: Pending,
}

struct Kept {
    id: String,
    stamp: u64,
    digest: u64,
    loaded: Loaded,
}

thread_local! {
    /// Invariant: at most `KEPT` texts, oldest first; a view cannot keep one itself.
    static LOADED: RefCell<VecDeque<Kept>> = const { RefCell::new(VecDeque::new()) };
}

/// The text `id` ready to draw, made once and found again while it is the same text. `stamp`
/// is anything that moves when the text may have changed, such as its table's version.
pub fn load(id: &str, stamp: u64, text: &str, lang: Lang, wake: Option<Wake>) -> Loaded {
    LOADED.with(|kept| {
        let Ok(mut kept) = kept.try_borrow_mut() else {
            return fresh(text, lang, digest(id, text, lang), wake);
        };
        let at = kept.iter().position(|entry| entry.id == id);
        if let Some(entry) = at.and_then(|at| kept.get_mut(at)) {
            if entry.stamp == stamp {
                return entry.loaded.clone();
            }
            let digest = digest(id, text, lang);
            if entry.digest != digest {
                (entry.digest, entry.loaded) = (digest, fresh(text, lang, digest, wake));
            }
            entry.stamp = stamp;
            return entry.loaded.clone();
        }
        let digest = digest(id, text, lang);
        let loaded = fresh(text, lang, digest, wake);
        if kept.len() >= KEPT {
            kept.pop_front();
        }
        kept.push_back(Kept {
            id: id.to_owned(),
            stamp,
            digest,
            loaded: loaded.clone(),
        });
        loaded
    })
}

/// A text's revision. It holds the id, so two files with the same text do not share rows.
fn digest(id: &str, text: &str, lang: Lang) -> u64 {
    let mut hasher = DefaultHasher::new();
    (id, text, lang).hash(&mut hasher);
    hasher.finish()
}

fn fresh(text: &str, lang: Lang, digest: u64, wake: Option<Wake>) -> Loaded {
    let doc = Arc::new(Doc::new(text, lang, digest));
    let colours = Pending::of(&doc, wake);
    Loaded { doc, colours }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    use rune_kit::syntax::Syntax;

    use super::{Colours, Pending, RUNE_CAP, Wake, load};
    use crate::code::doc::{Doc, Lang, LineNo};
    use crate::code::view::sample;

    type Check = Result<(), Box<dyn std::error::Error>>;

    /// The text and kind of each coloured run of 0-based `line`.
    fn runs(colours: &Colours, doc: &Doc, line: usize) -> Vec<(String, Syntax)> {
        let no = LineNo::from_index(line);
        let text = doc.line(no).unwrap_or("");
        colours
            .line(no)
            .iter()
            .filter_map(|t| Some((text.get(t.cols.within(text)?)?.to_owned(), t.kind)))
            .collect()
    }

    #[test]
    fn a_300_kib_file_is_coloured_to_its_last_line() -> Check {
        let doc = Doc::new(sample::rust(0, 300 * 1024), Lang::RUST, 0);
        assert!(doc.text().len() >= 300 * 1024);
        let colours = Colours::of(&doc);
        let plain: Vec<usize> = (0..doc.lines())
            .filter(|&i| {
                doc.line(LineNo::from_index(i))
                    .is_some_and(|l| l.starts_with("pub fn "))
            })
            .filter(|&i| {
                runs(&colours, &doc, i).first() != Some(&("pub fn ".to_owned(), Syntax::Keyword))
            })
            .collect();
        assert_eq!(
            plain.len(),
            0,
            "{} `pub fn` lines drew plain, the first at line {:?}",
            plain.len(),
            plain.first()
        );
        assert_eq!((colours.pieces(), colours.seams()), (3, 0));
        Ok(())
    }

    /// The reference is Rune's own single pass over a source short enough for it.
    #[test]
    fn pieces_colour_every_line_as_one_pass_does() -> Check {
        let sources = [
            (Lang::RUST, sample::rust(2400, 0)),
            (Lang::PYTHON, sample::PYTHON.repeat(160)),
            (Lang::TYPESCRIPT, sample::TYPESCRIPT.repeat(160)),
            (Lang::TOML, sample::TOML.repeat(160)),
            (Lang::SHELL, sample::SHELL.repeat(160)),
            (Lang::JSON, sample::JSON.repeat(160)),
        ];
        for (lang, text) in sources {
            assert!(text.len() < RUNE_CAP, "{lang:?} is {} bytes", text.len());
            let doc = Doc::new(text, lang, 0);
            let whole = Colours::in_pieces(&doc, RUNE_CAP);
            assert_eq!(whole.pieces(), 1);
            for limit in [3000, 4096, 5000, 7777, 16 * 1024] {
                let pieces = Colours::in_pieces(&doc, limit);
                assert!(
                    pieces.pieces() > 1,
                    "{lang:?} at {limit}: {} pieces",
                    pieces.pieces()
                );
                let differ =
                    (0..doc.lines()).find(|&i| runs(&pieces, &doc, i) != runs(&whole, &doc, i));
                let shown = differ.map(|i| {
                    (
                        i,
                        doc.line(LineNo::from_index(i)),
                        runs(&pieces, &doc, i),
                        runs(&whole, &doc, i),
                    )
                });
                assert_eq!(
                    shown, None,
                    "{lang:?} in pieces of {limit} bytes differs from one pass"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn a_file_is_coloured_in_the_language_its_name_says() -> Check {
        // Each sample's comment; JSON has none, so one of its strings.
        let note = Syntax::Comment;
        let (rust, ts) = (sample::RATE_LIMIT, sample::TYPESCRIPT);
        let files = [
            (
                "bucket.rs",
                rust,
                "/// Twenty uploads a minute for each user.",
                note,
            ),
            (
                "bucket.ts",
                ts,
                "/** Twenty uploads a minute for each user. */",
                note,
            ),
            (
                "bucket.py",
                sample::PYTHON,
                "# One token for each upload.",
                note,
            ),
            (
                "Cargo.toml",
                sample::TOML,
                "# Rate limits live in the middleware.",
                note,
            ),
            (
                "upload.sh",
                sample::SHELL,
                "# Twenty uploads, then one more.",
                note,
            ),
            (
                "package.json",
                sample::JSON,
                "\"atlas-api\"",
                Syntax::String,
            ),
        ];
        for (path, text, words, kind) in files {
            let doc = Doc::new(text, Lang::for_path(path), 0);
            let holds = |i: &usize| {
                doc.line(LineNo::from_index(*i))
                    .is_some_and(|l| l.contains(words))
            };
            let line = (0..doc.lines())
                .find(holds)
                .ok_or("the sample lost its line")?;
            let found = runs(&Colours::of(&doc), &doc, line);
            assert!(
                found.contains(&(words.to_owned(), kind)),
                "{path}: line {line} is {found:?}"
            );
        }
        let plain = Doc::new("no colour here\n", Lang::for_path("notes.unknown"), 0);
        assert_eq!(runs(&Colours::of(&plain), &plain, 0), []);
        Ok(())
    }

    #[test]
    fn a_long_text_is_coloured_on_a_worker_that_wakes_the_view_when_done() -> Check {
        let doc = Arc::new(Doc::new(sample::rust(400, 0), Lang::RUST, 0));
        let (done, woken) = channel();
        let wake: Wake = Box::new(move || {
            let _ = done.send(std::thread::current().name().map(str::to_owned));
        });
        let pending = Pending::past(&doc, Some(wake), 1024);
        let worker = woken.recv_timeout(Duration::from_secs(20))?;
        assert_eq!(worker.as_deref(), Some("yd-colour"));
        let made = pending.get().ok_or("woken before the colours were there")?;
        let here = Colours::of(&doc);
        for i in 0..doc.lines() {
            assert_eq!(runs(made, &doc, i), runs(&here, &doc, i), "line {i}");
        }

        let (done, woken) = channel();
        let wake: Wake = Box::new(move || {
            let _ = done.send(());
        });
        let short = Pending::past(&doc, Some(wake), usize::MAX);
        assert!(short.get().is_some(), "a short text is coloured at once");
        assert!(woken.try_recv().is_err(), "and nobody is woken for it");
        assert!(Pending::past(&doc, None, 0).get().is_some());
        Ok(())
    }

    #[test]
    fn a_text_is_made_once_and_made_again_only_when_it_changes() -> Check {
        let first = load("test:kept", 1, "let a = 1;\n", Lang::RUST, None);
        let again = load("test:kept", 1, "let a = 1;\n", Lang::RUST, None);
        assert!(Arc::ptr_eq(&first.doc, &again.doc));
        let moved = load("test:kept", 2, "let a = 1;\n", Lang::RUST, None);
        assert!(
            Arc::ptr_eq(&first.doc, &moved.doc),
            "a new stamp over the same text keeps the doc"
        );
        let changed = load("test:kept", 3, "let a = 2;\n", Lang::RUST, None);
        assert_eq!(changed.doc.text(), "let a = 2;\n");
        assert_ne!(changed.doc.rev(), first.doc.rev());
        for i in 0..20 {
            load(&format!("test:other-{i}"), 0, "x", Lang::PLAIN, None);
        }
        let back = load("test:kept", 3, "let a = 2;\n", Lang::RUST, None);
        assert!(
            !Arc::ptr_eq(&changed.doc, &back.doc),
            "only a few texts are kept"
        );
        Ok(())
    }
}
