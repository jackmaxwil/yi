//! Changes pane: the thread's uncommitted diff file by file, comments on lines, revert by hunk.

mod lines;
mod merge;
mod scenes;
#[cfg(test)]
mod tests;

use std::borrow::Cow;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;

use rune::{Cx, El, GrowthSpec, ScrollSpec, Tx, col, row, spacer, text};
use rune_kit::diff::{FileDiff, parse};
use yd_core::address::thing::Thing;
use yd_core::address::uri::Address;
use yd_core::feed::Table;
use yd_core::ids::{FeedId, PaneId, ThreadId};
use yd_core::kind::Kind;
use yd_core::pane::PaneKind;
use yd_core::row::{Row, Rows};
use yd_core::tokens::Tokens;
use yd_ui::chrome::tabs::{Tab, Tabs};
use yd_ui::kit::chip::{Chip, chip};
use yd_ui::kit::segmented::segmented;
use yd_ui::overlay::pick::Finder;
use yd_ui::pane::{PaneCx, PaneDef};
use yd_ui::theme::look::{CODE_LINE, META, NONE, colour, white};
use yd_ui::win::{Hub, Win};

use lines::Draw;
use yd_ui::states::status::{Status, body as state_body};

use super::states::WORDS;

pub const FEED: FeedId = yd_core::feed::CHANGES;

/// What a row stands for: a diff line (`<pane key>.row.<path>:<hunk>:<index>.L<n>`) is that
/// line of its file, a file's header (`.row.<path>`) the file.
fn thing_at(hub: &Hub, _: PaneId, key: &str) -> Option<Thing> {
    let (_, row) = key.split_once(".row.")?;
    let line = row.rsplit_once(".L").and_then(|(at, n)| {
        let number = n.parse::<u32>().ok().filter(|n| *n > 0)?;
        let mut parts = at.rsplitn(3, ':');
        let counted = |part: Option<&str>| part.is_some_and(|part| part.parse::<u32>().is_ok());
        let placed = counted(parts.next()) && counted(parts.next());
        parts.next().filter(|_| placed).map(|path| (path, number))
    });
    let address = match line {
        Some((path, n)) => Address::new(Kind::Lines, path).ok()?.part(format!("L{n}")),
        None => Address::new(Kind::File, row).ok()?,
    };
    Some(Thing::at(&hub.model, address))
}

pub const DEF: PaneDef = PaneDef {
    kind: PaneKind::Changes,
    view,
    scenes: scenes::SCENES,
    commands: &[],
    settings: &[],
    tabs: Some(tab),
    on_tab: None,
    finder: Some(Finder::Keyed(thing_at)),
};

/// The pane's one tab: how many files changed after its name, their sums as its meta.
fn tab(pane: &PaneCx<'_>) -> Tabs {
    let changed = files(pane.model().tables.get(FEED)).len();
    let tab = Tab {
        count: u32::try_from(changed).ok().filter(|count| *count > 0),
        meta: meta(pane),
        ..Tab::of(PaneKind::Changes)
    };
    Tabs {
        tabs: vec![tab],
        ..Tabs::default()
    }
}

/// A comment being written under a line: the file, the line and the words so far.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Draft {
    pub path: String,
    pub line: u32,
    pub words: String,
    /// A whole hunk's comment: the lines it spans (`L3-L9`) and its diff text.
    pub hunk: Option<(String, String)>,
}

/// The pane's own state; it travels with the pane.
#[derive(Debug, Default)]
pub struct State {
    /// The file whose chip is on; the first file when none.
    pub file: Option<String>,
    /// Comments on lines, as rows with `path`, `line`, `author`, `at`, `status`, `finding`,
    /// `body`: there is no store for them yet, so they live with the pane.
    pub comments: Vec<Row>,
    pub draft: Option<Draft>,
    /// The hunk whose revert waits for a yes: its file and number.
    pub confirm: Option<(String, usize)>,
    /// The merge of a Yi checkout into the project, once its button is pressed, and the
    /// thread it was pressed for: another thread's Changes does not show it.
    pub merge: Option<(Option<ThreadId>, merge::Merge)>,
}

