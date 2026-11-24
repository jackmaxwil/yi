//! Design pane: the picked element's properties, the list of changes, hand to the agent.

pub mod scenes;
#[cfg(test)]
mod tests;

use rune::{Cursor, Cx, El, KeyEvent, TextEdit, TextStyle, Tx, col, el, row, text, text_input};
use yd_core::address::attach::AttachMsg;
use yd_core::command::by_kind::{ELEMENT_DESIGN, ELEMENT_SOURCE};
use yd_core::command::table::{Command, Route, Run, Target};
use yd_core::command::tools::design::{
    ELEMENT, HAND, MAX_INSET, MAX_SIDE, PICK, Property, RESET, SET, START, STOP,
};
use yd_core::feed::{FeedStatus, SourceCmd};
use yd_core::grid::step::GridMsg;
use yd_core::ids::{CommandId, FeedId, PaneId, RowId};
use yd_core::model::Model;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_core::reduce::step::commands::PROMPT;
use yd_core::row::Row;
use yd_core::tokens::{Rgba, Tokens};
use yd_ui::chrome::tabs::{TARGET, Tab, Tabs};
use yd_ui::grid_view::view::{open_file, show_pane};
use yd_ui::kit::button::{Button, button};
use yd_ui::kit::chip::{Chip, chip};
use yd_ui::kit::label::section;
use yd_ui::menu::command_box;
use yd_ui::pane::{PaneCx, PaneDef};
use yd_ui::states::status::{Status, view as state_view};
use yd_ui::theme::look::{BODY, CODE, CODE_LINE, LABEL, colour, white};
use yd_ui::theme::scale::{LINE_SWATCH, SEMIBOLD, WELL};
use yd_ui::win::{Hub, Win, dispatch, field};

use super::changes::{self, Change};
use super::states::{PICKING, REFUSED, WORDS};

/// The feed the element and its changes are rows of: the page's own.
pub const FEED: FeedId = yd_core::feed::WEB;
/// The one field a value is typed into, in the window that edits it.
const VALUE: &str = "value";
/// An old value: 10 in the code face.
const WAS: TextStyle = TextStyle::new(10.0, 400.0).lh(16.0).family(CODE);
const CARD_LINE: TextStyle = CODE_LINE.lh(17.0);
const COUNT: TextStyle = TextStyle {
    weight: SEMIBOLD,
    ..LABEL
};

pub const DEF: PaneDef = PaneDef {
    kind: PaneKind::Design,
    view,
    scenes: scenes::SCENES,
    commands: COMMANDS,
    settings: &[],
    tabs: Some(tabs),
    on_tab: None,
    finder: None,
};

const fn command(id: CommandId, title: &'static str, route: Route) -> Command {
    Command {
        id,
        title,
        key: None,
        menu: None,
        kinds: &[],
        route,
    }
}

/// Design mode's commands. The window carries all but Reset, which the page's source does.
pub const COMMANDS: &[Command] = &[
    command(START, "Design the Web Page", Route::Model),
    command(STOP, "Leave Design Mode", Route::Model),
    command(PICK, "Pick Another Element", Route::Model),
    command(HAND, "Hand Design Changes to the Agent", Route::Model),
    command(RESET, "Reset Design Changes", Route::Source(FEED)),
];

/// Design mode, one for the app as the page is: kept in the hub under no pane, so the web
/// pane's layer and the Design pane read the same.
#[derive(Debug, Default)]
pub struct Mode {
    pub on: bool,
    /// The next press on the page picks the element under it.
    pub picking: bool,
    /// The property whose value is being typed, and why the last one typed was refused.
    pub editing: Option<Property>,
    pub refused: Option<String>,
    /// The size the element had when a handle was taken, and the size last asked for.
    pub grip: Option<(u32, u32)>,
    pub asked: Option<(u32, u32)>,
}

pub fn mode(hub: &Hub) -> Option<&Mode> {
    hub.panes.get::<Mode>(None)
}

/// Changes design mode from a handler.
pub fn set(win: &mut Win, edit: impl FnOnce(&mut Mode)) {
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(mode) = hub.panes.get_mut::<Mode>(None)
    {
        edit(mode);
    }
}

