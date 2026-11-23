//! Remote desktop pane: a machine's screen, its windows and rows drawn over the picture from the
//! feed's rows so pick finds them. A live screen has no such list: its host sends the picture.

mod bar;
mod sample;
pub mod scenes;
#[cfg(test)]
mod tests;

use std::borrow::Cow;

use rune::{Cx, El, Tx, col, el};
use yd_core::address::attach::AttachMsg;
use yd_core::address::thing::Thing;
use yd_core::address::uri::Address;
use yd_core::command::by_kind::{SCREEN_MARK, SCREEN_SHOT};
use yd_core::command::table::{Command, Route, Target};
use yd_core::command::tools::remote::{self, SCREEN};
use yd_core::feed::{FeedStatus, REMOTE, SourceCmd, Table};
use yd_core::ids::{CommandId, FeedId, PaneId, RowId, SettingKey};
use yd_core::kind::Kind;
use yd_core::model::Model;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_core::row::Row;
use yd_core::settings::schema::{Choice, Control, Setting, SettingValue, Store};
use yd_ui::chrome::tabs::{Tab, Tabs};
use yd_ui::grid_view::view::show_pane;
use yd_ui::overlay::pick::Finder;
use yd_ui::pane::{PaneCx, PaneDef};
use yd_ui::screen::input::Input;
use yd_ui::screen::surface::{Screen, surface};
use yd_ui::states::status::{Status, view as state_view};
use yd_ui::theme::look::colour;
use yd_ui::theme::scale::WELL;
use yd_ui::win::{Hub, Win, dispatch};

use super::states::WORDS;
use crate::design::markup;

/// The feed this pane shows; the `machines` source fills it.
pub const FEED: FeedId = REMOTE;

/// The setting that names the machine host: `none` or `recorded`.
pub const ADAPTER: &str = "machines.adapter";
const CHOICES: [Choice; 2] = [
    Choice {
        label: Cow::Borrowed("None"),
        value: Cow::Borrowed("none"),
    },
    Choice {
        label: Cow::Borrowed("Recorded data"),
        value: Cow::Borrowed("recorded"),
    },
];
const ADAPTER_ROW: Setting = Setting {
    key: SettingKey::fixed(ADAPTER),
    group: Cow::Borrowed("Machines"),
    name: Cow::Borrowed("Machine host"),
    description: Cow::Borrowed("Which host answers for VMs, assets and remote screens."),
    control: Control::Select(Cow::Borrowed(&CHOICES)),
    store: Store::App,
    default: Some(SettingValue::Text(Cow::Borrowed("none"))),
    value: None,
};

/// The adapter the setting names, as the source is told it.
pub fn adapter(model: &Model) -> String {
    match model.settings.value(ADAPTER) {
        Some(SettingValue::Text(name)) => name.to_string(),
        _ => "none".to_owned(),
    }
}

/// The record's letterbox rule: the screen is held 5 points clear of the box on every side
/// and never drawn below this scale.
const CLEAR: f32 = 5.0;
const LEAST: f32 = 0.08;

/// The points each screen pixel takes in a letterbox of `w` by `h` points (record 4.6).
pub fn scale(w: f32, h: f32) -> f32 {
    let (sw, sh) = (SCREEN.0 as f32, SCREEN.1 as f32);
    let fitted = ((w - 2.0 * CLEAR) / sw).min((h - 2.0 * CLEAR) / sh);
    fitted.max(LEAST)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Watch,
    Control,
    MarkUp,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Watch, Mode::Control, Mode::MarkUp];

    pub fn title(self) -> &'static str {
        match self {
            Mode::Watch => "Watch",
            Mode::Control => "Control",
            Mode::MarkUp => "Mark up",
        }
    }

    fn command(self) -> CommandId {
        match self {
            Mode::Watch => remote::WATCH,
            Mode::Control => remote::CONTROL,
            Mode::MarkUp => remote::MARK_UP,
        }
    }
}

/// What the pane keeps: the screen its source draws to, how it is used, and which machine.
#[derive(Debug, Default)]
pub struct State {
    pub screen: Screen,
    pub mode: Mode,
    /// The VM whose screen was asked for; none shows the first the host has.
    pub machine: Option<String>,
}

const fn button(id: CommandId, title: &'static str) -> Command {
    Command {
        id,
        title,
        key: None,
        menu: None,
        kinds: &[],
        route: Route::Model,
    }
}

/// The bar's buttons, as the command box lists them.
pub const COMMANDS: &[Command] = &[
    button(remote::WATCH, "Watch the Screen"),
    button(remote::CONTROL, "Control the Screen"),
    button(remote::MARK_UP, "Mark Up the Screen"),
    button(remote::SCREENSHOT, "Screenshot the Screen"),
];

pub const DEF: PaneDef = PaneDef {
    kind: PaneKind::Remote,
    view,
    scenes: scenes::SCENES,
    commands: COMMANDS,
    settings: &[ADAPTER_ROW],
    tabs: Some(tabs),
    on_tab: None,
    finder: Some(Finder::Keyed(thing_at)),
};