/// One changed file: its path, status letter, counts and parsed diff.
#[derive(Debug, Clone)]
pub struct File {
    pub path: String,
    pub status: String,
    pub added: String,
    pub removed: String,
    pub diff: Option<FileDiff>,
}

/// The changed files in the order the feed sent them.
pub fn files(table: Option<&Table>) -> Vec<File> {
    let rows = table
        .into_iter()
        .flat_map(|t| (0..t.len()).filter_map(|i| t.get(i)));
    rows.filter_map(|row| {
        let path = row.id.as_str().strip_prefix("change:")?.to_owned();
        let cell = |field| row.words(field).unwrap_or("0").to_owned();
        let diff = row
            .words("patch")
            .and_then(|patch| parse(patch).into_iter().next());
        Some(File {
            path,
            status: cell("status"),
            added: cell("added"),
            removed: cell("removed"),
            diff,
        })
    })
    .collect()
}

/// The meta a pane's tab shows: "2 files · +54 −1".
pub fn meta(pane: &PaneCx<'_>) -> String {
    let files = files(pane.model().tables.get(FEED));
    let sum = |pick: fn(&File) -> &str| {
        files
            .iter()
            .filter_map(|f| pick(f).parse::<u64>().ok())
            .sum::<u64>()
    };
    let noun = if files.len() == 1 { "file" } else { "files" };
    format!(
        "{} {noun} · +{} −{}",
        files.len(),
        sum(|f| &f.added),
        sum(|f| &f.removed)
    )
}

/// A bar narrower than this has no room for its controls and says what it compares with.
const CONTROLS_FROM: f32 = 420.0;

/// "vs main · a1b9f3e": what the changes are measured against, as the git feed's row says;
/// a thread's own checkout is named first, since its edits are not in the project's folder.
fn against(table: Option<&Table>) -> Option<String> {
    let repo = table?.find(&yd_core::ids::RowId::new("repo"))?;
    let word = |cell: &str| repo.words(cell).filter(|word| !word.is_empty());
    let (base, commit) = (word("base")?, word("commit")?);
    Some(match lane(table) {
        Some(branch) => format!("checkout {branch} vs {base} · {commit}"),
        None => format!("vs {base} · {commit}"),
    })
}

/// The branch of the checkout Yi gave the thread, `yi/<session id>`, when it works in one.
fn lane(table: Option<&Table>) -> Option<&str> {
    let repo = table?.find(&yd_core::ids::RowId::new("repo"))?;
    repo.words("branch")
        .filter(|branch| branch.starts_with("yi/"))
}

fn view(pane: &PaneCx<'_>, cx: &mut Cx<Win>) -> El {
    let key = pane.key(PaneKind::Changes);
    crate::files::pane::copy::under(pane, &key, diffs(pane, cx))
}

fn diffs(pane: &PaneCx<'_>, cx: &mut Cx<Win>) -> El {
    let key = pane.key(PaneKind::Changes);
    let tokens = pane.tokens();
    let empty = State::default();
    let state = pane.state::<State>().unwrap_or(&empty);
    // Invariant: the read is made on every run, whatever the body turns out to be.
    let narrow = cx
        .layout_size(&key)
        .is_some_and(|size| size[0] < CONTROLS_FROM);
    let table = pane.model().tables.get(FEED);
    let shown = pane.thread().map(|thread| thread.id.clone());
    let ours = state.merge.as_ref().filter(|(of, _)| *of == shown);
    let said = merge::line(tokens, &key, pane.pane, table, ours.map(|(_, m)| *m));
    let files = files(table);
    if files.is_empty() {
        let status = Status::of_table(table).unwrap_or(Status::Empty);
        let body = state_body(pane, PaneKind::Changes, &status, &WORDS);
        return match said {
            Some(said) => col().w_full().h_full().child(said).child(body),
            None => body,
        };
    }
    let on = state
        .file
        .clone()
        .or_else(|| files.first().map(|f| f.path.clone()));
    let chips = files.iter().map(|file| {
        file_chip(
            tokens,
            &key,
            pane.pane,
            file,
            on.as_deref() == Some(&file.path),
        )
    });
    let bar = row()
        .key(format!("{key}.chips"))
        .w_full()
        .h(22.0)
        .px(8.0)
        .gap(5.0)
        .center_y()
        .border_sides([0.0, 0.0, 1.0, 0.0], colour(white(tokens.lines)));
    // The chips give way first, so a narrow pane keeps the merge button whole.
    let held = row().gap(5.0).shrink().min_w(0.0).clip_rounded(0.0);
    let bar = bar.child(held.children(chips)).child(spacer());
    // The mockup's rule: the narrow bar drops its controls for what the diff is against.
    let bar = if narrow || lane(table).is_some() {
        let base = against(table).unwrap_or_default();
        let base = text(base).font(META).color(colour(tokens.faint));
        bar.child(base.truncate().shrink().min_w(0.0))
            .children(merge::button_of(tokens, &key, pane.pane, table))
    } else {
        let mode = format!("{key}.mode");
        bar.child(chip(tokens, Chip::Plain, "This turn", None, true))
            .child(segmented(
                tokens,
                &mode,
                &["Unified", "Split"],
                0,
                |_: &mut Win, _| {},
            ))
    };
    col()
        .key(key.clone())
        .w_full()
        .h_full()
        .fill(colour(tokens.pane_tint.alpha(tokens.glass_pane)))
        .child(bar)
        .children(said)
        .child(body(tokens, &key, pane.pane, files, state))
}