/// The picked element's row; none before a pick, and for a row that is only a note.
pub fn element(model: &Model) -> Option<&Row> {
    let row = model.tables.get(FEED)?.find(&RowId::new(ELEMENT))?;
    let named = row.words("name").is_some_and(|name| !name.is_empty());
    named.then_some(row)
}

/// Asks the page's source for something of design mode.
pub fn ask(win: &mut Win, command: CommandId, text: Option<String>) {
    let run = Run {
        text,
        ..Run::new(command)
    };
    dispatch(win, Msg::Source(FEED, SourceCmd::Run(run)));
}

/// Turns design mode on: the next press picks unless an element already is, and the window
/// shows a Design pane.
pub fn start(win: &mut Win) {
    let picked = win
        .hub
        .try_borrow()
        .is_ok_and(|hub| element(&hub.model).is_some());
    set(win, |mode| (mode.on, mode.picking) = (true, !picked));
    let window = win.window;
    let kind = PaneKind::Design;
    dispatch(win, Msg::Grid(GridMsg::Open { window, kind }));
}

/// The one tab, with the number of changes at the bar's right.
fn tabs(pane: &PaneCx<'_>) -> Tabs {
    let changes = changes::list(pane.model().tables.get(FEED)).len();
    let meta = Some(changes).filter(|n| *n > 0).map(changes::count);
    let tab = Tab {
        meta: meta.unwrap_or_default(),
        ..Tab::of(PaneKind::Design)
    };
    Tabs {
        tabs: vec![tab],
        ..Tabs::default()
    }
}

/// What the window carries out for design mode: start and stop it, arm a pick, hand the
/// changes over, and the two right-click rows of an element.
pub fn carry(win: &mut Win, tx: &mut Tx, command: CommandId, target: &Target) -> bool {
    match command {
        START | ELEMENT_DESIGN => {
            start(win);
            show_pane(win, tx, PaneKind::Design);
            true
        }
        STOP => {
            set(win, |mode| *mode = Mode::default());
            true
        }
        PICK => {
            set(win, |mode| (mode.on, mode.picking) = (true, true));
            true
        }
        HAND => hand(win).is_some(),
        ELEMENT_SOURCE => drawn_at(win, tx, target).is_some(),
        _ => false,
    }
}

/// Sends the shown thread one prompt: the changes as words, each changed element and the
/// lines that draw it attached. None when there is nothing to hand over or no thread.
fn hand(win: &mut Win) -> Option<()> {
    let (run, things) = {
        let hub = win.hub.try_borrow().ok()?;
        let changes = changes::list(hub.model.tables.get(FEED));
        let thread = yd_ui::shown::thread(&hub, win.window)?;
        let folder = yd_ui::shown::works_in(&hub, thread);
        let mut things = changes::things(&hub.model, &changes, Some(folder));
        things.retain(|thing| hub.model.attached.number(&thing.address).is_none());
        let run = Run {
            text: (!changes.is_empty()).then(|| changes::prompt(&changes)),
            ..Run::new(PROMPT).on(Target::Thing(thread.address().ok()?))
        };
        (run, things)
    };
    run.text.as_ref()?;
    for thing in things {
        dispatch(win, Msg::Attach(AttachMsg::Toggle(thing)));
    }
    dispatch(win, Msg::Command(Route::Wire, run));
    // What was attached went with this prompt; it does not go with the next one too.
    dispatch(win, Msg::Attach(AttachMsg::Clear));
    Some(())
}

/// Opens the lines that draw the element the command is aimed at, when the page says where.
fn drawn_at(win: &mut Win, tx: &mut Tx, target: &Target) -> Option<()> {
    let lines = {
        let hub = win.hub.try_borrow().ok()?;
        let row = element(&hub.model)?;
        let word = |cell: &str| row.words(cell).unwrap_or_default();
        let own = changes::element(word("page"), word("name"));
        let aimed = matches!(target, Target::Thing(address) if Some(address) == own.as_ref());
        let thread = yd_ui::shown::thread(&hub, win.window);
        let folder = thread.map(|thread| yd_ui::shown::works_in(&hub, thread));
        changes::lines(word("source"), folder).filter(|_| aimed)?
    };
    open_file(win, tx, lines);
    Some(())
}

