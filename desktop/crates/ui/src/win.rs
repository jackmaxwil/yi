//! What every window shares on the UI thread, each window's own state, and the one place a
//! change is applied: step the model, hand its effects on, wake the other windows.

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use rune::{AppHandle, TextEdit};
use yd_core::ids::{PaneId, WindowId};
use yd_core::inbox::Inbox;
use yd_core::model::Model;
use yd_core::msg::{Effect, Msg, step};
use yd_core::pane::PaneKind;

use crate::pane::{PaneDef, Scene, Stage};

pub type Shared = Rc<RefCell<Hub>>;

/// Which text field of a window: the pane it is in (none for the window's own, such as the
/// command box) and the field's name there.
pub type Field = (Option<PaneId>, &'static str);

/// The text fields one window edits. Invariant: Rune keeps a field's focus and caret in
/// the edit the view draws, so a view draws these and never a copy kept elsewhere.
pub type Edits = HashMap<Field, TextEdit>;

/// The field `name` of `pane`.
pub const fn field(pane: PaneId, name: &'static str) -> Field {
    (Some(pane), name)
}

/// State kept by type: one value of each type per pane, or per holder when no pane is named.
#[derive(Default)]
pub struct Local(HashMap<(Option<PaneId>, TypeId), Box<dyn Any>>);

impl Local {
    pub fn get<T: 'static>(&self, pane: Option<PaneId>) -> Option<&T> {
        self.0.get(&(pane, TypeId::of::<T>()))?.downcast_ref()
    }

    /// The value of type `T`, made from its default the first time it is asked for.
    pub fn get_mut<T: Default + 'static>(&mut self, pane: Option<PaneId>) -> Option<&mut T> {
        self.0
            .entry((pane, TypeId::of::<T>()))
            .or_insert_with(|| Box::new(T::default()))
            .downcast_mut()
    }

    /// Hands everything kept for pane `from` to pane `to`: how state built for a pane drawn
    /// alone follows that pane into a grid.
    pub fn adopt(&mut self, from: PaneId, to: PaneId) {
        let kept: Vec<_> = self.0.keys().copied().collect();
        for key in kept.into_iter().filter(|(pane, _)| *pane == Some(from)) {
            if let Some(state) = self.0.remove(&key) {
                self.0.insert((Some(to), key.1), state);
            }
        }
    }
}

/// The model and everything else the windows share. One per process, on the UI thread.
pub struct Hub {
    pub model: Model,
    /// Each pane's own state. It is here, not in a window, so it travels with the pane.
    pub panes: Local,
    /// What a window draws: the app, or one pane or view alone for a scene.
    pub stage: Stage,
    pub defs: &'static [&'static PaneDef],
    pub scenes: Vec<&'static Scene>,
    /// Every open window, for waking the others after a change.
    pub wins: Vec<(WindowId, AppHandle<Win>)>,
    inbox: Option<Inbox>,
    sink: Option<Box<dyn FnMut(Effect)>>,
    /// Effects asked while a sink was awaited: frame 0 lays panes out before the ready hook.
    held: Option<Vec<Effect>>,
}

/// The most effects kept for a sink still to come; later ones are dropped as without one.
const HELD: usize = 4096;

impl Hub {
    pub fn new(defs: &'static [&'static PaneDef], scenes: Vec<&'static Scene>) -> Shared {
        Rc::new(RefCell::new(Self {
            model: Model::default(),
            panes: Local::default(),
            stage: Stage::App,
            defs,
            scenes,
            wins: Vec::new(),
            inbox: None,
            sink: None,
            held: None,
        }))
    }

    pub fn def(&self, kind: PaneKind) -> Option<&'static PaneDef> {
        self.defs.iter().copied().find(|def| def.kind == kind)
    }

    /// The channel worker threads send into; `drain` empties it.
    pub fn set_inbox(&mut self, inbox: Inbox) {
        self.inbox = Some(inbox);
    }

    /// Who carries effects out. Without one they are dropped: a scene has no world to ask.
    pub fn set_sink(&mut self, sink: impl FnMut(Effect) + 'static) {
        self.sink = Some(Box::new(sink));
    }

    /// Keeps the effects asked before a sink is set, for the sink to carry first.
    pub fn hold(&mut self) {
        self.held.get_or_insert_with(Vec::new);
    }
}

/// One window's state: the shared hub and what belongs to this window alone.
pub struct Win {
    pub hub: Shared,
    pub window: WindowId,
    /// Rune state that stays with this window: drag states, hover, an open menu.
    pub local: Local,
    pub edits: Edits,
}

impl Win {
    pub fn new(hub: Shared, window: WindowId) -> Self {
        Self {
            hub,
            window,
            local: Local::default(),
            edits: Edits::default(),
        }
    }
}

/// Applies `msg` from the window `win`. Call it from handlers, never from a view.
pub fn dispatch(win: &mut Win, msg: Msg) {
    let effects = match win.hub.try_borrow_mut() {
        Ok(mut hub) => step(&mut hub.model, msg),
        Err(_) => return,
    };
    deliver(&win.hub, effects);
    wake_others(&win.hub, win.window);
}

/// Steps what worker threads sent since the last frame; true when anything changed.
pub fn drain(win: &mut Win) -> bool {
    let drained = match win.hub.try_borrow_mut() {
        Ok(mut hub) => {
            let Hub { model, inbox, .. } = &mut *hub;
            match inbox {
                Some(inbox) => inbox.drain(model),
                None => return false,
            }
        }
        Err(_) => return false,
    };
    let changed = drained.steps > 0;
    // Effects held for the sink go with the first drain after it is set.
    deliver(&win.hub, drained.effects);
    if changed {
        wake_others(&win.hub, win.window);
    }
    changed
}

fn deliver(shared: &Shared, effects: Vec<Effect>) {
    // Invariant: the sink runs with the hub unborrowed, so it may open a window or step again.
    let Ok(mut hub) = shared.try_borrow_mut() else {
        return;
    };
    let Some(mut sink) = hub.sink.take() else {
        if let Some(held) = &mut hub.held {
            let room = HELD.saturating_sub(held.len());
            held.extend(effects.into_iter().take(room));
        }
        return;
    };
    let held = hub.held.take().unwrap_or_default();
    drop(hub);
    held.into_iter().chain(effects).for_each(&mut sink);
    if let Ok(mut hub) = shared.try_borrow_mut() {
        hub.sink = Some(sink);
    }
}

/// One empty update per other window, so its view runs again and reads the changed model.
/// A poll that changed what the hub holds without a message calls it too.
pub fn wake_others(shared: &Shared, me: WindowId) {
    let Ok(hub) = shared.try_borrow() else { return };
    hub.wins
        .iter()
        .filter(|(id, _)| *id != me)
        .for_each(|(_, app)| app.update(|_, _| {}));
}
