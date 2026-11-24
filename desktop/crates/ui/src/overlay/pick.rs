//! Pick mode and the lookup under it: from a keyed element to its thing, a press to attach.

use std::sync::PoisonError;

use rune::{Picked, Tx};
use yd_core::address::attach::{ATTACH, AttachMsg, PICK};
use yd_core::address::thing::Thing;
use yd_core::address::uri::Address;
use yd_core::command::run::shown_path;
use yd_core::command::table::Target;
use yd_core::feed::{self, page_row_id};
use yd_core::ids::{CommandId, FeedId, PaneId, TabId, ThreadId};
use yd_core::kind::Kind;
use yd_core::list::ListDesc;
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::row::Rows;

use crate::chrome::tabs::Tabs;
use crate::code::rows::InView;
use crate::list::row;
use crate::pane::PaneCx;
use crate::term::view::state::{Shells, Tab};
use crate::win::{Hub, Win, dispatch};

/// The layer that takes the pointer while picking; it holds the keyboard for Escape.
pub const LAYER: &str = "overlay.pick";
/// The most keys one pointer position is tried under: a path's keyed ancestors, nearest first.
const MAX_TRIES: usize = 512;

/// How one kind of pane says what its keyed elements stand for.
#[derive(Debug, Clone, Copy)]
pub enum Finder {
    /// A list pane: a row's key names its row in the feed called by the pane's slug.
    Rows(&'static ListDesc),
    /// Any other pane reads the key itself: a code line, a message, a step.
    Keyed(fn(&Hub, PaneId, &str) -> Option<Thing>),
}

/// An element and what it stands for.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub key: String,
    pub thing: Thing,
}

/// What attaching by hand keeps beside the model, in `hub.panes` under no pane: who finds
/// things, whether the pointer is picking, and the element each attached thing was found at.
#[derive(Debug, Default)]
pub struct Context {
    finders: Vec<(PaneKind, Finder)>,
    pub picking: bool,
    /// What the pointer is over while picking.
    pub hover: Option<Found>,
    /// The attached thing whose chip the pointer is on, and the one whose box it is in.
    chip: Option<Address>,
    boxed: Option<Address>,
    /// The chip pressed last: the address line shows it while nothing is lit.
    pub active: Option<Address>,
    keys: Vec<(Address, String)>,
}

impl Context {
    /// Says how panes of `kind` find their things. The last word for a kind stands.
    pub fn find_with(&mut self, kind: PaneKind, finder: Finder) {
        self.finders.retain(|(known, _)| *known != kind);
        self.finders.push((kind, finder));
    }

    /// The attached thing that is lit: the one whose chip the pointer is on, else the one
    /// whose box it is in. Its chip and its box are drawn lit together.
    pub fn lit(&self) -> Option<&Address> {
        self.chip.as_ref().or(self.boxed.as_ref())
    }

    /// The element an attached thing was picked at: where its box is drawn.
    pub fn key_of(&self, address: &Address) -> Option<&str> {
        let kept = self.keys.iter().find(|(known, _)| known == address);
        kept.map(|(_, key)| key.as_str())
    }
}

pub fn context(hub: &Hub) -> Option<&Context> {
    hub.panes.get::<Context>(None)
}

/// The pane a key belongs to: its first part is `<slug>-<pane id>`.
pub fn pane_of(key: &str) -> Option<(PaneKind, PaneId)> {
    let (slug, id) = key.split('.').next()?.rsplit_once('-')?;
    let kind = PaneKind::from_slug(slug)?;
    Some((kind, PaneId(id.parse().ok()?)))
}

/// A thread's row in the sidebar or the folded strip: `shell.<form>.<thread id>.sidebar-row`.
fn thread_row(hub: &Hub, key: &str) -> Option<Thing> {
    let row = key.strip_prefix("shell.")?.strip_suffix(".sidebar-row")?;
    let (_, id) = row.split_once('.')?;
    let thread = hub.model.wire.threads.get(&ThreadId::new(id))?;
    Some(Thing::at(&hub.model, thread.address().ok()?))
}

