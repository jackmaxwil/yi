//! The Mac menus and the two status items, rows of the command tables.

use std::cell::RefCell;
use std::sync::OnceLock;

use rune::{AppHandle, MenuItem, Shortcut, Tx};
use yd_core::address::uri::Address;
use yd_core::command::table::{Command, Route, Run, Target};
use yd_core::ids::{ApprovalId, ThreadId, WindowId};
use yd_core::kind::Kind;
use yd_core::msg::Msg;
use yd_core::reduce::approvals::OptionKind;
use yd_core::reduce::state::Wire;
use yd_core::reduce::step::commands::{ANSWER, CANCEL, NEW};
use yd_core::reduce::threads::Want;
use yd_mac::{Row, RowId, StatusItem};
use yd_panes::notifications::view::{Bell, Item};
use yd_ui::grid_view::picker;
use yd_ui::menu::command_box;
use yd_ui::shown::{show, thread};
use yd_ui::slots::SLOTS;
use yd_ui::win::{Hub, Shared, Win, dispatch};

use crate::shell::view;

type Action = fn(&mut Win, &mut Tx);

/// Rows a status menu lists under each heading before it says how many more there are.
const LISTED: usize = 8;

/// One item of the Mac menu bar: its row of the command table, and whether anything carries
/// the command out yet.
#[derive(Debug, Clone, Copy)]
pub struct Entry {
    pub command: &'static Command,
    pub enabled: bool,
}

/// The Mac menu bar as the command table gives it (`command_box::menu_bar`), menu by menu,
/// less what a menu cannot aim. Read once: the tables are static.
pub fn table(hub: &Hub) -> &'static [Entry] {
    static TABLE: OnceLock<Vec<Entry>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let commands = command_box::table(hub);
        let menus = command_box::menu_bar(hub);
        let items = menus.into_iter().flat_map(|menu| menu.items);
        let rows = items.filter_map(|item| {
            let command = commands.find(item.command)?;
            let enabled = item.enabled;
            Some(Entry { command, enabled })
        });
        let rows = rows.filter(|entry| aimable(entry.command));
        rows.take(SLOTS).collect()
    })
}

/// A menu has no pointer to aim with: it can run what is aimed at the app, at the thread the
/// window shows, at that thread's waiting ask, or at its folder.
fn aimable(command: &Command) -> bool {
    let kinds = command.kinds;
    kinds.is_empty()
        || kinds
            .iter()
            .all(|kind| matches!(kind, Kind::Thread | Kind::Approval | Kind::Place))
}

/// What `command` is aimed at in this window, or none when the window has nothing for it.
fn aim(hub: &Hub, window: WindowId, command: &Command) -> Option<Target> {
    if command.kinds.is_empty() {
        return Some(Target::App);
    }
    let shown = thread(hub, window)?;
    let at = |kind, id: &str| Address::new(kind, id).ok().map(Target::Thing);
    match command.kinds.first()? {
        Kind::Thread => at(Kind::Thread, shown.id.as_str()),
        Kind::Place => at(Kind::Place, &shown.cwd),
        Kind::Approval => {
            let mut asks = hub.model.wire.waiting();
            let ask = asks.find(|ask| ask.thread == shown.id)?;
            at(Kind::Approval, ask.id.as_str())
        }
        _ => None,
    }
}

fn menu_item<const I: usize>(win: &mut Win, tx: &mut Tx) {
    let aimed = win.hub.try_borrow().ok().and_then(|hub| {
        let command = table(&hub).get(I)?.command;
        Some((command.id, aim(&hub, win.window, command)?))
    });
    if let Some((command, target)) = aimed {
        view::run(win, tx, command, target);
    }
}

const ACTIONS: [Action; SLOTS] = yd_ui::slots!(menu_item);

/// The chord and the title one menu item wears.
type Worn = (Option<String>, Option<String>);

thread_local! {
    /// What each menu item wore when the Mac's menus were last put up.
    static WORN: RefCell<Option<Vec<Worn>>> = const { RefCell::new(None) };
}