/// Changes this pane's state from a handler.
pub fn change(win: &mut Win, pane: PaneId, edit: impl FnOnce(&mut State)) {
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(state) = hub.panes.get_mut::<State>(Some(pane))
    {
        edit(state);
    }
}

/// Shows `machine`'s screen in this pane: its element asks the source once it is drawn.
pub fn open(win: &mut Win, pane: PaneId, machine: &str) {
    change(win, pane, |state| state.machine = Some(machine.to_owned()));
}

fn table(hub: &Hub) -> Option<&Table> {
    hub.model.tables.get(FEED)
}

/// The `screen:0` row: the machine, its size, rate and latency, and what it last said.
fn header(table: Option<&Table>) -> Option<&Row> {
    table?.find(&RowId::new("screen:0"))
}

fn tabs(pane: &PaneCx<'_>) -> Tabs {
    let row = header(table(pane.hub));
    let word = |field: &str| row.and_then(|r| r.words(field)).filter(|w| !w.is_empty());
    let latency = word("ms").map(|ms| format!("{ms} ms"));
    let parts: Vec<String> = [word("fps").map(str::to_owned), latency]
        .into_iter()
        .flatten()
        .collect();
    let tab = Tab {
        title: word("machine")
            .unwrap_or(PaneKind::Remote.title())
            .to_owned(),
        meta: parts.join(" · "),
        ..Tab::of(PaneKind::Remote)
    };
    Tabs {
        tabs: vec![tab],
        ..Tabs::default()
    }
}

/// The ask that shows a machine: aimed at its `vm` address, at none for the first.
fn ask_open(win: &mut Win, pane: PaneId, machine: Option<&str>) {
    // The source finds the pane's screen in its state, so the state exists before the ask.
    change(win, pane, |_| {});
    let run = yd_core::command::table::Run::new(remote::OPEN);
    let target = machine.and_then(|id| Address::new(Kind::Vm, id).ok());
    let run = target.map_or(run.clone(), |address| run.on(Target::Thing(address)));
    dispatch(win, Msg::Source(FEED, SourceCmd::Run(run)));
}

/// What the surface heard: Control hands it to the source, Mark up draws a region with it
/// and nothing reaches the far side, Watch drops it.
fn send(win: &mut Win, pane: PaneId, input: Input) {
    let read = win.hub.try_borrow().ok().and_then(|hub| {
        let mode = hub.panes.get::<State>(Some(pane))?.mode;
        let machine = header(table(&hub)).and_then(|row| row.words("machine"));
        Some((mode, machine.unwrap_or_default().to_owned()))
    });
    match read {
        Some((Mode::MarkUp, machine)) => {
            markup::took(win, pane, &machine, input);
        }
        Some((Mode::Control, _)) => {
            if let Some(ask) = input.ask() {
                dispatch(win, Msg::Source(FEED, SourceCmd::Screen(ask)));
            }
        }
        Some((Mode::Watch, _)) | None => {}
    }
}

fn view(pane: &PaneCx<'_>, cx: &mut Cx<Win>) -> El {
    let key = pane.key(PaneKind::Remote);
    let tokens = pane.tokens();
    let fallback = State::default();
    let state = pane.state::<State>().unwrap_or(&fallback);
    let table = table(pane.hub);
    let row = header(table);
    let id = pane.pane;
    let machine = state.machine.clone();
    let asked = format!("{key}.open.{}", machine.as_deref().unwrap_or("first"));
    let open = el().key(asked).w(0.0).h(0.0).on_layout_tx(
        move |win: &mut Win, _: [f32; 4], _: &mut Tx| {
            ask_open(win, id, machine.as_deref());
        },
    );
    let fail = match table.map(Table::status) {
        Some(FeedStatus::Error(why)) => Some(why.as_str()),
        _ => None,
    };
    let held = format!("{key}.letterbox");
    let size = cx.layout_size(&held);
    let body = match (row, fail) {
        (None, Some(why)) => state_view(pane, &key, &Status::Error(why.to_owned()), &WORDS),
        (None, None) if adapter(&pane.hub.model) == "none" => state_view(
            pane,
            &key,
            &Status::Error(remote::NO_HOST.to_owned()),
            &WORDS,
        ),
        (None, None) => {
            let status = match table.map(Table::status) {
                Some(FeedStatus::Offline) => Status::Offline(None),
                Some(FeedStatus::Live | FeedStatus::Empty) => Status::Empty,
                Some(FeedStatus::Loading | FeedStatus::Error(_)) | None => Status::Loading,
            };
            state_view(pane, &key, &status, &WORDS)
        }
        (Some(_), _) => {
            let s = size.map(|[w, h]| scale(w, h));
            let screen = s.map(|s| {
                let (w, h) = (SCREEN.0 as f32 * s, SCREEN.1 as f32 * s);
                let on = move |win: &mut Win, input: Input| send(win, id, input);
                let face = format!("{key}.surface");
                let picture = surface(&face, &state.screen, on);
                let drawn = sample::overlay(tokens, &key, table, s);
                // A region stays boxed while it is attached, whatever the bar says now.
                let named = row.and_then(|r| r.words("machine")).unwrap_or_default();
                let marks = markup::layer(pane, cx, &face, &state.screen, named);
                el().key(format!("{key}.screen"))
                    .w(w)
                    .h(h)
                    .clip_rounded(0.0)
                    .child(picture)
                    .child(drawn)
                    .child(marks)
            });
            col()
                .key(held)
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .padding(CLEAR)
                .center()
                .fill(colour(WELL.of(tokens)))
                .children(screen)
        }
    };
    let said_line = bar::note_of(state.mode, row, fail);
    col()
        .key(key.clone())
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(open)
        .child(bar::bar(pane, &key, state.mode, row))
        .children(said_line.map(|line| bar::note(tokens, &key, &line)))
        .child(body)
}

