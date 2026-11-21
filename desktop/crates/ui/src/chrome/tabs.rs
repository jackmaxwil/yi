//! The tab bar: tabs, meta and the split, pull-out and close icons.

use rune::{Cursor, Drag, El, Icon, Tx, el, row, text};
use yd_core::command::table::{Command, Route, Run, Target};
use yd_core::ids::{CommandId, PaneId, TabId};
use yd_core::pane::PaneKind;
use yd_core::tokens::Tokens;

use crate::grid_view::drag::{At, Grab, hand};
use crate::grid_view::view::{Act, act};
use crate::kit::button::PRESS_SCALE;
use crate::kit::icons::glyph;
use crate::line_icon;
use crate::menu::command_box;
use crate::pane::Scene;
use crate::theme::look::{LABEL, META, NONE, colour};
use crate::theme::scale::{PRESS, TAB_AWAY, TAB_BAR, TAB_ON};
use crate::win::Win;

pub const SCENES: &[Scene] = &[];

/// The bar is 20 tall: 19 of tabs over one clear line (record 2.6).
pub const BAR: f32 = 20.0;
/// The bar where it is its window's title bar, under the traffic lights (record 2.8.5).
pub const TITLED: f32 = 23.0;
/// Clear room between the last light and the first tab.
const AFTER_LIGHTS: f32 = 6.0;
pub const TAB: f32 = 19.0;
const ICON: f32 = 11.0;
const TAB_PAD: f32 = 5.0;
const TAB_GAP: f32 = 4.0;

pub const CLOSE_TAB: CommandId = CommandId("tab.close");
pub const PREVIOUS_TAB: CommandId = CommandId("tab.previous");
pub const NEXT_TAB: CommandId = CommandId("tab.next");
pub const SPLIT: CommandId = CommandId("pane.split");
pub const POP_OUT: CommandId = CommandId("pane.pop-out");
pub const CLOSE: CommandId = CommandId("pane.close");
pub const ACROSS: CommandId = CommandId("pane.keep-across");
pub const HERE: CommandId = CommandId("pane.keep-here");
pub const MOVE_LEFT: CommandId = CommandId("pane.move-left");
pub const MOVE_RIGHT: CommandId = CommandId("pane.move-right");
pub const MOVE_UP: CommandId = CommandId("pane.move-up");
pub const MOVE_DOWN: CommandId = CommandId("pane.move-down");

const fn command(id: CommandId, title: &'static str, key: Option<&'static str>) -> Command {
    Command {
        id,
        title,
        key,
        menu: Some("Window"),
        kinds: &[],
        route: Route::Model,
    }
}

/// What a hand can do to the pane it is in. `grid_view::view::run` carries each out.
pub const COMMANDS: &[Command] = &[
    command(CLOSE_TAB, "Close Tab", Some("cmd+w")),
    command(PREVIOUS_TAB, "Show Previous Tab", Some("cmd+shift+[")),
    command(NEXT_TAB, "Show Next Tab", Some("cmd+shift+]")),
    command(SPLIT, "Split Pane", None),
    command(POP_OUT, "Move Pane to Its Own Window", None),
    command(CLOSE, "Close Pane", None),
    command(ACROSS, "Keep Pane Across Sessions", None),
    command(HERE, "Keep Pane in This Session Only", None),
    command(MOVE_LEFT, "Move Pane Left", Some("ctrl+shift+cmd+left")),
    command(MOVE_RIGHT, "Move Pane Right", Some("ctrl+shift+cmd+right")),
    command(MOVE_UP, "Move Pane Up", Some("ctrl+shift+cmd+up")),
    command(MOVE_DOWN, "Move Pane Down", Some("ctrl+shift+cmd+down")),
];