/// The chord and the title each item of [`table`] wears now, in the table's order; a kept
/// layout's number that nobody keeps has no title and is not put up.
fn worn(hub: &Hub) -> Vec<Worn> {
    let keys = command_box::keymap(hub);
    let wears = |entry: &Entry| {
        let title = picker::title(&hub.model.grid, entry.command);
        let chord = keys.key(entry.command.id).map(str::to_owned);
        (chord, title.map(|title| title.into_owned()))
    };
    table(hub).iter().map(wears).collect()
}

/// The items to put up again once a key or a title in them has moved since they were last
/// put up, so a key changed in Settings or a layout kept shows in the Mac's menus at once.
pub fn due(hub: &Hub) -> Option<Vec<MenuItem<Win>>> {
    // ponytail: the map is built on every poll; gate on a settings revision if it shows.
    let now = worn(hub);
    let current = WORN.with(|kept| {
        kept.try_borrow()
            .is_ok_and(|kept| kept.as_ref() == Some(&now))
    });
    (!current).then(|| items(hub))
}

/// Records that the menus as [`items`] gives them now are the ones AppKit holds.
pub fn put_up(hub: &Hub) {
    let now = worn(hub);
    WORN.with(|kept| {
        if let Ok(mut kept) = kept.try_borrow_mut() {
            *kept = Some(now);
        }
    });
}

/// The menu bar's own items, one a row of [`table`]. A key reaches the window's own handlers
/// first, so Return still sends a message and Esc still closes a panel.
pub fn items(hub: &Hub) -> Vec<MenuItem<Win>> {
    let keys = command_box::keymap(hub);
    let item = |(entry, action): (&Entry, Action)| {
        let command = entry.command;
        let title = picker::title(&hub.model.grid, command)?;
        let made = MenuItem::new(title, action)
            .id(command.id.0)
            .menu(command.menu.unwrap_or("Thread"))
            .enabled(entry.enabled)
            .keys_first();
        match keys
            .key(command.id)
            .and_then(|key| key.parse::<Shortcut>().ok())
        {
            Some(key) => Some(made.key_equivalent(key)),
            None => Some(made),
        }
    };
    table(hub).iter().zip(ACTIONS).filter_map(item).collect()
}

/// What picking a row of a status menu does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    Open(ThreadId),
    /// Answers the ask with the option that allows it once.
    Allow(ApprovalId, String),
    NewThread,
    ShowWindows,
    StopAll,
}

/// The one item's title and its menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Menu {
    /// What needs you, else what is working: one item, however much is going on.
    pub title: String,
    pub rows: Vec<Row>,
    /// What each [`RowId`] in `rows` does: the row's number is its place here.
    pub picks: Vec<Pick>,
}

fn row(menu: &mut Menu, title: String, pick: Pick) {
    let id = RowId(u32::try_from(menu.picks.len()).unwrap_or(u32::MAX));
    // Return picks the row that allows: AppKit draws its key at the row's right.
    let made = match pick {
        Pick::Allow(..) => Row::Return { id, title },
        Pick::Open(_) | Pick::NewThread | Pick::ShowWindows | Pick::StopAll => {
            Row::Item { id, title }
        }
    };
    menu.rows.push(made);
    menu.picks.push(pick);
}

fn clip(words: &str) -> String {
    words.chars().take(64).collect()
}