/// A pane's tab, `<pane key>.tab-<tab id>.pane-tab`, named by its title.
fn tab(hub: &Hub, kind: PaneKind, pane: PaneId, key: &str) -> Option<Thing> {
    let (_, id) = key.strip_suffix(".pane-tab")?.rsplit_once(".tab-")?;
    // A tab that stands for another pane of the stack, `pane-<n>`, is that pane.
    if let Some(other) = id.strip_prefix("pane-") {
        let other = PaneId(other.parse().ok()?);
        return Some(Thing {
            label: hub.model.grid.kind(other)?.title().to_owned(),
            address: Address::new(Kind::Pane, other.0.to_string()).ok()?,
            excerpt: None,
        });
    }
    let id = TabId(id.parse().ok()?);
    let context = PaneCx {
        hub,
        pane,
        focused: false,
        edits: None,
    };
    let own = hub
        .def(kind)
        .and_then(|def| def.tabs)
        .map(|own| own(&context));
    let kept = hub.panes.get::<Tabs>(Some(pane));
    let tabs = Tabs::shown(own.as_ref().or(kept), kind);
    let tab = tabs.tabs.iter().find(|tab| tab.id == id)?;
    // The tab in front stands for what the pane shows: a page's address, a shell's screen.
    let front = tabs.active().is_some_and(|active| active.id == id);
    Some(Thing {
        label: tab.title.clone(),
        address: Address::new(Kind::Tab, format!("{}/{}", pane.0, id.0)).ok()?,
        excerpt: front.then(|| shown(hub, kind, pane)).flatten(),
    })
}

/// The most rows of a list a pane carries as its words.
const MAX_ROWS: usize = 40;

/// A page pane's words: it is a picture, so its address and how the agent can read it itself.
fn page(hub: &Hub, feed: FeedId) -> Option<String> {
    let id = page_row_id(feed);
    let url = hub.model.tables.get(feed)?.find(&id)?.words("url")?;
    let url = Some(url).filter(|url| !url.is_empty())?;
    Some(format!(
        "A live web page at {url}. This app shows it as a picture, so none of its text or HTML \
         reached this prompt. To read it, open that address with your own browser tool: \
         `agent-browser open {url}`, then `agent-browser snapshot`."
    ))
}

/// What a pane shows, as words: a shell's screen, a list pane's rows, or a page's address.
fn shown(hub: &Hub, kind: PaneKind, pane: PaneId) -> Option<String> {
    let pages = [(PaneKind::Web, feed::WEB), (PaneKind::Report, feed::REPORT)];
    if let Some((_, feed)) = pages.iter().find(|(of, _)| *of == kind) {
        return page(hub, *feed);
    }
    if kind == PaneKind::Code {
        // What it held of a file since closed is not what the pane shows now.
        let seen = hub.panes.get::<InView>(Some(pane))?;
        let held = seen.0.as_ref().map(|(file, ..)| file.as_str());
        let shown = shown_path(&hub.model, pane);
        return (shown.is_some() && shown == held).then(|| seen.words())?;
    }
    if let Some(shells) = hub.panes.get::<Shells>(Some(pane)) {
        let Tab::Shell(feed) = shells.active else {
            return None;
        };
        let surface = &shells.shell(feed)?.surface;
        let parser = surface
            .parser
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let screen = parser.screen().contents();
        return Some(screen.trim_end().to_owned()).filter(|said| !said.is_empty());
    }
    let Some(Finder::Rows(desc)) = hub.def(kind).and_then(|def| def.finder) else {
        return None;
    };
    let table = hub.model.tables.get(FeedId::new(kind.slug()))?;
    let rows = (0..table.len().min(MAX_ROWS)).filter_map(|at| table.get(at));
    let said = rows.filter_map(|r| row::thing(desc, r).ok()?.excerpt);
    let mut said: Vec<String> = said.collect();
    if table.len() > MAX_ROWS {
        said.push(format!("[…] {MAX_ROWS} of {} rows", table.len()));
    }
    Some(said.join("\n")).filter(|said| !said.is_empty())
}

