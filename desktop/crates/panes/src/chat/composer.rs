//! The composer: the draft, the model, effort and mode chips, send and stop, and what is
//! queued. One box in two sizes: the pane's, and the reading size of the document and New chat.

use std::path::Path;

use rune::{
    Cursor, El, FileDialogOptions, Icon, KeyEvent, PopupItem, PopupMenu, TextEdit, TextStyle, Tx,
    auto_grow, col, el, row, spacer, text, text_input,
};
use yd_core::address::attach::AttachMsg;
use yd_core::address::thing::Thing;
use yd_core::address::uri::Address;
use yd_core::command::table::{Route, Run, Target};
use yd_core::feed::{FILES, Table};
use yd_core::ids::{CommandId, PaneId};
use yd_core::kind::Kind;
use yd_core::msg::Msg;
use yd_core::reduce::step::commands::{CANCEL, CHILD_ANSWER, NEW, PROMPT, SEND_QUEUED, SET_OPTION};
use yd_core::reduce::threads::{Run as State, Thread};
use yd_core::row::Rows;
use yd_core::tokens::Tokens;
use yd_ui::a11y;
use yd_ui::kit::chip::{Chip, chip};
use yd_ui::kit::icons::glyph;
use yd_ui::line_icon;
use yd_ui::pane::PaneCx;
use yd_ui::shown::DRAFT;
use yd_ui::theme::look::{BODY, META, READ_PROSE, colour};
use yd_ui::theme::scale::{COMPOSER, COMPOSER_PANE, LINE_MENU, RADIUS_BUTTON, RADIUS_COMPOSER};
use yd_ui::win::{Hub, Win, dispatch, field};

use crate::chat::approval;
use crate::chat::messages::line;
use crate::chat::pane::shown;

use crate::copy::labels;

static PLUS: Icon = line_icon!("plus", r#"<path d="M12 5v14M5 12h14"/>"#);
static SEND: Icon = line_icon!("send", r#"<path d="M12 19V5M6 11l6-6 6 6"/>"#);

/// Chips in the reading-size box: 11.5 dim on 22.
const CHIP: TextStyle = TextStyle::new(11.5, 400.0).lh(16.0);

/// The words being written in one pane, as last changed: kept in the hub so they travel
/// with the pane. The window that edits them holds the field itself (`Win.edits`).
#[derive(Clone, Default)]
pub struct Draft(pub TextEdit);

/// Where a sent draft goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Send {
    /// A prompt to the thread the pane shows.
    Prompt,
    /// The first prompt of a new thread in this folder.
    New(String),
    /// The answer to the question the child agent at this address waits on.
    Answer(Address),
}

/// How the box is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Form {
    /// In a pane: 10.5 type, the model and mode pickers, a 22 button.
    Pane,
    /// Reading size, led by Attach, with `gap` between words and chips.
    Reading { gap: f32 },
}

/// Whether the window's field is the one to trust: it holds the keyboard, or it still says
/// what the hub last heard. Otherwise another window changed the draft since.
fn current_edit(own: &TextEdit, kept: Option<&Draft>) -> bool {
    own.focused || kept.is_none_or(|kept| kept.0.text == own.text)
}

/// The field Rune edits: this window's own, seeded from the hub's draft.
fn edit(win: &mut Win, pane: PaneId) -> &mut TextEdit {
    let Win { hub, edits, .. } = win;
    let hub = hub.try_borrow().ok();
    let kept = hub
        .as_ref()
        .and_then(|hub| hub.panes.get::<Draft>(Some(pane)));
    let own = edits.entry(field(pane, DRAFT)).or_default();
    if !current_edit(own, kept) {
        *own = kept.map(|kept| kept.0.clone()).unwrap_or_default();
    }
    own
}

/// The field to draw. Invariant: Rune keeps focus and caret in the edit the view draws, so
/// this is the one `on_edit` returns whenever the window has it.
pub fn drawn<'a>(pane: &PaneCx<'a>) -> Option<&'a TextEdit> {
    let kept = pane.state::<Draft>();
    let own = pane
        .edits
        .and_then(|edits| edits.get(&field(pane.pane, DRAFT)));
    own.filter(|own| current_edit(own, kept))
        .or(kept.map(|kept| &kept.0))
}

/// Copies the window's field to the hub after each change, so it travels with the pane.
fn sync(win: &mut Win, pane: PaneId) {
    let draft = Draft(edit(win, pane).clone());
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(kept) = hub.panes.get_mut::<Draft>(Some(pane))
    {
        *kept = draft;
    }
}