/// The counts and rows of the status menu: what the bell holds that waits on the user, in
/// the bell's own words, then what is at work.
pub fn menu(hub: &Hub) -> Menu {
    let wire = &hub.model.wire;
    let now = hub.model.now;
    let working = wire.attention().working;
    let bell = hub.panes.get::<Bell>(None);
    let shown = bell.map(Bell::shown).unwrap_or_default();
    let needs: Vec<&Item> = shown
        .into_iter()
        .filter(|item| item.needs_you() && !item.read)
        .collect();
    let title = match (needs.len(), working.len()) {
        (0, working) => format!("◐ {working}"),
        (needs, _) => format!("● {needs}"),
    };
    let mut menu = Menu {
        title,
        rows: Vec::new(),
        picks: Vec::new(),
    };
    if needs.is_empty() && working.is_empty() {
        let nothing = yd_panes::notifications::states::WORDS.empty.what;
        menu.rows.push(Row::Header(nothing.to_owned()));
    }
    if !needs.is_empty() {
        menu.rows
            .push(Row::Header(format!("{} need you", needs.len())));
    }
    let mut first_ask = true;
    for item in needs.iter().take(LISTED) {
        let title = clip(&format!("{} — {}", item.title, item.detail));
        match (&item.want, allow_once(wire, &item.want)) {
            (Want::Approval(id), Some(option)) if first_ask => {
                first_ask = false;
                row(
                    &mut menu,
                    format!("{title}   Allow"),
                    Pick::Allow(id.clone(), option),
                );
            }
            _ => row(&mut menu, title, Pick::Open(item.thread.clone())),
        }
    }
    more(&mut menu, needs.len(), "in the bell");
    if !working.is_empty() {
        menu.rows
            .push(Row::Header(format!("{} working", working.len())));
    }
    for id in working.iter().take(LISTED) {
        let thread = wire.threads.get(id);
        let name = thread
            .and_then(|thread| thread.title.clone())
            .unwrap_or_else(|| id.to_string());
        let since = thread.map_or(now, |thread| thread.last_activity);
        row(
            &mut menu,
            clip(&format!("{name} · {}", now.age(since))),
            Pick::Open(id.clone()),
        );
    }
    more(&mut menu, working.len(), "working");
    menu.rows.push(Row::Separator);
    row(&mut menu, "New thread   ⌘N".to_owned(), Pick::NewThread);
    row(
        &mut menu,
        "Show all windows   ⌃⌘W".to_owned(),
        Pick::ShowWindows,
    );
    row(&mut menu, "Stop all agents".to_owned(), Pick::StopAll);
    menu
}

/// The option of the ask `want` names that allows it once, when it offers one.
fn allow_once(wire: &Wire, want: &Want) -> Option<String> {
    let Want::Approval(id) = want else {
        return None;
    };
    let ask = wire.waiting().find(|ask| &ask.id == id)?;
    let once = ask
        .offers()
        .find(|offer| offer.kind == OptionKind::AllowOnce);
    once.map(|offer| offer.id.0.to_owned())
}

/// A heading that counts the rows a list left out, when it left any out.
fn more(menu: &mut Menu, total: usize, where_: &str) {
    let rest = total.saturating_sub(LISTED);
    if rest > 0 {
        menu.rows
            .push(Row::Header(format!("… and {rest} more {where_}")));
    }
}

/// Brings every window forward.
pub(crate) fn raise(hub: &Hub) {
    for (_, app) in &hub.wins {
        app.show();
        app.focus();
    }
}

/// Carries out a picked row, from the window whose state it reaches.
pub fn pick(win: &mut Win, pick: &Pick) {
    match pick {
        Pick::Open(id) => {
            show(win, id.clone());
            if let Ok(hub) = win.hub.try_borrow() {
                raise(&hub);
            }
        }
        Pick::Allow(ask, option) => {
            let Ok(address) = Address::new(Kind::Approval, ask.as_str()) else {
                return;
            };
            let run = Run::new(ANSWER).on(Target::Thing(address));
            dispatch(
                win,
                Msg::Command(
                    Route::Wire,
                    Run {
                        text: Some(option.clone()),
                        ..run
                    },
                ),
            );
        }
        Pick::NewThread => {
            let place = win.hub.try_borrow().ok().and_then(|hub| {
                let shown = thread(&hub, win.window)?;
                Address::new(Kind::Place, &shown.cwd).ok()
            });
            let target = place.map(Target::Thing).unwrap_or_default();
            dispatch(win, Msg::Command(Route::Wire, Run::new(NEW).on(target)));
        }
        Pick::ShowWindows => {
            if let Ok(hub) = win.hub.try_borrow() {
                raise(&hub);
            }
        }
        Pick::StopAll => {
            let working = win
                .hub
                .try_borrow()
                .ok()
                .map(|hub| hub.model.wire.attention().working);
            for id in working.unwrap_or_default() {
                let Ok(address) = Address::new(Kind::Thread, id.as_str()) else {
                    continue;
                };
                dispatch(
                    win,
                    Msg::Command(Route::Wire, Run::new(CANCEL).on(Target::Thing(address))),
                );
            }
        }
    }
}