/// A pane as a thing, with what it shows as its excerpt, so an attached pane is words.
pub fn whole(hub: &Hub, pane: PaneId) -> Option<Thing> {
    let kind = hub.model.grid.kind(pane)?;
    Some(Thing {
        label: kind.title().to_owned(),
        address: Address::new(Kind::Pane, pane.0.to_string()).ok()?,
        excerpt: shown(hub, kind, pane),
    })
}

/// `thing` with its words filled in when it is a pane found by its key alone.
pub fn filled(hub: &Hub, thing: Thing) -> Thing {
    let pane = thing.address.id().parse().ok().map(PaneId);
    let wants = thing.kind() == Kind::Pane && thing.excerpt.is_none();
    match pane.filter(|_| wants).and_then(|pane| whole(hub, pane)) {
        Some(whole) => Thing {
            label: thing.label,
            ..whole
        },
        None => thing,
    }
}

fn in_pane(hub: &Hub, key: &str) -> Option<Thing> {
    let (kind, pane) = pane_of(key)?;
    if let Some(tab) = tab(hub, kind, pane, key) {
        return Some(tab);
    }
    let own = own(hub, kind, pane, key);
    // A pane's own root stands for the whole pane, so picking anywhere in it finds something.
    own.or_else(|| (!key.contains('.')).then(|| whole(hub, pane)).flatten())
}

fn own(hub: &Hub, kind: PaneKind, pane: PaneId, key: &str) -> Option<Thing> {
    // The pane's own definition says how; a finder registered by hand stands in for a
    // pane that has none (a scene's stand-in pane).
    let registered = || {
        let finders = &context(hub)?.finders;
        let (_, finder) = finders.iter().find(|(known, _)| *known == kind)?;
        Some(*finder)
    };
    let finder = hub
        .def(kind)
        .and_then(|def| def.finder)
        .or_else(registered)?;
    match &finder {
        Finder::Rows(desc) => {
            let table = hub.model.tables.get(FeedId::new(kind.slug()))?;
            row::thing(desc, table.find(&row::id_of(key)?)?).ok()
        }
        Finder::Keyed(find) => find(hub, pane, key),
    }
}

/// What the element keyed `key` stands for, under its kind's name: the one lookup the boxes,
/// pick mode and the right-click menu share. Nothing for an element that is no thing.
pub fn thing_at(hub: &Hub, key: &str) -> Option<Thing> {
    let thing = thread_row(hub, key).or_else(|| in_pane(hub, key))?;
    Some(thing.named(&hub.model))
}

/// The nearest thing at or above the element at `path`, a tree path as Rune gives one: its
/// keyed ancestors joined by `/`. A key may hold `/` itself, so every run of parts is tried.
pub fn find(hub: &Hub, path: &str) -> Option<Found> {
    let cuts: Vec<usize> = path.match_indices('/').map(|(at, _)| at).collect();
    let ends = cuts.iter().copied().chain([path.len()]).rev();
    let keys = ends.flat_map(|end| {
        let after = cuts.iter().rev().filter(move |cut| **cut < end);
        let starts = after.map(|cut| cut.saturating_add(1)).chain([0]);
        starts.filter_map(move |start| path.get(start..end))
    });
    let keyed = keys.filter(|key| !key.is_empty() && !key.starts_with('#'));
    keyed.take(MAX_TRIES).find_map(|key| {
        let thing = thing_at(hub, key)?;
        let key = key.to_owned();
        Some(Found { key, thing })
    })
}

/// Keeps `key` as where the thing at `address` is drawn while it is attached, and forgets
/// every thing that no longer is.
fn remember(hub: &mut Hub, address: Address, key: String) {
    let Hub { model, panes, .. } = hub;
    let Some(context) = panes.get_mut::<Context>(None) else {
        return;
    };
    let on = |address: &Address| model.attached.number(address).is_some();
    context
        .keys
        .retain(|(kept, _)| *kept != address && on(kept));
    if on(&address) {
        context.keys.push((address, key));
    }
    context.chip = context.chip.take().filter(on);
    context.boxed = context.boxed.take().filter(on);
    context.active = context.active.take().filter(on);
}