fn file_chip(tokens: &Tokens, key: &str, pane: PaneId, file: &File, on: bool) -> El {
    let (fill, ink) = if on {
        (tokens.selected, tokens.fg)
    } else {
        (NONE, tokens.dim)
    };
    let name = file
        .path
        .rsplit_once('/')
        .map_or(file.path.as_str(), |(_, name)| name);
    let count = |sign: &str, n: &str, ink| {
        (n != "0").then(|| text([sign, n].concat()).font(CODE_LINE).color(colour(ink)))
    };
    let (path, header) = (file.path.clone(), format!("{key}.row.{}", file.path));
    row()
        .key(format!("{key}.chip.{}", file.path))
        .h(18.0)
        .px(6.0)
        .gap(4.0)
        .center_y()
        .radius(5.0)
        .fill(colour(fill))
        .border(1.0, colour(white(tokens.lines)))
        .child(text(name).font(CODE_LINE).color(colour(ink)))
        .children(count("+", &file.added, tokens.ok))
        .children(count("−", &file.removed, tokens.bad))
        .on_press_tx(move |win: &mut Win, tx: &mut Tx| {
            change(win, pane, |s| s.file = Some(path.clone()));
            tx.scroll_key_to(header.clone(), 0.0);
        })
}

/// Every file's header, lines and comments on one list of virtual rows.
fn body(tokens: &Tokens, key: &str, pane: PaneId, files: Vec<File>, state: &State) -> El {
    let items = lines::items(&files, state);
    let mut version = DefaultHasher::new();
    (&items, &state.draft, &state.confirm).hash(&mut version);
    for comment in &state.comments {
        comment.id.hash(&mut version);
    }
    let draw = Rc::new(Draw {
        tokens: tokens.clone(),
        key: key.to_owned(),
        pane,
        files,
        items,
        comments: state.comments.clone(),
        draft: state.draft.clone(),
    });
    let (keyed, built) = (draw.clone(), draw.clone());
    let stack = col()
        .key(format!("{key}.stack"))
        .w_full()
        .growth(GrowthSpec::default())
        .virtual_rows(
            draw.items.len(),
            16.0,
            move |_: &Win, at| Cow::Owned(keyed.key_at(at)),
            move |_: &Win, at| built.row_at(at),
        )
        .virtual_rows_version(version.finish());
    col()
        .key(format!("{key}.rows"))
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(stack)
}

/// Changes this pane's state from a handler.
pub(crate) fn change(win: &mut Win, pane: PaneId, edit: impl FnOnce(&mut State)) {
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(state) = hub.panes.get_mut::<State>(Some(pane))
    {
        edit(state);
    }
}

/// Faint 9.5 words.
pub(crate) fn faint(tokens: &Tokens, words: &str) -> El {
    text(words).font(META).color(colour(tokens.faint))
}