/// The one item in the system bar, and the menu it was last given.
struct Items {
    item: StatusItem,
    shown: Option<Menu>,
}

thread_local! {
    // Invariant: a status item is made and used on the main thread only, and is not `Send`.
    static ITEMS: RefCell<Option<Items>> = const { RefCell::new(None) };
}

/// Adds the item to the system bar; they pick through `handle`, which reaches a window's
/// state from outside a Rune turn. Main thread only; a second call changes nothing.
pub fn start(handle: &AppHandle<Win>) {
    ITEMS.with(|items| {
        let Ok(mut items) = items.try_borrow_mut() else {
            return;
        };
        if items.is_some() {
            return;
        }
        let picker = |handle: AppHandle<Win>| {
            move |id: RowId| {
                handle.update(move |win, _| {
                    let picked = ITEMS.with(|items| {
                        let items = items.try_borrow().ok()?;
                        let menu = items.as_ref()?.shown.as_ref()?;
                        menu.picks.get(usize::try_from(id.0).ok()?).cloned()
                    });
                    if let Some(picked) = picked {
                        pick(win, &picked);
                    }
                });
            }
        };
        if let Ok(item) = StatusItem::new(picker(handle.clone())) {
            *items = Some(Items { item, shown: None });
        }
    });
}

/// Gives the items the counts and rows the model now says; nothing is sent to AppKit when
/// they have not changed.
pub fn refresh(shared: &Shared) {
    let Ok(hub) = shared.try_borrow() else { return };
    // The Mac's menus live on the main window's app: they follow the key map from here.
    let main = hub.wins.iter().find(|(id, _)| *id == crate::root::MAIN);
    if let Some((_, app)) = main
        && let Some(items) = due(&hub)
        && app.set_menu(items).is_ok()
    {
        put_up(&hub);
    }
    let now = menu(&hub);
    ITEMS.with(|items| {
        let Ok(mut items) = items.try_borrow_mut() else {
            return;
        };
        let Some(items) = items
            .as_mut()
            .filter(|items| items.shown.as_ref() != Some(&now))
        else {
            return;
        };
        if items.item.set(&now.title, &now.rows).is_ok() {
            items.shown = Some(now);
        }
    });
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    use std::error::Error;
    use std::rc::Rc;

    use yd_core::msg::Effect;
    use yd_core::wire::replay::play;

    use super::*;

    pub(crate) type Sent = Rc<RefCell<Vec<String>>>;

    /// A window over the bell's mockup threads, and every request it sends.
    pub(crate) fn window() -> Result<(Win, Sent), Box<dyn Error>> {
        let shared = Hub::new(yd_panes::registry::PANES, Vec::new());
        let sent: Sent = Rc::default();
        {
            let mut hub = shared.try_borrow_mut()?;
            let log = Rc::clone(&sent);
            hub.set_sink(move |effect| {
                if let Effect::Wire(request) = effect {
                    log.borrow_mut().push(request.line);
                }
            });
            let mut scenes = yd_panes::registry::scenes();
            let scene = scenes.find(|scene| scene.name == "notifications/mockup");
            (scene.ok_or("no scene")?.setup)(&mut hub);
        }
        Ok((Win::new(shared, WindowId(0)), sent))
    }

    /// The server's list with one thread at work, as `session/list` writes a row.
    const WORKING: &str = r#"{"dir":"send","frame":{"jsonrpc":"2.0","id":9,"method":"session/list","params":{}}}
{"dir":"recv","frame":{"jsonrpc":"2.0","id":9,"result":{"sessions":[{"sessionId":"sessions","cwd":"/work/atlas-api","title":"Sessions","_meta":{"yi":{"unseen":0,"lastState":"running","lastEventMs":1794678944000,"pinnedMs":null}}}]}}}"#;

    fn titles(menu: &Menu) -> Vec<&str> {
        fn title(row: &Row) -> &str {
            match row {
                Row::Header(title) | Row::Item { title, .. } | Row::Return { title, .. } => {
                    title.as_str()
                }
                Row::Separator => "-",
            }
        }
        menu.rows.iter().map(title).collect()
    }

    #[test]
    fn the_status_menu_lists_what_needs_you_and_the_first_ask_allows_once()
    -> Result<(), Box<dyn Error>> {
        let (win, _) = window()?;
        let hub = win.hub.try_borrow()?;
        let menu = menu(&hub);
        assert_eq!(menu.title, "● 2", "one item, what needs you first");
        let want = [
            "2 need you",
            "Rate limit — Wants to run cargo test -p api upload::   Allow",
            "Cold start — Asks: profile the release build?",
            "-",
            "New thread   ⌘N",
            "Show all windows   ⌃⌘W",
            "Stop all agents",
        ];
        assert_eq!(titles(&menu), want);
        // The mockup's "Allow ↵": Return is the row's own key, not words in its title.
        let keyed = menu
            .rows
            .iter()
            .filter(|row| matches!(row, Row::Return { .. }));
        assert_eq!(keyed.count(), 1, "one row takes Return: {:?}", menu.rows);
        assert!(matches!(menu.rows.get(1), Some(Row::Return { .. })));
        let allow = menu.picks.first();
        let ask = ApprovalId::new("perm_rate-limit_1");
        assert_eq!(allow, Some(&Pick::Allow(ask, "allow_once".to_owned())));
        Ok(())
    }

    #[test]
    fn picking_the_allow_row_answers_the_ask_and_a_thread_row_shows_the_thread()
    -> Result<(), Box<dyn Error>> {
        let (mut win, sent) = window()?;
        let ask = ApprovalId::new("perm_rate-limit_1");
        pick(&mut win, &Pick::Allow(ask, "allow_once".to_owned()));
        let lines = sent.borrow();
        assert!(
            lines.iter().any(|line| line.contains("allow_once")),
            "{lines:?}"
        );
        drop(lines);
        pick(&mut win, &Pick::Open(ThreadId::new("cold-start")));
        let hub = win.hub.try_borrow()?;
        let shown = thread(&hub, win.window).map(|thread| thread.id.to_string());
        assert_eq!(shown.as_deref(), Some("cold-start"));
        Ok(())
    }

    #[test]
    fn a_working_thread_is_listed_and_stop_all_stops_it() -> Result<(), Box<dyn Error>> {
        let (mut win, sent) = window()?;
        play(&mut win.hub.try_borrow_mut()?.model, WORKING)?;
        {
            let hub = win.hub.try_borrow()?;
            let menu = menu(&hub);
            assert_eq!(menu.title, "● 2", "what needs you before what works");
            assert!(titles(&menu).contains(&"1 working"));
            assert!(
                titles(&menu).contains(&"Sessions · 3m"),
                "{:?}",
                titles(&menu)
            );
        }
        pick(&mut win, &Pick::StopAll);
        let lines = sent.borrow();
        let stopped = lines
            .iter()
            .any(|line| line.contains("session/cancel") && line.contains("sessions"));
        assert!(stopped, "{lines:?}");
        Ok(())
    }

    #[test]
    fn nothing_waiting_says_so() -> Result<(), Box<dyn Error>> {
        let shared = Hub::new(&[], Vec::new());
        let hub = shared.try_borrow()?;
        let menu = menu(&hub);
        assert_eq!(titles(&menu).first(), Some(&"Nothing needs you"));
        Ok(())
    }

    #[test]
    fn the_menu_bar_lists_each_command_once_and_every_key_parses() -> Result<(), Box<dyn Error>> {
        let (win, _) = window()?;
        let hub = win.hub.try_borrow()?;
        let commands: Vec<&Command> = table(&hub).iter().map(|entry| entry.command).collect();
        let ids: BTreeSet<&str> = commands.iter().map(|command| command.id.0).collect();
        assert_eq!(
            ids.len(),
            commands.len(),
            "Rune refuses two items with one id"
        );
        let kept = |command: &&&Command| picker::title(&hub.model.grid, command).is_some();
        let put_up = commands.iter().filter(kept).count();
        assert_eq!(
            items(&hub).len(),
            put_up,
            "a number nobody keeps is not put up"
        );
        assert!(
            commands.len() < SLOTS,
            "a command past the pool has no item"
        );
        for command in &commands {
            let key = command.key.unwrap_or("a");
            assert!(
                key.parse::<Shortcut>().is_ok(),
                "{} has key {key:?}",
                command.id.0
            );
        }
        let names: BTreeSet<&str> = commands.iter().filter_map(|command| command.menu).collect();
        assert_eq!(
            names,
            BTreeSet::from(["File", "Help", "Thread", "View", "Window", "Yi"])
        );
        Ok(())
    }

    /// A key changed in Settings: the menus are due again, wearing it, and once put up they
    /// are current until the next change.
    #[test]
    fn a_key_the_user_changes_is_due_on_the_mac_menus_at_once() -> Result<(), Box<dyn Error>> {
        use yd_core::settings::schema::SettingValue;
        use yd_core::settings::store::SettingsMsg;
        let (mut win, _) = window()?;
        {
            let mut hub = win.hub.try_borrow_mut()?;
            let keys = yd_core::command::keys::settings(&command_box::table(&hub));
            hub.model.settings.rows.extend(keys);
            put_up(&hub);
            assert!(due(&hub).is_none(), "nothing moved since the menus went up");
        }
        let chord = SettingValue::Text("cmd+shift+y".into());
        let key = yd_core::ids::SettingKey::fixed("keys.thread.new");
        dispatch(&mut win, Msg::Settings(SettingsMsg::Set(key, Some(chord))));
        let hub = win.hub.try_borrow()?;
        let at = table(&hub).iter().position(|entry| entry.command.id == NEW);
        let at = at.ok_or("New thread is not in the menus")?;
        let chord = worn(&hub).get(at).and_then(|(chord, _)| chord.clone());
        assert_eq!(chord.as_deref(), Some("cmd+shift+y"));
        assert_eq!(due(&hub).map(|items| items.len()), Some(items(&hub).len()));
        put_up(&hub);
        assert!(due(&hub).is_none(), "put up, so current");
        Ok(())
    }

    #[test]
    fn a_command_is_aimed_at_what_the_window_shows() -> Result<(), Box<dyn Error>> {
        let (win, _) = window()?;
        let hub = win.hub.try_borrow()?;
        let rows = table(&hub).iter().map(|entry| entry.command);
        let find = |id: &str| rows.clone().find(|command| command.id.0 == id);
        let stop = find("thread.cancel").ok_or("no Stop")?;
        let aimed = aim(&hub, win.window, stop);
        let shown = thread(&hub, win.window).ok_or("no thread")?;
        let want = Address::new(Kind::Thread, shown.id.as_str())?;
        assert_eq!(aimed, Some(Target::Thing(want)));
        let allow = find("approval.allow").ok_or("no Allow Once")?;
        let ask = hub.model.wire.waiting().find(|ask| ask.thread == shown.id);
        let ask = ask.ok_or("the shown thread has no ask")?;
        let want = Address::new(Kind::Approval, ask.id.as_str())?;
        assert_eq!(aim(&hub, win.window, allow), Some(Target::Thing(want)));
        let flip = find("layout.flip-chat").ok_or("no flip")?;
        assert_eq!(aim(&hub, win.window, flip), Some(Target::App));
        Ok(())
    }
}