/// The screen thing at `address`: its id is the machine, then the window, then the row.
fn screen_thing(row: &Row, table: &Table, machine: &str) -> Option<Thing> {
    let time = header(Some(table))
        .and_then(|h| h.words("time"))
        .unwrap_or_default();
    let slug = |id: &str| id.split_once(':').map_or(id, |(_, rest)| rest).to_owned();
    let parent = row.words("parent").map(str::to_owned);
    let path = match &parent {
        Some(window) => format!("{machine}/{window}/{}", slug(row.id.as_str())),
        None => format!("{machine}/{}", slug(row.id.as_str())),
    };
    let name = row.words("name").unwrap_or_default();
    let what = if parent.is_some() { "row" } else { "window" };
    let address = Address::new(Kind::Screen, path)
        .ok()?
        .part(format!("t={time}"));
    Some(Thing {
        label: format!("{machine} · {name} {what}"),
        address,
        excerpt: Some(name.to_owned()),
    })
}

/// A window or a row of the sample screen, by the key its element was drawn under.
fn thing_at(hub: &Hub, pane: PaneId, key: &str) -> Option<Thing> {
    let prefix = format!("{}-{}.screen.", PaneKind::Remote.slug(), pane.0);
    let id = key.strip_prefix(prefix.as_str())?;
    let table = table(hub)?;
    let row = table.find(&RowId::new(id))?;
    let machine = header(Some(table))?.words("machine")?;
    matches!(row.words("kind"), Some("window" | "row"))
        .then(|| screen_thing(row, table, machine))?
}

/// Attaches the screen as it is now, as one thing: its address names the machine and the time.
fn screenshot(win: &mut Win, machine: &str) {
    let Ok(hub) = win.hub.try_borrow() else {
        return;
    };
    let time = table(&hub)
        .and_then(|t| header(Some(t)))
        .and_then(|h| h.words("time"));
    let size = table(&hub)
        .and_then(|t| header(Some(t)))
        .and_then(|h| h.words("size"));
    let Ok(address) = Address::new(Kind::Screen, machine) else {
        return;
    };
    let time = time.unwrap_or_default();
    let address = address.part(format!("t={time}"));
    if hub.model.attached.number(&address).is_some() {
        return;
    }
    let thing = Thing {
        label: format!("{machine} · screenshot {time}"),
        address,
        excerpt: Some(format!(
            "A screenshot of {machine}'s screen at {time}, {}.",
            size.unwrap_or_default()
        )),
    };
    drop(hub);
    dispatch(win, Msg::Attach(AttachMsg::Toggle(thing)));
}

/// The bar's buttons and the right-click row that screenshots a screen. False for any other
/// command or target.
pub fn carry(win: &mut Win, tx: &mut Tx, command: CommandId, target: &Target) -> bool {
    let mode = Mode::ALL.into_iter().find(|mode| mode.command() == command);
    let machine = |win: &Win| {
        let hub = win.hub.try_borrow().ok()?;
        let row = header(table(&hub))?;
        row.words("machine").map(str::to_owned)
    };
    match (target, mode) {
        (Target::Pane(pane), Some(mode)) => {
            change(win, *pane, |state| state.mode = mode);
            true
        }
        (Target::Pane(_), None) if command == remote::SCREENSHOT => {
            let named = machine(win);
            named.is_some_and(|machine| {
                screenshot(win, &machine);
                true
            })
        }
        (Target::Thing(address), None) if address.kind() == Kind::Screen => {
            let named = address.id().split('/').next().map(str::to_owned);
            if command == SCREEN_MARK {
                let shown = show_pane(win, tx, PaneKind::Remote);
                return shown.is_some_and(|pane| {
                    change(win, pane, |state| state.mode = Mode::MarkUp);
                    true
                });
            }
            command == SCREEN_SHOT
                && named.is_some_and(|machine| {
                    screenshot(win, &machine);
                    true
                })
        }
        _ => false,
    }
}