/// Why a typed value is not one of `property`.
fn refusal(property: Property, typed: &str) -> String {
    let typed: String = typed.trim().chars().take(40).collect();
    let wanted = match property {
        Property::Size => format!("a size is width × height, each 1 to {MAX_SIDE}"),
        Property::Padding => format!("padding is one or two numbers up to {MAX_INSET}"),
        Property::Fill | Property::Text => "a colour is #rrggbb".to_owned(),
        Property::Corners => format!("corners are a number up to {MAX_INSET}"),
        Property::Says => "words are 1 to 512 characters on one line".to_owned(),
    };
    format!("“{typed}” was not applied: {wanted}")
}

/// Applies what was typed for `property`, or says in place why not and keeps the field.
fn submit(win: &mut Win, pane: PaneId, property: Property) {
    let typed = win.edits.get(&field(pane, VALUE));
    let typed = typed.map(|edit| edit.text.clone()).unwrap_or_default();
    match property.checked(&typed) {
        Some(value) => {
            set(win, |mode| (mode.editing, mode.refused) = (None, None));
            ask(win, SET, Some(format!("{}={value}", property.slug())));
        }
        None => set(win, |mode| mode.refused = Some(refusal(property, &typed))),
    }
}

/// What a row of the pane is drawn from.
struct Drawn<'a, 'b> {
    pane: &'a PaneCx<'b>,
    key: &'a str,
    row: &'a Row,
    mode: &'a Mode,
}

impl Drawn<'_, '_> {
    fn now(&self, property: Property) -> &str {
        self.row.words(property.slug()).unwrap_or_default()
    }

    /// The value before any change, when it differs from the one now.
    fn was(&self, property: Property) -> Option<&str> {
        let was = self.row.words(&format!("{}_was", property.slug()))?;
        (was != self.now(property)).then_some(was)
    }

    /// One value in its box, ringed when changed, keyed `<pane>.value.<property>` (a second
    /// box of it `.1`). A press opens the field on it.
    fn value(&self, property: Property, words: &str, index: usize) -> El {
        let tokens = self.pane.tokens();
        let changed = self.was(property).is_some();
        let ink = if changed { tokens.need } else { tokens.fg };
        let slug = property.slug();
        let own = match index {
            0 => format!("{}.value.{slug}", self.key),
            more => format!("{}.value.{slug}.{more}", self.key),
        };
        let boxed = row()
            .key(own)
            .h(16.0)
            .px(6.0)
            .min_w(0.0)
            .center_y()
            .radius(4.0)
            .fill(colour(tokens.raised))
            .child(text(words).font(CODE_LINE).color(colour(ink)).truncate());
        let boxed = if changed {
            boxed.border(1.0, colour(tokens.need))
        } else {
            boxed
        };
        let fixed = property == Property::Says && self.row.words("leaf") == Some("no");
        if fixed {
            return boxed;
        }
        let (pane, now) = (self.pane.pane, self.now(property).to_owned());
        let field_key = format!("{}.field", self.key);
        boxed
            .cursor(Cursor::Pointer)
            .on_press_tx(move |win: &mut Win, tx: &mut Tx| {
                win.edits
                    .insert(field(pane, VALUE), TextEdit::new(now.clone()));
                set(win, |mode| {
                    (mode.editing, mode.refused) = (Some(property), None)
                });
                tx.focus(field_key.clone(), false);
            })
    }

    /// The open field, and beside it why the last value typed was refused.
    fn typing(&self, property: Property) -> Vec<El> {
        let tokens = self.pane.tokens();
        let pane = self.pane.pane;
        let blank = TextEdit::new("");
        let edits = self.pane.edits;
        let edit = edits.and_then(|edits| edits.get(&field(pane, VALUE)));
        let input = text_input(edit.unwrap_or(&blank), CODE_LINE)
            .key(format!("{}.field", self.key))
            .w(150.0)
            .color(colour(tokens.fg))
            .caret(colour(tokens.accent), yd_ui::a11y::selection(tokens))
            .on_edit(move |win: &mut Win| win.edits.entry(field(pane, VALUE)).or_default())
            .on_submit(move |win: &mut Win, _: &mut Tx| submit(win, pane, property))
            .on_input_key(move |win: &mut Win, event: &KeyEvent, _: &mut Tx| {
                let escape = matches!(event.key, rune::Key::Escape);
                if escape {
                    set(win, |mode| (mode.editing, mode.refused) = (None, None));
                }
                escape
            });
        let refused = self.mode.refused.as_deref().map(|why| {
            text(why)
                .key(format!("{}.refused", self.key))
                .font(LABEL)
                .color(colour(tokens.warn))
                .truncate()
        });
        std::iter::once(input).chain(refused).collect()
    }

