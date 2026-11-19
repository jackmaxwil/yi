//! A menu-bar status item: a title that carries a live count, and a menu of rows.

use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};

use objc2::rc::Retained;
use objc2::runtime::{NSObject, Sel};
use objc2::{ClassType, DeclaredClass, declare_class, msg_send, msg_send_id, mutability, sel};
use objc2_app_kit::{NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength};
use objc2_foundation::{MainThreadMarker, NSString};

/// Which row of a status menu was picked. The caller chooses the numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RowId(pub u32);

/// One row of a status menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// A greyed section title, such as "2 need you".
    Header(String),
    /// A row the user can pick.
    Item {
        id: RowId,
        title: String,
    },
    /// A row the user can pick, which Return also picks while the menu is open.
    Return {
        id: RowId,
        title: String,
    },
    Separator,
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StatusError {
    #[error("status items are made on the main thread")]
    NotMainThread,
    #[error("the system gave the status item no button")]
    NoButton,
    #[error("the status menu has no row {index}")]
    NoRow { index: usize },
}

type Pick = Box<dyn Fn(RowId)>;

declare_class!(
    struct Target;

    // SAFETY: NSObject has no subclassing rules, the ivars are only touched on the main
    // thread, which `MainThreadOnly` enforces, and `Target` has no `Drop`.
    unsafe impl ClassType for Target {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "YdStatusMenuTarget";
    }

    impl DeclaredClass for Target {
        type Ivars = Pick;
    }

    // SAFETY: `pick:` has the shape AppKit sends an action with: the sender, no return.
    unsafe impl Target {
        #[method(pick:)]
        fn pick(&self, sender: &NSMenuItem) {
            // SAFETY: only rows `StatusItem::row` made carry this target, so the sender is a
            // live menu item, and AppKit sends its action on the main thread.
            let tag = unsafe { sender.tag() };
            let Ok(id) = u32::try_from(tag) else { return };
            // Invariant: a handler may drop the item that owns this target; the retain keeps
            // `self` and its handler alive until the call returns.
            let _alive = self.retain();
            // Invariant: a panic may not unwind into AppKit's frames, so it stops here.
            if catch_unwind(AssertUnwindSafe(|| (self.ivars())(RowId(id)))).is_err() {
                // `eprintln!` panics when stderr is gone, and this frame may not panic.
                let _ = writeln!(std::io::stderr(), "yd-mac: a status menu handler panicked");
            }
        }
    }
);

/// A status item in the system menu bar. Dropping it takes it out of the bar.
pub struct StatusItem {
    item: Retained<NSStatusItem>,
    menu: Retained<NSMenu>,
    target: Retained<Target>,
    // Invariant: the marker is not `Send`, so every method runs on the main thread.
    mtm: MainThreadMarker,
}

impl StatusItem {
    /// Adds an empty item to the menu bar; `on_pick` runs on the main thread.
    /// Fails with [`StatusError::NotMainThread`] anywhere else.
    pub fn new(on_pick: impl Fn(RowId) + 'static) -> Result<Self, StatusError> {
        let mtm = MainThreadMarker::new().ok_or(StatusError::NotMainThread)?;
        let ivars: Pick = Box::new(on_pick);
        let alloc = mtm.alloc::<Target>().set_ivars(ivars);
        // SAFETY: `init` on a new NSObject subclass whose ivars were just set.
        let target: Retained<Target> = unsafe { msg_send_id![super(alloc), init] };
        // SAFETY: on the main thread; the system bar lives as long as the process.
        let bar = unsafe { NSStatusBar::systemStatusBar() };
        // SAFETY: on the main thread; the length is AppKit's own constant.
        let item = unsafe { bar.statusItemWithLength(NSVariableStatusItemLength) };
        let menu = NSMenu::new(mtm);
        // SAFETY: on the main thread; the item retains the menu it is given.
        unsafe { item.setMenu(Some(&menu)) };
        Ok(Self {
            item,
            menu,
            target,
            mtm,
        })
    }

    /// Shows `title` in the bar and replaces the menu's rows.
    pub fn set(&self, title: &str, rows: &[Row]) -> Result<(), StatusError> {
        // SAFETY: on the main thread (`mtm`); reads a property of an item this owns.
        let button = unsafe { self.item.button(self.mtm) }.ok_or(StatusError::NoButton)?;
        // SAFETY: on the main thread; the button copies the string.
        unsafe { button.setTitle(&NSString::from_str(title)) };
        self.clear();
        for row in rows {
            self.menu.addItem(&self.row(row));
        }
        Ok(())
    }

