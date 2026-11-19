//! The diff as rows: a header per file, a 16 tall row per line, comments under their lines.

use std::path::PathBuf;

use rune::{Cursor, El, Key, KeyEvent, TextStyle, Tx, col, row, spacer, text};
use rune_kit::diff::LineKind;
use yd_core::address::thing::Thing;
use yd_core::address::uri::Address;
use yd_core::feed::SourceCmd;
use yd_core::ids::PaneId;
use yd_core::kind::Kind;
use yd_core::msg::Msg;
use yd_core::row::Row;
use yd_core::tokens::Tokens;
use yd_ui::grid_view::view::open_file;
use yd_ui::theme::look::{CODE, CODE_LINE, LABEL, META, NONE, colour, white};
use yd_ui::theme::scale::{FILE_HEAD, NOTE, SEMIBOLD};
use yd_ui::win::{Win, dispatch};

use super::{Draft, FEED, File, State, change, faint};

/// One drawn row. Lines are found by file, hunk and index in the hunk.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Item {
    Header(usize),
    Line {
        file: usize,
        hunk: usize,
        line: usize,
    },
    /// A hunk's revert question, under its first line.
    Asked {
        file: usize,
        hunk: usize,
    },
    /// A comment, by its place in the pane's comments.
    Comment(usize),
    Draft,
}

/// The rows in order: each file's header, its lines, and after a line its comments.
pub fn items(files: &[File], state: &State) -> Vec<Item> {
    let mut items = Vec::new();
    for (f, file) in files.iter().enumerate() {
        items.push(Item::Header(f));
        let Some(diff) = &file.diff else { continue };
        for (h, hunk) in diff.hunks.iter().enumerate() {
            for (l, line) in hunk.lines.iter().enumerate() {
                items.push(Item::Line {
                    file: f,
                    hunk: h,
                    line: l,
                });
                if l == 0 && state.confirm.as_ref() == Some(&(file.path.clone(), h)) {
                    items.push(Item::Asked { file: f, hunk: h });
                }
                let Some(at) = number(line.kind, line.old, line.new) else {
                    continue;
                };
                let here = |path: &str, n: u32| {
                    path == file.path && n == at && line.kind != LineKind::Removed
                };
                for (c, comment) in state.comments.iter().enumerate() {
                    let n = comment.words("line").and_then(|n| n.parse().ok());
                    if n.is_some_and(|n| here(comment.words("path").unwrap_or(""), n)) {
                        items.push(Item::Comment(c));
                    }
                }
                if state.draft.as_ref().is_some_and(|d| here(&d.path, d.line)) {
                    items.push(Item::Draft);
                }
            }
        }
    }
    items
}

/// The one number a line shows: a removed line its old number, any other its new one.
fn number(kind: LineKind, old: Option<u32>, new: Option<u32>) -> Option<u32> {
    if kind == LineKind::Removed { old } else { new }
}

/// What the row closures read: they run after the view has returned.
pub struct Draw {
    pub tokens: Tokens,
    pub key: String,
    pub pane: PaneId,
    pub files: Vec<File>,
    pub items: Vec<Item>,
    pub comments: Vec<Row>,
    pub draft: Option<Draft>,
}

impl Draw {
    pub fn key_at(&self, at: usize) -> String {
        let key = &self.key;
        match self.items.get(at) {
            Some(Item::Header(f)) => format!("{key}.row.{}", self.path(*f)),
            Some(Item::Line { file, hunk, line }) => {
                let n = self
                    .line(*file, *hunk, *line)
                    .and_then(|l| number(l.kind, l.old, l.new));
                format!(
                    "{key}.row.{}:{hunk}:{line}.L{}",
                    self.path(*file),
                    n.unwrap_or(0)
                )
            }
            Some(Item::Asked { file, hunk }) => format!("{key}.ask.{}:{hunk}", self.path(*file)),
            Some(Item::Comment(c)) => format!("{key}.comment.{c}"),
            Some(Item::Draft) | None => format!("{key}.draft"),
        }
    }

    fn path(&self, file: usize) -> &str {
        self.files.get(file).map_or("", |f| f.path.as_str())
    }

    fn line(&self, file: usize, hunk: usize, line: usize) -> Option<&rune_kit::diff::Line> {
        self.files
            .get(file)?
            .diff
            .as_ref()?
            .hunks
            .get(hunk)?
            .lines
            .get(line)
    }