/// The run `command` makes of the pane's thread, or of a new thread in the folder.
fn aimed(
    win: &Win,
    pane: PaneId,
    to: &Send,
    command: CommandId,
    words: Option<String>,
) -> Option<Run> {
    let target = match to {
        Send::Prompt => shown(&*win.hub.try_borrow().ok()?, pane)?.address().ok()?,
        Send::New(folder) => Address::new(Kind::Place, folder.as_str()).ok()?,
        Send::Answer(child) => child.clone(),
    };
    Some(Run {
        command,
        target: Target::Thing(target),
        text: words,
    })
}

fn thread_run(win: &Win, pane: PaneId, command: CommandId, words: Option<String>) -> Option<Run> {
    aimed(win, pane, &Send::Prompt, command, words)
}

/// Sends the draft: a prompt, which the server queues while a turn runs, or a new thread.
/// With nothing written it sends the prompts a Stop held.
fn send(win: &mut Win, pane: PaneId, to: &Send) {
    let words = edit(win, pane).text.trim().to_owned();
    if words.is_empty() {
        if *to == Send::Prompt {
            send_queued(win, pane);
        }
        return;
    }
    let command = match to {
        Send::Prompt => PROMPT,
        Send::New(_) => NEW,
        Send::Answer(_) => CHILD_ANSWER,
    };
    let Some(run) = aimed(win, pane, to, command, Some(words)) else {
        return;
    };
    edit(win, pane).clear();
    sync(win, pane);
    if let Send::New(folder) = to {
        crate::new_chat::view::record(win, folder);
        yd_ui::shown::sent(win, pane, run.text.clone().unwrap_or_default());
    }
    dispatch(win, Msg::Command(Route::Wire, run));
    // What was attached went with these words; it does not go with the next ones too.
    dispatch(win, Msg::Attach(AttachMsg::Clear));
}

/// Opens the Mac's file chooser; the file chosen is attached to the next prompt.
fn attach(_: &mut Win, tx: &mut Tx) {
    let options = FileDialogOptions {
        title: "Attach a File".to_owned(),
        ..FileDialogOptions::default()
    };
    tx.open_file(options, |win: &mut Win, chosen, _tx: &mut Tx| {
        let chosen = chosen.ok().flatten();
        let Some(path) = chosen.as_deref().and_then(Path::to_str) else {
            return;
        };
        let Ok(address) = Address::new(Kind::File, path) else {
            return;
        };
        let label = path.rsplit('/').next().unwrap_or(path).to_owned();
        let thing = Thing {
            label,
            address,
            excerpt: None,
        };
        dispatch(win, Msg::Attach(AttachMsg::Toggle(thing)));
    });
}

fn send_queued(win: &mut Win, pane: PaneId) {
    if let Some(run) = thread_run(win, pane, SEND_QUEUED, None) {
        dispatch(win, Msg::Command(Route::Wire, run));
    }
}

fn stop(win: &mut Win, pane: PaneId) {
    if let Some(run) = thread_run(win, pane, CANCEL, None) {
        dispatch(win, Msg::Command(Route::Wire, run));
    }
}

/// With nothing written, the keys go to the approval card; while an `@` word is being written,
/// Return attaches the first file it finds. Esc in New chat's own composer leaves the page.
fn keys(win: &mut Win, pane: PaneId, event: &KeyEvent, page: bool) -> bool {
    if page && event.key == rune::Key::Escape {
        return yd_ui::shown::leave(win);
    }
    if event.key == rune::Key::Enter {
        let draft = edit(win, pane).text.clone();
        let hub = win.hub.try_borrow().ok();
        let files = hub.as_ref().and_then(|hub| hub.model.tables.get(FILES));
        let first = files.and_then(|files| found(files, &draft).into_iter().next());
        drop(hub);
        if let Some(path) = first {
            pick(win, pane, &path);
            return true;
        }
    }
    // New chat shows no ask's card, so its keys answer none.
    !page && edit(win, pane).text.is_empty() && approval::key(win, pane, event)
}

/// Files of the shown thread's project a finder lists at most.
const MAX_FOUND: usize = 6;

/// The word being written after an `@`, when the draft ends in one.
fn asking(draft: &str) -> Option<&str> {
    let last = draft.rsplit(char::is_whitespace).next()?;
    last.strip_prefix('@')
}