pub static CHAT: Icon = line_icon!("chat", r#"<path d="M4 5h16v11H9l-5 4z"/>"#);
pub static FOLDER: Icon = line_icon!(
    "folder",
    r#"<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>"#
);
pub static DIFF: Icon = line_icon!("diff", r#"<path d="M12 4v8M8 8h8M8 18h8"/>"#);
pub static FILE: Icon = line_icon!(
    "file",
    r#"<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/>"#
);
pub static TERM: Icon = line_icon!("term", r#"<path d="m5 8 4 4-4 4M12 16h7"/>"#);
pub static GLOBE: Icon = line_icon!(
    "globe",
    r#"<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3c3 3.500 3 14.500 0 18M12 3c-3 3.500-3 14.500 0 18"/>"#
);
pub static MONITOR: Icon = line_icon!(
    "monitor",
    r#"<rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4"/>"#
);
pub static SLIDERS: Icon = line_icon!(
    "sliders",
    r#"<path d="M4 7h10M18 7h2M4 17h4M12 17h8"/><circle cx="16" cy="7" r="2"/><circle cx="10" cy="17" r="2"/>"#
);
pub static BRANCH: Icon = line_icon!(
    "branch",
    r#"<circle cx="7" cy="6" r="2"/><circle cx="7" cy="18" r="2"/><circle cx="17" cy="8" r="2"/><path d="M7 8v8M17 10c0 4-10 2-10 6"/>"#
);
pub static SHIELD: Icon = line_icon!(
    "shield",
    r#"<path d="M12 3 5 6v6c0 4.500 3 7.500 7 9 4-1.500 7-4.500 7-9V6z"/>"#
);
pub static PANEL: Icon = line_icon!(
    "panel",
    r#"<rect x="3" y="4" width="18" height="16" rx="3"/><path d="M12 4v16M3 12h9"/>"#
);
pub static PLAY: Icon = line_icon!("play", r#"<path d="M8 5v14l11-7z" fill="currentColor"/>"#);
pub static TARGET: Icon = line_icon!(
    "target",
    r#"<circle cx="12" cy="12" r="7"/><path d="M12 2v4M12 18v4M2 12h4M18 12h4"/>"#
);
pub static PLUS: Icon = line_icon!("plus", r#"<path d="M12 5v14M5 12h14"/>"#);
pub static OUT: Icon = line_icon!("out", r#"<path d="M8 16 17 7M9 7h8v8"/>"#);
pub static X: Icon = line_icon!("x", r#"<path d="M6 6l12 12M18 6 6 18"/>"#);

/// The icon a pane kind's tab carries in the mockup.
pub fn icon_of(kind: PaneKind) -> &'static Icon {
    match kind {
        PaneKind::Chat => &CHAT,
        PaneKind::Files => &FOLDER,
        PaneKind::Changes => &DIFF,
        PaneKind::Terminal => &TERM,
        PaneKind::Web | PaneKind::Assets => &GLOBE,
        PaneKind::Remote | PaneKind::Vms => &MONITOR,
        PaneKind::Code
        | PaneKind::Report
        | PaneKind::Logs
        | PaneKind::Script
        | PaneKind::IPython => &FILE,
        PaneKind::Agents => &BRANCH,
        PaneKind::Findings => &SHIELD,
        PaneKind::Containers | PaneKind::Plugins | PaneKind::PluginKit => &PANEL,
        PaneKind::Daemons => &PLAY,
        PaneKind::Debugger => &TARGET,
        PaneKind::Design
        | PaneKind::Trace
        | PaneKind::LanguageServers
        | PaneKind::Settings
        | PaneKind::Themes => &SLIDERS,
    }
}

/// One tab of a pane.
#[derive(Debug, Clone)]
pub struct Tab {
    pub id: TabId,
    pub icon: &'static Icon,
    pub title: String,
    /// The number after the title, such as the 2 of `Changes 2`.
    pub count: Option<u32>,
    /// What the bar says at its right while this tab is the active one.
    pub meta: String,
    /// It cannot close: a pane's own tab that the others go back to. It has no cross.
    pub fixed: bool,
}

/// Tab ids from here up are not a pane's own: each names a pane stacked behind the one
/// drawn, by its number above this.
const STACKED: u32 = 1 << 30;

/// The pane a tab stands for when it is another pane of the slot's stack, not the pane's own.
pub fn pane_of(tab: TabId) -> Option<PaneId> {
    tab.0.checked_sub(STACKED).map(PaneId)
}

/// What names a tab in its key: its number, or `pane-<n>` for another pane of the stack.
fn part(tab: TabId) -> String {
    pane_of(tab).map_or_else(|| tab.0.to_string(), |pane| format!("pane-{}", pane.0))
}

impl Tab {
    /// The tab that stands for `pane`, of `kind`, stacked behind the pane a bar is drawn for.
    pub fn stacked(pane: PaneId, kind: PaneKind) -> Tab {
        let id = TabId(STACKED.saturating_add(pane.0));
        Tab {
            id,
            ..Tab::of(kind)
        }
    }

    /// The one tab a pane has until its owner gives it others: its kind's name.
    pub fn of(kind: PaneKind) -> Tab {
        Tab {
            id: TabId(0),
            icon: icon_of(kind),
            title: kind.title().to_owned(),
            count: None,
            meta: String::new(),
            fixed: false,
        }
    }

    /// How wide it draws: padding, icon, gap, title, and a count after one more gap. It
    /// measures text, so it is asked only while a tab is dragged.
    pub fn width(&self) -> f32 {
        let count = self.count.map_or(0.0, |count| {
            TAB_GAP + rune::text::advance(&count.to_string(), LABEL)
        });
        let title = rune::text::advance(&self.title, LABEL);
        (TAB_PAD * 2.0 + ICON + TAB_GAP + title + count).ceil()
    }
}

/// Which way `Tabs::turn` goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Turn {
    Back,
    On,
}

/// A pane's tabs, kept in `hub.panes` so they travel with the pane. A pane that keeps none
/// shows the one tab its kind names.
#[derive(Debug, Clone, Default)]
pub struct Tabs {
    pub tabs: Vec<Tab>,
    pub active: Option<TabId>,
    /// The bar ends its tabs with a `+` that asks the pane for one more.
    pub adds: bool,
    /// A button of the pane's own beside the split icon, keyed `<pane key>.pane-<part>`.
    pub tool: Option<Tool>,
}

/// A pane's own button in its bar: a press runs `command` aimed at the pane.
#[derive(Debug, Clone, Copy)]
pub struct Tool {
    pub part: &'static str,
    pub icon: &'static Icon,
    pub label: &'static str,
    /// Drawn in the accent while what it shows is shown.
    pub on: bool,
    pub command: CommandId,
}

impl Tabs {
    /// What a pane's bar shows: the tabs it keeps, or the one its kind names.
    pub fn shown(kept: Option<&Tabs>, kind: PaneKind) -> Tabs {
        match kept {
            Some(tabs) if !tabs.tabs.is_empty() => tabs.clone(),
            _ => Tabs {
                tabs: vec![Tab::of(kind)],
                ..Tabs::default()
            },
        }
    }

    /// The tab in front: the one picked, or the first.
    pub fn active(&self) -> Option<&Tab> {
        let picked = self.tabs.iter().find(|tab| Some(tab.id) == self.active);
        picked.or(self.tabs.first())
    }

    fn index(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == id)
    }

    pub fn select(&mut self, id: TabId) {
        if self.index(id).is_some() {
            self.active = Some(id);
        }
    }

    /// Brings the next or the previous tab in front, going round at the ends.
    pub fn turn(&mut self, way: Turn) {
        let Some(at) = self.active().and_then(|tab| self.index(tab.id)) else {
            return;
        };
        let last = self.tabs.len().saturating_sub(1);
        let next = match way {
            Turn::On if at >= last => 0,
            Turn::On => at.saturating_add(1),
            Turn::Back => at.checked_sub(1).unwrap_or(last),
        };
        self.active = self.tabs.get(next).map(|tab| tab.id);
    }

    /// Moves tab `id` so that `before` of the other tabs come ahead of it.
    pub fn place(&mut self, id: TabId, before: usize) {
        let Some(from) = self.index(id) else { return };
        let tab = self.tabs.remove(from);
        self.tabs.insert(before.min(self.tabs.len()), tab);
    }

    /// Takes tab `id` out; the tab that was beside it comes in front.
    pub fn take(&mut self, id: TabId) -> Option<Tab> {
        let at = self.index(id)?;
        let tab = self.tabs.remove(at);
        if self.active.is_none_or(|active| active == id) {
            let beside = self.tabs.get(at).or(self.tabs.last());
            self.active = beside.map(|tab| tab.id);
        }
        Some(tab)
    }

    /// How many of the other tabs sit left of `x`, by their middles: where a tab dragged to
    /// `x` points from the bar's left belongs. Middles keep a wide neighbour from flickering.
    pub fn slot_at(&self, dragged: TabId, x: f32) -> usize {
        let mut left = 0.0;
        let mut ahead = 0usize;
        for tab in &self.tabs {
            let width = tab.width();
            if tab.id != dragged && left + width / 2.0 < x {
                ahead = ahead.saturating_add(1);
            }
            left += width;
        }
        ahead
    }
}

/// What a bar knows of its pane.
pub struct Bar<'a> {
    pub pane: PaneId,
    /// The pane's key, the prefix of every key in the bar.
    pub key: &'a str,
    pub tabs: &'a Tabs,
    pub focused: bool,
    /// Pulled out of another window: the middle icon docks it back.
    pub pulled: bool,
    /// The only pane of its window: the bar moves the window, not the pane.
    pub alone: bool,
    /// The pane hears its own tabs' asks: each tab carries a close cross.
    pub closes: bool,
    /// The room the traffic lights take, where the bar is its window's title bar.
    pub lights: Option<f32>,
    pub at: At,
}

/// The most of a title, in points, a tab keeps when its bar is short of room: "zsh" whole.
const TITLE_KEPT: f32 = 24.0;

fn tab(tokens: &Tokens, bar: &Bar<'_>, tab: &Tab, active: bool) -> El {
    let (ink, fill) = match (active, bar.focused) {
        (true, true) => (tokens.fg, TAB_ON.of(tokens)),
        (true, false) => (tokens.fg, TAB_AWAY.of(tokens)),
        (false, _) => (tokens.faint, NONE),
    };
    let (pane, id, at) = (bar.pane, tab.id, bar.at);
    // Another pane of the stack closes from its tab, whatever the pane in front does.
    let closes = (bar.closes || pane_of(tab.id).is_some()) && !tab.fixed;
    let counted = tab.count.map_or(0.0, |count| {
        TAB_GAP + rune::text::advance(&count.to_string(), LABEL)
    });
    let title = rune::text::advance(&tab.title, LABEL).min(TITLE_KEPT);
    let crossed = if closes { TAB_GAP + 8.0 } else { 0.0 };
    let least = 2.0 * TAB_PAD + ICON + TAB_GAP + title + counted + crossed;
    let count = tab.count.map(|count| {
        text(count.to_string())
            .font(LABEL)
            .color(colour(tokens.faint))
    });
    let body = row()
        .key(format!("{}.tab-{}.pane-tab", bar.key, part(tab.id)))
        .role("tab")
        .focusable(true)
        .focus_selected(active)
        // A pointer's press is the drag's first phase, below: only the keyboard presses here.
        .on_press_tx(move |win: &mut Win, tx: &mut Tx| {
            if tx.focus_visible() {
                act(win, tx, Act::Tab(pane, id));
            }
        })
        .h(TAB)
        .self_start()
        .px(TAB_PAD)
        .gap(TAB_GAP)
        .center_y()
        .fill(colour(fill))
        .cursor(Cursor::Pointer)
        .press(colour(PRESS.of(tokens)))
        .press_scale(PRESS_SCALE)
        .child(glyph(tab.icon, ICON, ink))
        .child(
            text(tab.title.as_str())
                .font(LABEL)
                .color(colour(ink))
                .truncate()
                .min_w(0.0)
                .shrink(),
        )
        .children(count)
        .children(closes.then(|| {
            row()
                .key(format!("{}.tab-{}.pane-tab-close", bar.key, part(tab.id)))
                .label("Close tab")
                .cursor(Cursor::Pointer)
                .press_scale(PRESS_SCALE)
                .child(glyph(&X, 8.0, tokens.faint))
                .on_press_tx(move |win: &mut Win, tx: &mut Tx| {
                    act(win, tx, Act::CloseOne(pane, id))
                })
        }))
        .on_drag(move |win: &mut Win, drag: &mut Drag, tx: &mut Tx| {
            hand(win, Grab::Tab(pane, id), at, drag, tx);
        });
    // Incident: a long title pushed the pane's icons off the bar, and a tab shrunk past its icon
    // and cross drew them over each other. A title gives way down to `TITLE_KEPT`, never below.
    body.shrink().min_w(least)
}

fn pane_icon(
    tokens: &Tokens,
    bar: &Bar<'_>,
    part: &str,
    drawing: &'static Icon,
    act_on: Act,
) -> El {
    let ink = match act_on {
        Act::PopOut(_) if bar.pulled => tokens.accent,
        _ => tokens.faint,
    };
    let said = match act_on {
        Act::Split(_) => "Split pane",
        Act::PopOut(_) if bar.pulled => "Put pane back",
        Act::PopOut(_) => "Pull pane out",
        _ => "Close pane",
    };
    row()
        .key(format!("{}.{part}", bar.key))
        .label(said)
        .cursor(Cursor::Pointer)
        .press_scale(PRESS_SCALE)
        .child(glyph(drawing, ICON, ink))
        .on_press_tx(move |win: &mut Win, tx: &mut Tx| act(win, tx, act_on))
}

/// A pane's tab bar: its tabs, the active tab's meta, and the split, pull-out and close icons.
pub fn bar(tokens: &Tokens, bar: &Bar<'_>) -> El {
    let front = bar.tabs.active().map(|tab| tab.id);
    let tabs = bar
        .tabs
        .tabs
        .iter()
        .map(|each| tab(tokens, bar, each, Some(each.id) == front));
    let meta = bar.tabs.active().map_or("", |tab| tab.meta.as_str());
    let more = bar.tabs.adds.then(|| {
        let pane = bar.pane;
        row()
            .key(format!("{}.pane-tab-new", bar.key))
            .label("New tab")
            .h(TAB)
            .self_start()
            .px(4.0)
            .center_y()
            .cursor(Cursor::Pointer)
            .press_scale(PRESS_SCALE)
            .child(glyph(&PLUS, ICON, tokens.faint))
            .on_press_tx(move |win: &mut Win, tx: &mut Tx| act(win, tx, Act::NewTab(pane)))
    });
    let tool = bar.tabs.tool.map(|tool| {
        let ink = if tool.on { tokens.accent } else { tokens.faint };
        let (pane, command) = (bar.pane, tool.command);
        row()
            .key(format!("{}.pane-{}", bar.key, tool.part))
            .label(tool.label)
            .cursor(Cursor::Pointer)
            .press_scale(PRESS_SCALE)
            .child(glyph(tool.icon, ICON, ink))
            .on_press_tx(move |win: &mut Win, tx: &mut Tx| {
                let run = Run::new(command).on(Target::Pane(pane));
                command_box::run(win, tx, run);
            })
    });
    let icons = row()
        .h_full()
        .px(5.0)
        .gap(6.0)
        .ml(4.0)
        .center_y()
        .children(tool)
        .child(pane_icon(
            tokens,
            bar,
            "pane-split",
            &PANEL,
            Act::Split(bar.pane),
        ))
        .child(pane_icon(
            tokens,
            bar,
            "pane-out",
            &OUT,
            Act::PopOut(bar.pane),
        ))
        .child(pane_icon(
            tokens,
            bar,
            "pane-close",
            &X,
            Act::Close(bar.pane),
        ));
    let (pane, at) = (bar.pane, bar.at);
    let lights = bar.lights.filter(|room| *room > 0.0).map(|room| {
        el().key(format!("{}.pane-lights", bar.key))
            .w(room + AFTER_LIGHTS)
            .h_full()
    });
    let strip = row()
        .key(format!("{}.pane-tab-bar", bar.key))
        .w_full()
        .h(if bar.lights.is_some() { TITLED } else { BAR })
        .fill(colour(TAB_BAR.of(tokens)))
        .cursor(Cursor::Grab)
        .clip_rounded(0.0)
        .children(lights)
        .child(
            row()
                .role("tablist")
                .min_w(0.0)
                .shrink()
                .clip_rounded(0.0)
                .label("Tabs")
                .focus_group()
                .children(tabs),
        )
        .children(more)
        .child(
            row()
                .grow(1.0)
                .min_w(0.0)
                .h_full()
                .pl(8.0)
                .center_y()
                .justify_end()
                .child(
                    text(meta)
                        .key(format!("{}.pane-meta", bar.key))
                        .font(META)
                        .color(colour(tokens.faint))
                        .truncate(),
                ),
        )
        .child(icons);
    if bar.alone {
        return strip.window_drag();
    }
    strip.on_drag(move |win: &mut Win, drag: &mut Drag, tx: &mut Tx| {
        hand(win, Grab::Bar(pane), at, drag, tx);
    })
}
