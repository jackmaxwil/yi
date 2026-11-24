//! One state block for one feed status: an icon, what is so, what to do, at most one button.

use rune::{Beat, El, Icon, Sequence, Tx, col, row, text};
use yd_core::command::table::{Run, Target};
use yd_core::feed::{FeedStatus, Table};
use yd_core::ids::CommandId;
use yd_core::pane::PaneKind;
use yd_core::reduce::state::Link;
use yd_core::row::Rows;
use yd_core::wire::event::Down;

use crate::kit::button::{Button, button};
use crate::kit::icons::glyph;
use crate::line_icon;
use crate::menu::command_box;
use crate::pane::{PaneCx, Scene};
use crate::theme::look::{BODY, META, colour};
use crate::win::Win;

mod scenes;

pub const SCENES: &[Scene] = scenes::SCENES;

/// Loading shows only after this long, so a feed that answers at once never flashes it.
pub const LOADING_AFTER: f64 = 0.3;

/// Held hidden for [`LOADING_AFTER`], then faded in.
static HELD: [Beat; 3] = [
    Beat::new(0.0).opacity(0.0),
    Beat::new(LOADING_AFTER).opacity(0.0),
    Beat::new(LOADING_AFTER + 0.15).opacity(1.0),
];

/// A feed that has said nothing this long is not loading: the block then says so.
pub const LOADING_LIMIT: f64 = 10.0;

static LATE: [Beat; 3] = [
    Beat::new(0.0).opacity(0.0),
    Beat::new(LOADING_LIMIT).opacity(0.0),
    Beat::new(LOADING_LIMIT + 0.15).opacity(1.0),
];

/// What a pane says in one state: what is so, what to do, and the one button that does it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Said {
    pub what: &'static str,
    pub next: &'static str,
    pub button: Option<CommandId>,
}

/// A pane's words for its four states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Words {
    pub loading: Said,
    pub empty: Said,
    pub offline: Said,
    pub error: Said,
}

/// What a pane shows in place of its body. Offline and error carry the source's own sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Loading,
    Empty,
    Offline(Option<String>),
    Error(String),
}

impl Status {
    /// What a feed's table shows instead of rows: nothing while it has rows to show.
    pub fn of_table(table: Option<&Table>) -> Option<Self> {
        let Some(table) = table else {
            return Some(Self::Loading);
        };
        if !table.is_empty() {
            return None;
        }
        Some(Self::of_feed(table.status()))
    }

    /// A feed's status as a state, whatever rows it holds.
    pub fn of_feed(status: &FeedStatus) -> Self {
        match status {
            FeedStatus::Loading => Self::Loading,
            FeedStatus::Live | FeedStatus::Empty => Self::Empty,
            FeedStatus::Offline => Self::Offline(None),
            FeedStatus::Error(why) => Self::Error(why.clone()),
        }
    }

    /// The link to Yi's server, for a pane that needs it: nothing while it is up. A server
    /// that is not there is offline; one that could not start or answer is an error.
    pub fn of_link(link: &Link) -> Option<Self> {
        match link {
            Link::Starting => Some(Self::Loading),
            Link::Up { .. } => None,
            Link::Down { why, .. } => Some(match why {
                Down::NoServer | Down::Stopped => Self::Offline(Some(why.to_string())),
                Down::StartFailed(_) | Down::Handshake(_) | Down::Closed(_) => {
                    Self::Error(why.to_string())
                }
            }),
        }
    }
}