/// The project's files whose path holds the `@` word, shortest first.
fn found(files: &Table, draft: &str) -> Vec<String> {
    let Some(word) = asking(draft).map(str::to_lowercase) else {
        return Vec::new();
    };
    let rows = (0..files.len()).filter_map(|n| files.get(n));
    let rows = rows.filter(|row| row.words("kind") == Some("file"));
    let mut paths: Vec<&str> = rows
        .map(|row| row.id.as_str())
        .filter(|path| path.to_lowercase().contains(&word))
        .collect();
    paths.sort_by_key(|path| (path.len(), *path));
    paths
        .into_iter()
        .take(MAX_FOUND)
        .map(str::to_owned)
        .collect()
}

/// Attaches the file `path` and takes its `@` word out of the draft.
fn pick(win: &mut Win, pane: PaneId, path: &str) {
    let Ok(address) = Address::new(Kind::File, path) else {
        return;
    };
    let text = edit(win, pane).text.clone();
    let kept = text
        .rsplit_once('@')
        .map_or("", |(kept, _)| kept)
        .to_owned();
    // The hub's copy first: a field that lost the keyboard is seeded again from it.
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(draft) = hub.panes.get_mut::<Draft>(Some(pane))
    {
        draft.0.replace_text(kept.clone());
    }
    let draft = edit(win, pane);
    let end = kept.len();
    draft.replace_text(kept);
    draft.set_caret(end);
    let thing = Thing {
        label: path.to_owned(),
        address,
        excerpt: None,
    };
    dispatch(win, Msg::Attach(AttachMsg::Toggle(thing)));
}

/// The current choice's name of the option `id`.
pub(crate) fn current(thread: &Thread, id: &str) -> Option<String> {
    let option = thread.option(id)?;
    let choice = option
        .options
        .iter()
        .find(|c| c.value == option.current_value);
    Some(line(choice.map_or(&option.current_value, |c| &c.name), 40))
}

/// The picker's groups, each item `<option>=<value>`, its name and whether it is current:
/// the options of `ids` in turn, and of the models those any thread answered with first.
pub(crate) fn groups(hub: &Hub, thread: &Thread, ids: &[&str]) -> Vec<Vec<(String, String, bool)>> {
    let said = hub
        .model
        .wire
        .threads
        .values()
        .flat_map(|t| t.transcript.messages.iter());
    let used: Vec<String> = said.filter_map(said_model).collect();
    let mut out = Vec::new();
    for id in ids {
        let Some(option) = thread.option(id) else {
            continue;
        };
        let models = *id == "model";
        let (first, rest): (Vec<_>, Vec<_>) =
            (option.options.iter()).partition(|c| models && used.contains(&c.value));
        for group in [first, rest].into_iter().filter(|g| !g.is_empty()) {
            let items = group.into_iter().map(|c| {
                let picked = c.value == option.current_value;
                (format!("{id}={}", c.value), line(&c.name, 60), picked)
            });
            out.push(items.collect());
        }
    }
    out
}

/// The `provider/model` an answer came from.
fn said_model(message: &yd_core::reduce::turns::Message) -> Option<String> {
    crate::chat::messages::said(message)
        .filter(|said| !said.user)
        .map(|said| said.model)
}

/// A picker over the choices of `ids`, one group each; a pick sets that option.
fn picker(win: &mut Win, tx: &mut Tx, pane: PaneId, ids: &'static [&'static str], anchor: String) {
    let Ok(hub) = win.hub.try_borrow() else {
        return;
    };
    let Some(thread) = shown(&hub, pane) else {
        return;
    };
    let mut menu = PopupMenu::new();
    for (n, group) in groups(&hub, thread, ids).into_iter().enumerate() {
        if n > 0 {
            menu = menu.separator();
        }
        let items = group
            .into_iter()
            .map(|(id, name, picked)| PopupItem::new(id, name).checked(picked));
        menu = menu.items(items);
    }
    drop(hub);
    tx.popup_menu(
        menu,
        anchor.as_str(),
        move |win: &mut Win, picked, _tx: &mut Tx| {
            let Some(picked) = picked else {
                return;
            };
            if let Some(run) = thread_run(win, pane, SET_OPTION, Some(picked.to_owned())) {
                dispatch(win, Msg::Command(Route::Wire, run));
            }
        },
    );
}