    pub fn row_at(&self, at: usize) -> El {
        let key = self.key_at(at);
        match self.items.get(at) {
            Some(Item::Header(f)) => self.header(key, *f),
            Some(Item::Line { file, hunk, line }) => self.code(key, *file, *hunk, *line),
            Some(Item::Asked { file, hunk }) => self.asked(key, *file, *hunk),
            Some(Item::Comment(c)) => self.comment(key, *c),
            Some(Item::Draft) | None => self.drafting(key),
        }
    }

    fn header(&self, key: String, f: usize) -> El {
        let tokens = &self.tokens;
        let Some(file) = self.files.get(f) else {
            return col();
        };
        let fresh = file.status == "A";
        let (path, name) = (file.path.clone(), format!("{key}.path"));
        let counts = match (file.added.as_str(), file.removed.as_str()) {
            (added, "0") => format!("+{added}"),
            (added, removed) => format!("+{added} −{removed}"),
        };
        row()
            .key(key)
            .w_full()
            .h(20.0)
            .px(8.0)
            .gap(8.0)
            .center_y()
            .fill(colour(FILE_HEAD.of(tokens)))
            .child(
                text(file.path.as_str())
                    .key(name)
                    .font(CODE_LINE)
                    .color(colour(tokens.fg))
                    .cursor(Cursor::Pointer)
                    .on_press_tx(move |win: &mut Win, tx: &mut Tx| {
                        if let Ok(file) = Address::new(Kind::File, path.as_str()) {
                            open_file(win, tx, file);
                        }
                    }),
            )
            .children(fresh.then(|| faint(tokens, "new file")))
            .child(spacer())
            .child(
                text(counts)
                    .font(META.family(CODE))
                    .color(colour(tokens.dim)),
            )
    }

    /// A line: its number (press it to comment), its sign, its text, and on a hunk's first
    /// line the revert and its question.
    fn code(&self, key: String, file: usize, hunk: usize, at: usize) -> El {
        let tokens = &self.tokens;
        let Some(line) = self.line(file, hunk, at) else {
            return col();
        };
        let (sign, fill) = match line.kind {
            LineKind::Added => ("+", tokens.added),
            LineKind::Removed => ("-", tokens.removed),
            LineKind::Context => ("", NONE),
        };
        let n = number(line.kind, line.old, line.new);
        let (pane, path) = (self.pane, self.path(file).to_owned());
        let commentable = n.filter(|_| line.kind != LineKind::Removed);
        // The number is a pointer's way to a comment; a keyboard stop on every line would bury
        // the pane's own.
        let gutter = row()
            .focusable(false)
            .w(28.0)
            .pr(8.0)
            .justify_end()
            .cursor(Cursor::Pointer)
            .child(faint(tokens, &n.map(|n| n.to_string()).unwrap_or_default()).font(CODE_LINE))
            .on_press(move |win: &mut Win| {
                if let Some(line) = commentable {
                    change(win, pane, |s| {
                        s.draft = Some(Draft {
                            path: path.clone(),
                            line,
                            words: String::new(),
                            hunk: None,
                        })
                    });
                }
            });
        // A long line wraps: the pane never hides code past its edge, and a hunk's actions go
        // under its first line before that line is squeezed under 60 points.
        row()
            .key(key)
            .w_full()
            .min_w(0.0)
            .flex_wrap()
            .justify_end()
            .min_h(16.0)
            .fill(colour(fill))
            .child(gutter)
            .child(row().w(11.0).child(faint(tokens, sign).font(CODE_LINE)))
            .child(
                text(line.text.as_str())
                    .font(CODE_LINE)
                    .color(colour(tokens.fg))
                    .wrap_anywhere()
                    .grow(1.0)
                    .min_w(60.0_f32.min(rune::text::advance(&line.text, CODE_LINE))),
            )
            .children((at == 0).then(|| self.controls(file, hunk)))
    }

    /// Invariant: a new file is never offered a revert: putting it back deletes it, and git
    /// keeps no copy.
    fn fresh(&self, file: usize) -> bool {
        self.files.get(file).is_none_or(|f| f.status == "A")
    }

    /// The hunk's lines as the source's `body` spells them, so it reverts only what was shown.
    fn shown(&self, file: usize, hunk: usize) -> Option<String> {
        let hunk = self.files.get(file)?.diff.as_ref()?.hunks.get(hunk)?;
        let lines = hunk.lines.iter().map(|line| {
            let sign = match line.kind {
                LineKind::Added => "+",
                LineKind::Removed => "-",
                LineKind::Context => " ",
            };
            [sign, line.text.as_str()].concat()
        });
        Some(lines.collect::<Vec<_>>().join("\n"))
    }