/// Attaches what was found, or takes it off: the same step a press makes, for a scene.
pub fn attach(hub: &mut Hub, found: Found) {
    let address = found.thing.address.clone();
    step(&mut hub.model, Msg::Attach(AttachMsg::Toggle(found.thing)));
    remember(hub, address, found.key);
}

/// A press on a found thing: attached with its box where it was found, or taken off.
pub fn toggle(win: &mut Win, found: Found) {
    let address = found.thing.address.clone();
    dispatch(win, Msg::Attach(AttachMsg::Toggle(found.thing)));
    if let Ok(mut hub) = win.hub.try_borrow_mut() {
        remember(&mut hub, address, found.key);
    }
}

fn change(win: &mut Win, change: impl FnOnce(&mut Context)) {
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(context) = hub.panes.get_mut::<Context>(None)
    {
        change(context);
    }
}

/// The pointer came onto the chip of the thing at `address`, or left it.
pub fn point_chip(win: &mut Win, address: Address, inside: bool) {
    change(win, |context| {
        // Invariant: a leave that follows another chip's enter leaves that chip lit.
        match (inside, context.chip.as_ref() == Some(&address)) {
            (true, _) => context.chip = Some(address),
            (false, true) => context.chip = None,
            (false, false) => {}
        }
    });
}

/// The pointer is in the box of the thing at `address`, or in none. False when that is
/// what was already known, so a caller can skip the redraw.
pub fn point_box(win: &mut Win, address: Option<&Address>) -> bool {
    let known = win.hub.try_borrow().ok();
    let known = known.and_then(|hub| context(&hub)?.boxed.clone());
    if known.as_ref() == address {
        return false;
    }
    change(win, |context| context.boxed = address.cloned());
    true
}

/// Makes a chip the active one: the address line shows it while nothing is lit.
pub fn activate(win: &mut Win, address: Address) {
    change(win, |context| context.active = Some(address));
}

/// Starts or stops picking. Stopping forgets what the pointer was over.
pub fn pick(win: &mut Win, on: bool) {
    change(win, |context| {
        context.picking = on;
        context.hover = None;
    });
}

/// The thing the element Rune picked stands for: its own, else its nearest ancestor's.
pub fn found(hub: &Hub, picked: &Picked) -> Option<Found> {
    let own = picked.key().and_then(|key| {
        let thing = thing_at(hub, key)?;
        let key = key.to_owned();
        Some(Found { key, thing })
    });
    own.or_else(|| find(hub, picked.path()))
}

/// The pointer moved while picking: `picked` is the element under it, beneath the layer.
pub fn hovered(win: &mut Win, picked: Option<Picked>) {
    let found = picked.and_then(|picked| found(&*win.hub.try_borrow().ok()?, &picked));
    change(win, |context| context.hover = found);
}

/// A press while picking: what the pointer is over is attached, or taken off.
pub fn pressed(win: &mut Win) {
    let hover = win.hub.try_borrow().ok().and_then(|hub| {
        let context = context(&hub)?;
        context.hover.clone().filter(|_| context.picking)
    });
    if let Some(found) = hover {
        toggle(win, found);
    }
}

/// Carries out `context.pick` and `context.attach` for a control or a key. False for any
/// other command, so a caller tries its own next.
pub fn run(win: &mut Win, tx: &mut Tx, command: CommandId, target: &Target) -> bool {
    match command {
        PICK => {
            let picking = win.hub.try_borrow().ok();
            let picking = picking.is_some_and(|hub| context(&hub).is_some_and(|c| c.picking));
            pick(win, !picking);
            if !picking {
                tx.focus(LAYER, false);
            }
            true
        }
        ATTACH => {
            let thing = match (target, win.hub.try_borrow()) {
                (Target::Thing(address), Ok(hub)) => {
                    Some(filled(&hub, Thing::at(&hub.model, address.clone())))
                }
                (Target::Thing(_) | Target::App | Target::Pane(_), _) => None,
            };
            if let Some(thing) = thing {
                dispatch(win, Msg::Attach(AttachMsg::Toggle(thing)));
            }
            true
        }
        _ => false,
    }
}