    /// A property's row: its name in 70 points, then its values, or the field while typing.
    fn property(&self, property: Property, name: &str) -> El {
        let tokens = self.pane.tokens();
        let now = self.now(property);
        let values: Vec<El> = if self.mode.editing == Some(property) {
            self.typing(property)
        } else {
            let swatch = matches!(property, Property::Fill | Property::Text)
                .then(|| Rgba::parse(now))
                .flatten()
                .map(|hue| swatch(tokens, hue));
            let boxes: Vec<El> = match property {
                Property::Padding => {
                    let parts = now.split_whitespace().enumerate();
                    parts
                        .map(|(at, part)| self.value(property, part, at))
                        .collect()
                }
                Property::Size
                | Property::Fill
                | Property::Text
                | Property::Corners
                | Property::Says => vec![self.value(property, now, 0)],
            };
            let was = self.was(property).filter(|_| property != Property::Says);
            let was = was.map(|was| {
                let struck = text(was).font(WAS).color(colour(tokens.faint));
                struck.strike(0, was.len(), None)
            });
            swatch.into_iter().chain(boxes).chain(was).collect()
        };
        row()
            .key(format!("{}.row.{}", self.key, property.slug()))
            .w_full()
            .h(22.0)
            .px(10.0)
            .center_y()
            .child(text(name).font(LABEL).color(colour(tokens.dim)).w(70.0))
            .child(
                row()
                    .grow(1.0)
                    .min_w(0.0)
                    .gap(5.0)
                    .center_y()
                    .children(values),
            )
    }
}

fn swatch(tokens: &Tokens, hue: Rgba) -> El {
    el().size(12.0, 12.0)
        .radius(3.0)
        .fill(colour(hue))
        .border(1.0, colour(LINE_SWATCH.of(tokens)))
}

/// The element's name, where the page says it is drawn, and Pick another.
fn header(pane: &PaneCx<'_>, key: &str, element: &Row) -> El {
    let tokens = pane.tokens();
    let word = |cell: &str| element.words(cell).unwrap_or_default();
    let another = chip(tokens, Chip::Plain, "Pick another", Some(&TARGET), false)
        .key(format!("{key}.pick-another"))
        .on_press_tx(|win: &mut Win, tx: &mut Tx| {
            command_box::run(win, tx, Run::new(PICK));
        });
    let source = text(word("source"))
        .font(LABEL)
        .color(colour(tokens.faint))
        .truncate()
        .grow(1.0)
        .min_w(0.0);
    row()
        .key(format!("{key}.design-header"))
        .w_full()
        .h(26.0)
        .px(10.0)
        .gap(8.0)
        .center_y()
        .border_sides([0.0, 0.0, 1.0, 0.0], colour(white(tokens.inner_lines)))
        .child(text(word("name")).font(CODE_LINE).color(colour(tokens.fg)))
        .child(source)
        .child(another)
}

/// The card: how many changes, that they are in the preview only, and a line for each.
fn card(tokens: &Tokens, key: &str, changes: &[Change]) -> Option<El> {
    let first = changes.first()?.number;
    let several = changes.iter().any(|change| change.number != first);
    let head = row()
        .gap(6.0)
        .child(
            text(changes::count(changes.len()))
                .font(COUNT)
                .color(colour(tokens.fg)),
        )
        .child(
            text("live in the preview only")
                .font(LABEL)
                .color(colour(tokens.faint)),
        );
    let mut lines = Vec::new();
    let mut under = None;
    for change in changes {
        if several && under != Some(change.number) {
            under = Some(change.number);
            let name = text(change.element.as_str()).font(CARD_LINE);
            lines.push(row().child(name.color(colour(tokens.faint))));
        }
        let noun = text(change.property.noun()).font(CARD_LINE);
        let line = text(change.line())
            .font(CARD_LINE)
            .color(colour(tokens.dim));
        lines.push(
            row()
                .key(format!(
                    "{key}.change.{}.{}",
                    change.number,
                    change.property.slug()
                ))
                .w_full()
                .child(noun.color(colour(tokens.fg)).w(92.0))
                .child(line.truncate().grow(1.0).min_w(0.0)),
        );
    }
    Some(
        col()
            .key(format!("{key}.changes"))
            .mx(8.0)
            .mt(8.0)
            .px(8.0)
            .py(5.0)
            .radius(8.0)
            .fill(colour(WELL.of(tokens)))
            .border(1.0, colour(tokens.control_line))
            .child(head)
            .children(lines),
    )
}