    /// The lines a hunk spans in the file as it is now, as an address part: `L3-L9`.
    fn span(&self, file: usize, hunk: usize) -> Option<(u32, u32)> {
        let hunk = self.files.get(file)?.diff.as_ref()?.hunks.get(hunk)?;
        let kept = hunk
            .lines
            .iter()
            .filter(|line| line.kind != LineKind::Removed);
        let numbers: Vec<u32> = kept.filter_map(|line| line.new).collect();
        Some((*numbers.iter().min()?, *numbers.iter().max()?))
    }

    /// A hunk's first line ends in its two actions (plan default 17): comment, and revert.
    fn controls(&self, file: usize, hunk: usize) -> El {
        let tokens = &self.tokens;
        let (pane, path) = (self.pane, self.path(file).to_owned());
        let shown = self.shown(file, hunk).unwrap_or_default();
        let draft = format!("{}.draft", self.key);
        let comment = self.span(file, hunk).map(|(first, last)| {
            let part = match first == last {
                true => format!("L{first}"),
                false => format!("L{first}-L{last}"),
            };
            let (path, hunk) = (path.clone(), Some((part, shown)));
            control("Comment", tokens.faint).on_press_tx(move |win: &mut Win, tx: &mut Tx| {
                let draft_of = Draft {
                    path: path.clone(),
                    line: first,
                    words: String::new(),
                    hunk: hunk.clone(),
                };
                change(win, pane, |s| s.draft = Some(draft_of));
                tx.focus_when_shown(draft.clone(), false);
            })
        });
        let revert = (!self.fresh(file)).then(|| {
            control("Revert", tokens.faint).on_press(move |win: &mut Win| {
                change(win, pane, |s| s.confirm = Some((path.clone(), hunk)));
            })
        });
        row()
            .pl(6.0)
            .pr(8.0)
            .gap(8.0)
            .children(comment)
            .children(revert)
    }

    /// The question a pressed Revert waits on, on a row of its own so it wraps.
    fn asked(&self, key: String, file: usize, hunk: usize) -> El {
        let tokens = &self.tokens;
        let (pane, path) = (self.pane, self.path(file).to_owned());
        let shown = self.shown(file, hunk);
        let button = control;
        let yes = button("Put back", tokens.bad).on_press(move |win: &mut Win| {
            change(win, pane, |s| s.confirm = None);
            if let Some(hunk) = shown.clone() {
                let file = PathBuf::from(&path);
                dispatch(win, Msg::Source(FEED, SourceCmd::Revert { file, hunk }));
            }
        });
        let no = button("Keep", tokens.dim)
            .on_press(move |win: &mut Win| change(win, pane, |s| s.confirm = None));
        let question = faint(tokens, "Put these lines back as they were?");
        row()
            .key(key)
            .w_full()
            .min_w(0.0)
            .flex_wrap()
            .pl(39.0)
            .pr(8.0)
            .py(2.0)
            .gaps(2.0, 8.0)
            .child(question.wrap().shrink().min_w(0.0))
            .child(yes)
            .child(no)
    }

    fn note(&self, key: String) -> El {
        row()
            .key(key)
            .mt(2.0)
            .mb(2.0)
            .ml(36.0)
            .mr(6.0)
            .px(6.0)
            .py(2.0)
            .radius(7.0)
            .fill(colour(NOTE.of(&self.tokens)))
            .border(1.0, colour(white(self.tokens.lines)))
    }

    /// A comment box: who and when on the first line, the words under it with `code` set off.
    fn comment(&self, key: String, c: usize) -> El {
        let tokens = &self.tokens;
        let Some(comment) = self.comments.get(c) else {
            return col();
        };
        let cell = |field| comment.words(field).unwrap_or("");
        let about = [
            format!("on line {}", cell("line")),
            cell("at").to_owned(),
            cell("status").to_owned(),
            cell("finding").to_owned(),
        ];
        let about: Vec<String> = about.into_iter().filter(|w| !w.is_empty()).collect();
        let who = row()
            .gap(4.0)
            .child(
                text(cell("author"))
                    .font(TextStyle {
                        weight: SEMIBOLD,
                        ..LABEL
                    })
                    .color(colour(tokens.fg)),
            )
            .child(
                text(about.join(" · "))
                    .font(LABEL)
                    .color(colour(tokens.faint))
                    .wrap()
                    .shrink(),
            );
        let words = cell("body").split('`').enumerate().map(|(i, part)| {
            if i % 2 == 0 {
                text(part)
                    .font(LABEL)
                    .color(colour(tokens.dim))
                    .wrap()
                    .shrink()
            } else {
                text(part)
                    .font(TextStyle {
                        size: 9.7,
                        ..CODE_LINE
                    })
                    .color(colour(tokens.dim))
                    .fill(colour(tokens.raised))
                    .radius(4.0)
                    .px(5.0)
                    .wrap_anywhere()
                    .shrink()
            }
        });
        self.note(key).child(
            col()
                .gap(1.0)
                .shrink()
                .child(who)
                .child(row().children(words)),
        )
    }