/// What the composer draws from.
pub struct Composer<'a> {
    pub key: &'a str,
    pub pane: PaneId,
    pub tokens: &'a Tokens,
    /// The thread whose mode and model the chips show, and whose turn send stops.
    pub thread: Option<&'a Thread>,
    /// The field to draw: [`drawn`].
    pub draft: Option<&'a TextEdit>,
    pub queued: Vec<&'a str>,
    /// The queue waits for a Send, since the turn it waited on was stopped.
    pub held: bool,
    /// The shown thread's project files, which an `@` word finds.
    pub files: Option<&'a Table>,
    /// What the empty box says.
    pub hint: &'static str,
    pub form: Form,
    pub send: Send,
}

impl Composer<'_> {
    pub fn view(&self) -> El {
        let tokens = self.tokens;
        let pane = self.pane;
        let blank = TextEdit::new("");
        let draft = self.draft.unwrap_or(&blank);
        let (style, lines) = match self.form {
            Form::Pane => (BODY.lh(17.0), 6),
            Form::Reading { .. } => (READ_PROSE.lh(19.0), 8),
        };
        let to = self.send.clone();
        let page = matches!(to, Send::New(_));
        let field = text_input(draft, style)
            .key(format!("{}.composer.input", self.key))
            .w_full()
            .color(colour(tokens.fg))
            .placeholder(self.hint, colour(tokens.faint))
            .label(self.hint)
            .caret(colour(tokens.accent), a11y::selection(tokens))
            .on_edit(move |win: &mut Win| edit(win, pane))
            .on_input_change(move |win: &mut Win, _tx: &mut Tx| sync(win, pane))
            .on_submit(move |win: &mut Win, _tx: &mut Tx| send(win, pane, &to))
            .on_input_key(move |win: &mut Win, event: &KeyEvent, _tx: &mut Tx| {
                keys(win, pane, event, page)
            });
        // A run held by a lost approval takes no prompt: say so rather than queued for ever.
        let said = match self.thread.is_some_and(Thread::lost) {
            true => "not sent, a lost approval holds the run",
            false => "queued",
        };
        let queued = self.queued.iter().map(|words| {
            text(format!("{said} · {}", line(words, 200)))
                .font(META)
                .color(colour(tokens.faint))
                .truncate()
        });
        let held = self.held.then(|| self.held_row());
        let frame = col().key(format!("{}.composer", self.key)).w_full();
        let frame = match self.form {
            Form::Pane => frame
                .pl(6.0)
                .pr(5.0)
                .py(4.0)
                .radius(9.0)
                .fill(colour(COMPOSER_PANE.of(tokens))),
            Form::Reading { gap } => frame
                .pt(11.0)
                .px(12.0)
                .pb(10.0)
                .gap(gap)
                .radius(RADIUS_COMPOSER.of(tokens))
                .fill(colour(COMPOSER.of(tokens)))
                .border(1.0, colour(LINE_MENU.of(tokens))),
        };
        let chips = match self.form {
            Form::Pane => self.chips(),
            Form::Reading { .. } => self.reading_chips(),
        };
        let boxed = frame.child(auto_grow(field, 1, lines)).child(chips);
        col()
            .w_full()
            .gap(4.0)
            .children(queued)
            .children(held)
            .children(self.finder(&draft.text))
            .child(boxed)
    }

    /// The files an `@` word finds, each a press that attaches it; Return takes the first.
    fn finder(&self, draft: &str) -> Option<El> {
        let tokens = self.tokens;
        let word = asking(draft)?;
        let found = self
            .files
            .map(|files| found(files, draft))
            .unwrap_or_default();
        let key = format!("{}.composer.files", self.key);
        let mut list = col().key(key.clone()).w_full().py(3.0).radius(6.0);
        list = list.fill(colour(tokens.raised));
        if found.is_empty() {
            let none = format!("No file of the project matches @{}", line(word, 60));
            return Some(
                list.px(8.0)
                    .child(text(none).font(META).color(colour(tokens.faint))),
            );
        }
        let pane = self.pane;
        let rows = found.into_iter().enumerate().map(|(n, path)| {
            row()
                .key(format!("{key}.{n}"))
                .w_full()
                .h(18.0)
                .px(8.0)
                .center_y()
                .cursor(Cursor::Pointer)
                .label(format!("Attach {path}"))
                .child(
                    text(line(&path, 120))
                        .font(META)
                        .color(colour(tokens.fg))
                        .truncate(),
                )
                .on_press(move |win: &mut Win| pick(win, pane, &path))
        });
        Some(list.children(rows))
    }

    /// What a Stop left queued: it says so, and Send sends it.
    fn held_row(&self) -> El {
        let (tokens, pane) = (self.tokens, self.pane);
        let words = "stopped · these wait until you send them";
        row()
            .key(format!("{}.composer.queued", self.key))
            .gap(8.0)
            .center_y()
            .child(text(words).font(META).color(colour(tokens.faint)))
            .child(
                text("Send")
                    .key(format!("{}.composer.queued.send", self.key))
                    .font(META)
                    .color(colour(tokens.accent))
                    .cursor(Cursor::Pointer)
                    .label("Send the queued prompts")
                    .on_press(move |win: &mut Win| send_queued(win, pane)),
            )
    }

    fn running(&self) -> bool {
        let busy = |t: &Thread| matches!(t.state, State::Running | State::NeedsYou);
        self.thread.is_some_and(busy) && self.send == Send::Prompt
    }

    /// The model with its effort, and the mode, as the thread's options name them.
    fn options(&self) -> (Option<String>, Option<String>) {
        let model = self.thread.and_then(|t| {
            let model = current(t, "model")?;
            Some(match current(t, "thought_level") {
                Some(effort) => format!("{model} · {effort}"),
                None => model,
            })
        });
        (model, self.thread.and_then(|t| current(t, "mode")))
    }

    /// Send, or stop while the thread's turn runs; `side` square with `radius` corners.
    fn button(&self, side: f32, radius: f32) -> El {
        let tokens = self.tokens;
        let pane = self.pane;
        let running = self.running();
        let face = if running {
            el().size(8.0, 8.0).radius(1.5).fill(colour(tokens.fg))
        } else {
            glyph(&SEND, 14.0, tokens.fg)
        };
        let to = self.send.clone();
        row()
            .key(format!("{}.composer.send", self.key))
            .size(side, side)
            .center()
            .radius(radius)
            .fill(colour(tokens.raised))
            .cursor(Cursor::Pointer)
            .label(if running { "Stop" } else { "Send" })
            .child(face)
            .on_press(move |win: &mut Win| {
                if running {
                    stop(win, pane)
                } else {
                    send(win, pane, &to)
                }
            })
    }

    fn chips(&self) -> El {
        let tokens = self.tokens;
        let pane = self.pane;
        let (model, mode) = self.options();
        let menu = |words: String, part: &'static str, ids: &'static [&'static str]| {
            let anchor = format!("{}.composer.{part}", self.key);
            chip(tokens, Chip::Plain, &words, None, false)
                .key(anchor.clone())
                .on_press_tx(move |win: &mut Win, tx: &mut Tx| {
                    picker(win, tx, pane, ids, anchor.clone())
                })
        };
        row()
            .w_full()
            .h(22.0)
            .gap(8.0)
            .center_y()
            .children(model.map(|words| {
                menu(words, "model", &["thought_level", "model"])
                    .shrink()
                    .min_w(0.0)
            }))
            .children(mode.map(|words| menu(words, "mode", &["mode"]).shrink().min_w(0.0)))
            .child(spacer())
            .child(self.button(22.0, RADIUS_BUTTON.of(tokens)))
    }

    fn reading_chips(&self) -> El {
        let tokens = self.tokens;
        let chip = |words: String, lead: Option<&'static Icon>, part: &str| {
            row()
                .key(format!("{}.composer.{part}", self.key))
                .h(22.0)
                .px(8.0)
                .gap(5.0)
                .center_y()
                .radius(5.0)
                .fill(colour(tokens.raised))
                .children(lead.map(|drawing| glyph(drawing, 11.0, tokens.dim)))
                .child(text(words).font(CHIP).color(colour(tokens.dim)))
        };
        let (model, mode) = self.options();
        row()
            .w_full()
            .h(28.0)
            .gap(6.0)
            .center_y()
            .child(
                chip(labels::ATTACH.to_owned(), Some(&PLUS), "attach")
                    .cursor(Cursor::Pointer)
                    .label("Attach a file")
                    .on_press_tx(attach),
            )
            .child(spacer())
            .children(mode.map(|words| chip(words, None, "mode")))
            .children(model.map(|words| chip(words, None, "model")))
            .child(self.button(28.0, 8.0).ml(4.0))
    }
}