    /// The title the bar shows now, read back from AppKit.
    pub fn title(&self) -> Option<String> {
        // SAFETY: on the main thread (`mtm`); reads a property of an item this owns.
        let button = unsafe { self.item.button(self.mtm) }?;
        // SAFETY: on the main thread; `title` returns a retained copy.
        Some(unsafe { button.title() }.to_string())
    }

    /// Picks the row at `index` as a click would, for drive mode and tests.
    pub fn pick_row(&self, index: usize) -> Result<(), StatusError> {
        // SAFETY: on the main thread; counts the rows of a menu this owns.
        let count = unsafe { self.menu.numberOfItems() };
        let at = isize::try_from(index).ok().filter(|i| *i < count);
        let at = at.ok_or(StatusError::NoRow { index })?;
        // SAFETY: on the main thread; `at` was checked against the row count above.
        unsafe { self.menu.performActionForItemAtIndex(at) };
        Ok(())
    }

    /// Empties the menu, first cutting each row loose from the target.
    fn clear(&self) {
        // SAFETY: on the main thread (`mtm`); counts the rows of a menu this owns.
        let count = unsafe { self.menu.numberOfItems() };
        for index in 0..count {
            // SAFETY: on the main thread; `index` is below the count just read.
            let Some(item) = (unsafe { self.menu.itemAtIndex(index) }) else {
                continue;
            };
            // SAFETY: on the main thread, on a row just read from the menu. AppKit does not
            // retain a target: a row an open menu still holds must not point at a freed one.
            unsafe { item.setTarget(None) };
        }
        // SAFETY: on the main thread; every row was cut loose just above.
        unsafe { self.menu.removeAllItems() };
    }

    fn row(&self, row: &Row) -> Retained<NSMenuItem> {
        match row {
            Row::Separator => NSMenuItem::separatorItem(self.mtm),
            Row::Header(title) => self.item_titled(title, None),
            Row::Item { id, title } | Row::Return { id, title } => {
                let item = self.item_titled(title, Some(sel!(pick:)));
                if matches!(row, Row::Return { .. }) {
                    // SAFETY: on the main thread; the item copies the string it is given.
                    unsafe { item.setKeyEquivalent(&NSString::from_str("\r")) };
                    // SAFETY: on the main thread; no flags is a valid mask (AppKit's own
                    // default is Command), and the argument is the NSUInteger the method takes.
                    let _: () = unsafe { msg_send![&item, setKeyEquivalentModifierMask: 0usize] };
                }
                // SAFETY: on the main thread; `clear` unsets this before `target` can go.
                unsafe { item.setTarget(Some(&self.target)) };
                // SAFETY: on the main thread; a tag is a plain integer.
                unsafe { item.setTag(isize::try_from(id.0).unwrap_or(-1)) };
                item
            }
        }
    }

    fn item_titled(&self, title: &str, action: Option<Sel>) -> Retained<NSMenuItem> {
        let (title, key) = (NSString::from_str(title), NSString::from_str(""));
        let alloc = self.mtm.alloc::<NSMenuItem>();
        // SAFETY: on the main thread; an alloc and init pair with live string arguments.
        unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(alloc, &title, action, &key) }
    }
}

impl Drop for StatusItem {
    fn drop(&mut self) {
        self.clear();
        // SAFETY: on the main thread (`mtm`); the system bar lives as long as the process.
        let bar = unsafe { NSStatusBar::systemStatusBar() };
        // SAFETY: on the main thread; removes the item this bar made in `new`.
        unsafe { bar.removeStatusItem(&self.item) };
    }
}

#[cfg(test)]
mod tests {
    use super::{StatusError, StatusItem};

    #[test]
    fn a_status_item_made_off_the_main_thread_is_refused() -> Result<(), Box<dyn std::error::Error>>
    {
        let made = std::thread::spawn(|| StatusItem::new(|_| {}).map(drop)).join();
        let made = made.map_err(|_| "the thread that tried panicked")?;
        assert!(
            matches!(made, Err(StatusError::NotMainThread)),
            "got {made:?}"
        );
        Ok(())
    }
}
