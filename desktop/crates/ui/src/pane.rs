//! What a pane is to the app: a definition in a static table, the context its view reads,
//! and the scenes that show it headless.

use rune::{Cx, El, col, text};
use yd_core::command::table::Command;
use yd_core::ids::{PaneId, TabId};
use yd_core::model::Model;
use yd_core::pane::PaneKind;
use yd_core::reduce::threads::Thread;
use yd_core::settings::schema::Setting;
use yd_core::tokens::Tokens;

use crate::chrome::tabs::Tabs;
use crate::overlay::pick::Finder;
use crate::theme::look::{META, TITLE, colour};
use crate::win::{Edits, Hub, Win};

/// What a window draws.
#[derive(Debug, Clone, Copy)]
pub enum Stage {
    /// The app: shell bars and the grid of panes.
    App,
    /// One pane filling the window.
    Pane(PaneKind),
    /// One view filling the window: an engine's demo, a whole-window form.
    View(fn(&Win, &Hub, &mut Cx<Win>) -> El),
}

/// A named state for headless pictures and trees: `<pane>/<state>`.
#[derive(Debug, Clone, Copy)]
pub struct Scene {
    pub name: &'static str,
    pub stage: Stage,
    /// Builds the state: fixtures sent as feed and wire events through `step`.
    pub setup: fn(&mut Hub),
}

/// What a hand asks of a pane that keeps its own tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabAsk {
    /// Bring this tab in front.
    Show(TabId),
    Close(TabId),
    /// The bar's `+`: one more tab.
    New,
}

/// One row of the pane table.
#[derive(Debug, Clone, Copy)]
pub struct PaneDef {
    pub kind: PaneKind,
    pub view: fn(&PaneCx<'_>, &mut Cx<Win>) -> El,
    pub scenes: &'static [Scene],
    /// The commands this pane adds to the app's.
    pub commands: &'static [Command],
    /// The settings this pane adds to the settings page.
    pub settings: &'static [Setting],
    /// The pane's own tabs as they stand: the one in front, and the meta each puts at the
    /// bar's right. None gives the bar the tabs kept for the pane, else one named after it.
    pub tabs: Option<fn(&PaneCx<'_>) -> Tabs>,
    /// What a press on one of its own tabs, a tab's close or the bar's `+` does.
    pub on_tab: Option<fn(&mut Win, PaneId, TabAsk)>,
    /// How its keyed elements are found as things, for pick mode, the boxes and right-click.
    pub finder: Option<Finder>,
}

/// What a pane's view reads.
#[derive(Clone, Copy)]
pub struct PaneCx<'a> {
    pub hub: &'a Hub,
    pub pane: PaneId,
    /// Whether this is the pane the user is in.
    pub focused: bool,
    /// The text fields the window drawing this pane edits; none where no window is at hand.
    pub edits: Option<&'a Edits>,
}

impl<'a> PaneCx<'a> {
    pub fn model(&self) -> &'a Model {
        &self.hub.model
    }

    pub fn tokens(&self) -> &'a Tokens {
        &self.hub.model.tokens
    }

    /// The thread the window this pane is in shows.
    pub fn thread(&self) -> Option<&'a Thread> {
        crate::shown::of_pane(self.hub, self.pane)
    }

    /// This pane's own state, if it has made any.
    pub fn state<T: 'static>(&self) -> Option<&'a T> {
        self.hub.panes.get(Some(self.pane))
    }

    /// The prefix of every key in this pane: `<slug>-<pane id>`.
    pub fn key(&self, kind: PaneKind) -> String {
        format!("{}-{}", kind.slug(), self.pane.0)
    }
}

/// The pane every scene shows alone.
pub const SCENE_PANE: PaneId = PaneId(0);

/// Puts the hub in scene `index` of its scene table. A scene past the table changes nothing.
pub fn show_scene(win: &mut Win, index: usize) {
    let Ok(mut hub) = win.hub.try_borrow_mut() else {
        return;
    };
    let Some(scene) = hub.scenes.get(index).copied() else {
        return;
    };
    hub.stage = scene.stage;
    (scene.setup)(&mut hub);
}

/// A pane that is not built yet: its name and the task that builds it.
pub fn placeholder(pane: &PaneCx<'_>, kind: PaneKind, task: &'static str) -> El {
    let tokens = pane.tokens();
    col()
        .key(pane.key(kind))
        .w_full()
        .h_full()
        .center()
        .gap(4.0)
        .fill(colour(tokens.pane_tint.alpha(tokens.glass_pane)))
        .child(text(kind.title()).font(TITLE).color(colour(tokens.fg)))
        .child(
            text(format!("not built yet · task {task}"))
                .font(META)
                .color(colour(tokens.faint)),
        )
}