    /// The comment being written: typed here, Return keeps it and attaches it to the next
    /// prompt, Esc drops it.
    fn drafting(&self, key: String) -> El {
        let tokens = &self.tokens;
        let words = self.draft.as_ref().map_or("", |d| d.words.as_str());
        let shown = if words.is_empty() {
            faint(tokens, "Comment on this line; ↵ adds it to the next prompt")
        } else {
            text(words).font(LABEL).color(colour(tokens.fg))
        };
        let shown = shown.wrap().shrink();
        let pane = self.pane;
        self.note(key)
            .focusable(true)
            .on_key(move |win: &mut Win, event: &KeyEvent, tx: &mut Tx| typed(win, pane, event, tx))
            .child(shown)
            .child(col().w(1.0).h(12.0).fill(colour(tokens.accent)))
    }
}

/// A word a person presses: faint 9.5 with a pointer.
fn control(words: &str, ink: yd_core::tokens::Rgba) -> El {
    text(words)
        .font(META)
        .color(colour(ink))
        .cursor(Cursor::Pointer)
}

/// A key in the comment box.
fn typed(win: &mut Win, pane: PaneId, event: &KeyEvent, _tx: &mut Tx) -> bool {
    let mut kept = None;
    match &event.key {
        Key::Text(words) if !event.mods.cmd && !event.mods.ctrl => {
            change(win, pane, |s| {
                s.draft.iter_mut().for_each(|d| d.words.push_str(words))
            });
        }
        Key::Backspace => change(win, pane, |s| {
            s.draft.iter_mut().for_each(|d| {
                d.words.pop();
            });
        }),
        Key::Escape => change(win, pane, |s| s.draft = None),
        Key::Enter => change(win, pane, |s| {
            // A hunk is sent with no note too: its lines say what is asked about.
            let said = |d: &Draft| !d.words.is_empty() || d.hunk.is_some();
            if let Some(draft) = s.draft.take().filter(said) {
                let id = format!("comment-{}", s.comments.len().saturating_add(1));
                let row = Row::new(id)
                    .text("path", draft.path.as_str())
                    .text("line", draft.line.to_string())
                    .text("author", "You")
                    .text("status", "open")
                    .text("body", draft.words.as_str());
                if !draft.words.is_empty() {
                    s.comments.push(row);
                }
                kept = Some(draft);
            }
        }),
        _ => return false,
    }
    if let Some(draft) = kept {
        attach(win, &draft);
    }
    true
}

/// Sends a kept comment with the next prompt: the lines it is about, its words as the excerpt.
fn attach(win: &mut Win, draft: &Draft) {
    let Ok(address) = Address::new(Kind::Lines, draft.path.as_str()) else {
        return;
    };
    let name = draft
        .path
        .rsplit_once('/')
        .map_or(draft.path.as_str(), |(_, name)| name);
    let (part, lines) = match &draft.hunk {
        Some((part, lines)) => (part.clone(), Some(lines)),
        None => (format!("L{}", draft.line), None),
    };
    let numbers = part.replace('L', "");
    let label = format!("{name}:{numbers}");
    // Incident: the words alone above the code read as part of it; the agent said only the
    // code reached it.
    let line = if numbers.contains('-') {
        "lines"
    } else {
        "line"
    };
    let mine = format!(
        "My review comment on {name} {line} {numbers}: \"{}\"",
        draft.words
    );
    let excerpt = match (draft.words.is_empty(), lines) {
        (false, Some(lines)) => format!("{mine}\nThe lines it is about, as they are now:\n{lines}"),
        (true, Some(lines)) => lines.clone(),
        (_, None) => mine,
    };
    let excerpt = Some(excerpt);
    let thing = Thing {
        label,
        address: address.part(part),
        excerpt,
    };
    dispatch(
        win,
        Msg::Attach(yd_core::address::attach::AttachMsg::Toggle(thing)),
    );
}