static LOADING: Icon = line_icon!("loading", r#"<path d="M12 4a8 8 0 1 1-8 8"/>"#);
static EMPTY: Icon = line_icon!(
    "empty",
    r#"<path d="M4 13h4l1.5 2.5h5L16 13h4M4 13l2.5-7h11l2.5 7v5H4z"/>"#
);
static OFFLINE: Icon = line_icon!(
    "offline",
    r#"<path d="M9 7V3M15 7V3M7 7h10v4a5 5 0 0 1-10 0zM12 16v5M4 4l16 16"/>"#
);
static ERROR: Icon = line_icon!(
    "error",
    r#"<circle cx="12" cy="12" r="8"/><path d="M12 8v5M12 16v.5"/>"#
);

/// The state block, drawn inside a pane's body under its bars, keyed `<key>.state`; its
/// button is `<key>.state.<command id>` and runs that command on the pane.
pub fn view(pane: &PaneCx<'_>, key: &str, status: &Status, words: &Words) -> El {
    let tokens = pane.tokens();
    let (drawing, source) = match status {
        Status::Loading => (&LOADING, None),
        Status::Empty => (&EMPTY, None),
        Status::Offline(source) => (&OFFLINE, source.as_deref()),
        Status::Error(source) => (&ERROR, Some(source.as_str())),
    };
    let said = pick(status, words);
    let ink = match status {
        Status::Error(_) => tokens.bad,
        Status::Loading | Status::Empty | Status::Offline(_) => tokens.faint,
    };
    let line = |words: &str, font, ink| text(words).font(font).color(colour(ink)).wrap();
    let press = said
        .button
        .and_then(|id| Some(press(pane, key, id)?.mt(6.0)));
    let block = col()
        .key(format!("{key}.state"))
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .center()
        .gap(3.0)
        .px(16.0)
        .child(glyph(drawing, 16.0, ink).mb(4.0))
        .child(line(said.what, BODY, tokens.dim))
        .children((!said.next.is_empty()).then(|| line(said.next, META, tokens.faint)))
        .children(source.map(|source| line(source, META.mono(), tokens.faint)))
        .children(press);
    match status {
        Status::Loading => block
            .child(
                line(
                    "Nothing has answered yet. Check Settings, or restart the app.",
                    META,
                    tokens.faint,
                )
                .key(format!("{key}.state.late"))
                .mt(3.0)
                .sequence(Sequence::new(&LATE)),
            )
            .sequence(Sequence::new(&HELD)),
        Status::Empty | Status::Offline(_) | Status::Error(_) => block,
    }
}

fn pick<'a>(status: &Status, words: &'a Words) -> &'a Said {
    match status {
        Status::Loading => &words.loading,
        Status::Empty => &words.empty,
        Status::Offline(_) => &words.offline,
        Status::Error(_) => &words.error,
    }
}

/// The state's one button, `<key>.state.<command id>`, running its command on the pane.
/// Nothing for a command no table declares.
fn press(pane: &PaneCx<'_>, key: &str, id: CommandId) -> Option<El> {
    let table = command_box::table(pane.hub);
    let command = table.find(id)?;
    let run = Run::new(id).on(Target::Pane(pane.pane));
    let face = button(pane.tokens(), Button::Plain, command.title, None);
    let face = face.key(format!("{key}.state.{}", id.0));
    Some(face.on_press_tx(move |win: &mut Win, tx: &mut Tx| {
        command_box::run(win, tx, run.clone());
    }))
}

/// A pane whose whole body is one state: its root, tinted as a pane is, with the block in it.
pub fn body(pane: &PaneCx<'_>, kind: PaneKind, status: &Status, words: &Words) -> El {
    let key = pane.key(kind);
    let tokens = pane.tokens();
    col()
        .key(key.clone())
        .w_full()
        .h_full()
        .fill(colour(tokens.pane_tint.alpha(tokens.glass_pane)))
        .child(view(pane, &key, status, words))
}

/// The state as one line above content a pane still shows, keyed `<key>.state`.
pub fn bar(pane: &PaneCx<'_>, key: &str, status: &Status, words: &Words) -> El {
    let tokens = pane.tokens();
    let said = pick(status, words);
    let press = said.button.and_then(|id| press(pane, key, id));
    row()
        .key(format!("{key}.state"))
        .w_full()
        .h(24.0)
        .px(8.0)
        .gap(6.0)
        .center_y()
        .child(text(said.what).font(META).color(colour(tokens.dim)))
        .child(
            text(said.next)
                .font(META)
                .color(colour(tokens.faint))
                .truncate()
                .grow(1.0)
                .min_w(0.0),
        )
        .children(press)
}

/// The one line a list keeps above rows it still holds: the feed went offline or failed.
pub fn notice(status: &FeedStatus) -> Option<String> {
    match status {
        FeedStatus::Offline => Some("Offline: showing what was last known".to_owned()),
        FeedStatus::Error(why) => Some(format!("Could not update: {why}")),
        FeedStatus::Loading | FeedStatus::Live | FeedStatus::Empty => None,
    }
}