/// Hand to agent, Reset, and who writes the code. Both are faded while nothing is changed.
fn actions(tokens: &Tokens, key: &str, any: bool) -> El {
    let run = |command: CommandId| {
        move |win: &mut Win, tx: &mut Tx| {
            command_box::run(win, tx, Run::new(command));
        }
    };
    let strength = if any { 1.0 } else { 0.5 };
    let hand = button(tokens, Button::Primary, "Hand to agent", None)
        .key(format!("{key}.hand"))
        .opacity(strength)
        .on_press_tx(run(HAND));
    let reset = button(tokens, Button::Plain, "Reset", None)
        .key(format!("{key}.reset"))
        .opacity(strength)
        .on_press_tx(run(RESET));
    let who = text("The agent writes them into the code.")
        .font(LABEL)
        .color(colour(tokens.faint));
    row()
        .key(format!("{key}.actions"))
        .w_full()
        .px(8.0)
        .pt(9.0)
        .gap(6.0)
        .center_y()
        .child(hand)
        .child(reset)
        .child(who)
}

/// A line in the body: what the source last said.
fn said(tokens: &Tokens, key: String, words: &str) -> El {
    let ink = tokens.warn;
    row()
        .key(key)
        .w_full()
        .px(10.0)
        .py(6.0)
        .child(text(words).font(BODY).color(colour(ink)).wrap())
}

fn view(pane: &PaneCx<'_>, _cx: &mut Cx<Win>) -> El {
    let key = pane.key(PaneKind::Design);
    let tokens = pane.tokens();
    let table = pane.model().tables.get(FEED);
    let row = table.and_then(|table| table.find(&RowId::new(ELEMENT)));
    let noted = row
        .and_then(|row| row.words("note"))
        .filter(|note| !note.is_empty());
    let note = noted.map(|note| said(tokens, format!("{key}.note"), note));
    let fallback = Mode::default();
    let mode = mode(pane.hub).unwrap_or(&fallback);
    let body = col().key(key.clone()).w_full().h_full().min_h(0.0);
    let Some(element) = element(pane.model()) else {
        // Nothing is picked: the page's own state says why, else how to pick.
        let status = match table.map(|table| table.status()) {
            Some(FeedStatus::Loading) => Status::Loading,
            Some(FeedStatus::Offline) => Status::Offline(None),
            Some(FeedStatus::Error(why)) => Status::Error(why.clone()),
            Some(FeedStatus::Live | FeedStatus::Empty) | None => Status::Empty,
        };
        // Incident: a refused pick's note sat cut to one line above "Press an element".
        let refused = noted.filter(|_| status == Status::Empty);
        let (status, words) = match refused {
            Some(why) => (Status::Error(why.to_owned()), &REFUSED),
            None if mode.on => (status, &PICKING),
            None => (status, &WORDS),
        };
        return body.child(state_view(pane, &key, &status, words));
    };
    let changes = changes::list(table);
    let drawn = Drawn {
        pane,
        key: &key,
        row: element,
        mode,
    };
    body.clip_rounded(0.0)
        .child(header(pane, &key, element))
        .children(note)
        .child(section(tokens, "Layout", None, None).mt(4.0))
        .child(drawn.property(Property::Size, "Size"))
        .child(drawn.property(Property::Padding, "Padding"))
        .child(section(tokens, "Look", None, None).mt(4.0))
        .child(drawn.property(Property::Fill, "Fill"))
        .child(drawn.property(Property::Text, "Text"))
        .child(drawn.property(Property::Corners, "Corners"))
        .child(section(tokens, "Words", None, None).mt(4.0))
        .child(drawn.property(Property::Says, "Says"))
        .children(card(tokens, &key, &changes))
        .child(actions(tokens, &key, !changes.is_empty()))
}
