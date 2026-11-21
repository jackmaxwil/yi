//! Findings pane: the hub work runs from, and its compact list form.

use rune::{Cx, El, Tx, col, text};
use yd_core::address::thing::Thing;
use yd_core::address::uri::Address;
use yd_core::command::by_kind::{FINDING_ASSIGN, FINDING_DISMISS};
use yd_core::command::table::{Run, Target};
use yd_core::feed::{FeedEvent, Table};
use yd_core::findings::state::Stage;
use yd_core::findings::triage::{Act, TriageMsg};
use yd_core::findings::view::{Finding, Scope, Severity, findings, ready};
use yd_core::ids::{CommandId, FeedId, PaneId, TabId, ThreadId};
use yd_core::kind::Kind;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_core::row::{Row, Rows};
use yd_core::tokens::Tokens;
use yd_ui::chrome::tabs::{FILE, Tab, Tabs};
use yd_ui::kit::bars::{footer, header, lead};
use yd_ui::kit::chip::{Chip, chip};
use yd_ui::list::row;
use yd_ui::list::view::worded;
use yd_ui::menu::command_box;
use yd_ui::overlay::pick::Finder;
use yd_ui::pane::{PaneCx, PaneDef, TabAsk};
use yd_ui::theme::look::{LABEL, META, colour};
use yd_ui::win::{Hub, Win, dispatch};

use crate::findings::desc::{
    ACCEPT, ACCEPT_ALL, ALLOW_TEST, ASK_PROOF, COMMANDS, DESC, FIX_NOW, HUB, MERGE, NOT_A_PROBLEM,
    severity_badge,
};

use super::states::WORDS;
use crate::copy::empty::threadless;

pub mod fold;
mod scenes;
#[cfg(test)]
mod tests;

/// Rows a scene or a source may send; while it holds none, the rows are the model's findings.
pub const FEED: FeedId = FeedId::new("findings");
/// The standing checks, one row each: `text`, `note`, and `on` as a flag.
pub const CHECKS: FeedId = FeedId::new("findings-checks");
/// The rule the compact list ends with (record 5.6).
const SOURCES: &str = "From review, tests, logs, the desktop and your own comments";
const CLOSED: TabId = TabId(1);

pub const DEF: PaneDef = PaneDef {
    kind: PaneKind::Findings,
    view,
    scenes: scenes::SCENES,
    commands: COMMANDS,
    settings: &[],
    tabs: Some(tabs),
    on_tab: Some(on_tab),
    finder: Some(Finder::Keyed(thing_at)),
};

/// The compact list beside the work, or the hub: the page of record 6.6.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Form {
    #[default]
    List,
    Hub,
}

#[derive(Debug, Clone, Default)]
pub struct View {
    pub form: Form,
    /// Every thread of the shown thread's folder, not the shown thread alone.
    pub project: bool,
    /// The place By place picked: only its findings show.
    pub place: Option<Address>,
    /// The Closed tab is in front.
    pub closed: bool,
    /// What the open finding's last pressed button came to, when nothing carried it.
    pub said: Option<String>,
}

/// A finding and the row drawn for it.
#[derive(Debug, Clone)]
pub struct Item {
    pub row: Row,
    pub finding: Finding,
}

fn shown_thread(hub: &Hub, pane: PaneId) -> ThreadId {
    let thread = yd_ui::shown::of_pane(hub, pane);
    thread.map_or_else(|| ThreadId::new(""), |thread| thread.id.clone())
}

/// Every finding the pane can show: the feed's rows, else the model's view of the thread
/// (or its project), each with its row.
pub fn gather(hub: &Hub, pane: PaneId) -> Vec<Item> {
    let thread = shown_thread(hub, pane);
    let view = hub.panes.get::<View>(Some(pane));
    if let Some(fed) = hub.model.tables.get(FEED).filter(|table| !table.is_empty()) {
        let rows = (0..fed.len()).filter_map(|at| fed.get(at));
        let item = |row: &Row| {
            let finding = fold::finding_of(row, &thread)?;
            Some(Item {
                row: row.clone(),
                finding,
            })
        };
        return rows.filter_map(item).collect();
    }
    let scope = match view.is_some_and(|view| view.project) {
        true => Scope::Project,
        false => Scope::Thread,
    };
    let found = findings(&hub.model, &thread, scope);
    let gate = ready(&found);
    let item = |finding: Finding| Item {
        row: fold::row_of(&finding, &gate),
        finding,
    };
    found.into_iter().map(item).collect()
}

/// The findings the list draws: the open ones or the closed ones, in the place picked.
pub fn listed<'a>(items: &'a [Item], view: Option<&View>) -> Vec<&'a Item> {
    let closed = view.is_some_and(|view| view.closed);
    let place = view.and_then(|view| view.place.as_ref());
    let keep = |item: &&Item| {
        (item.finding.stage == Stage::Done) == closed
            && place.is_none_or(|place| &item.finding.place == place)
    };
    items.iter().filter(keep).collect()
}

/// Whether the header's "N open" counts it: anything not done, a new suggestion included, as
/// the mockup counts. The gate still waits only on filed findings ([`Stage::open`]).
fn counted(item: &&&Item) -> bool {
    item.finding.stage != Stage::Done
}

/// How many open findings each severity has. Invariant: the chips count what the header's
/// "N open" counts.
pub fn severities(items: &[&Item]) -> Vec<String> {
    let of = |severity: Severity| {
        let open = items.iter().filter(counted);
        let n = open
            .filter(|item| item.finding.severity == severity)
            .count();
        format!("{} {n}", severity_badge(severity).0)
    };
    [Severity::High, Severity::Med, Severity::Low]
        .into_iter()
        .map(of)
        .collect()
}

fn stage_count(items: &[&Item], stages: &[Stage]) -> usize {
    let hit = |item: &&&Item| stages.contains(&item.finding.stage);
    items.iter().filter(hit).count()
}

pub fn open_count(items: &[&Item]) -> usize {
    items.iter().filter(counted).count()
}

fn thing_at(hub: &Hub, pane: PaneId, key: &str) -> Option<Thing> {
    let items = gather(hub, pane);
    if let Some((_, rest)) = key.split_once(".evidence.") {
        let (id, n) = rest.rsplit_once('.')?;
        let item = items.iter().find(|item| item.row.id.as_str() == id)?;
        return item.finding.evidence.get(n.parse::<usize>().ok()?).cloned();
    }
    let id = row::id_of(key)?;
    let item = items.into_iter().find(|item| item.row.id == id)?;
    let mut thing = row::thing(&DESC, &item.row).ok()?;
    thing.label = item.finding.title;
    Some(thing)
}

/// What a command comes to for the finding it is aimed at.
fn act_of(command: CommandId) -> Option<Act> {
    match command {
        ACCEPT => Some(Act::Accept),
        FINDING_ASSIGN | FIX_NOW => Some(Act::Assign { agent: None }),
        ASK_PROOF => Some(Act::AskProof),
        ALLOW_TEST => Some(Act::AllowTest),
        NOT_A_PROBLEM | FINDING_DISMISS => Some(Act::Dismiss {
            reason: "It is not a problem here.".to_owned(),
        }),
        _ => None,
    }
}

/// The triage a command aimed at `target` asks for, read from what the window shows.
fn triage(hub: &Hub, win: &Win, command: CommandId, target: &Target) -> Vec<TriageMsg> {
    let thread = yd_ui::shown::thread(hub, win.window);
    let thread = thread.map_or_else(|| ThreadId::new(""), |thread| thread.id.clone());
    let all = findings(&hub.model, &thread, Scope::Project);
    let acts = |finding: &Finding, act: Act| TriageMsg::Act {
        thread: thread.clone(),
        finding: finding.id.clone(),
        act,
    };
    if command == ACCEPT_ALL {
        let new = all.iter().filter(|finding| finding.stage == Stage::New);
        return new.map(|finding| acts(finding, Act::Accept)).collect();
    }
    let Target::Thing(address) = target else {
        return Vec::new();
    };
    if address.kind() != Kind::Finding {
        return Vec::new();
    }
    let Some(finding) = all.iter().find(|f| f.id.as_str() == address.id()) else {
        return Vec::new();
    };
    if command == MERGE {
        let same = |other: &&Finding| other.stage.filed() && other.place == finding.place;
        let into = all.iter().find(same);
        return into
            .map(|into| TriageMsg::Merge {
                finding: finding.id.clone(),
                into: into.id.clone(),
            })
            .into_iter()
            .collect();
    }
    act_of(command)
        .map(|act| acts(finding, act))
        .into_iter()
        .collect()
}

/// Puts `pane` in `form`. Going back to the list drops what only the hub can undo: the
/// Closed tab and the place picked.
pub fn show(win: &mut Win, pane: PaneId, form: Form) {
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(view) = hub.panes.get_mut::<View>(Some(pane))
    {
        *view = View {
            form,
            project: form == Form::Hub || view.project,
            ..View::default()
        };
    }
}

/// Carries out the triage commands on a finding of the thread this window shows, Accept all,
/// and opening the hub; false for any other command or a finding the model does not hold.
pub fn carry(win: &mut Win, tx: &mut Tx, command: CommandId, target: &Target) -> bool {
    if command == HUB {
        let pane = match target {
            Target::Pane(pane) => Some(*pane),
            Target::App | Target::Thing(_) => {
                yd_ui::grid_view::view::show_pane(win, tx, PaneKind::Findings)
            }
        };
        pane.inspect(|pane| show(win, *pane, Form::Hub));
        return pane.is_some();
    }
    let asks = match win.hub.try_borrow() {
        Ok(hub) => triage(&hub, win, command, target),
        Err(_) => return false,
    };
    let carried = !asks.is_empty();
    for ask in asks {
        dispatch(win, Msg::Triage(ask));
    }
    carried
}

fn tabs(pane: &PaneCx<'_>) -> Tabs {
    let items = gather(pane.hub, pane.pane);
    let view = pane.state::<View>();
    let done = items
        .iter()
        .filter(|item| item.finding.stage == Stage::Done);
    let done = u32::try_from(done.count()).unwrap_or(u32::MAX);
    let shown = u32::try_from(items.len())
        .unwrap_or(u32::MAX)
        .saturating_sub(done);
    let meta = "updated now".to_owned();
    let mut tabs = vec![Tab {
        count: (shown > 0).then_some(shown),
        meta: meta.clone(),
        fixed: true,
        ..Tab::of(PaneKind::Findings)
    }];
    if view.is_some_and(|view| view.form == Form::Hub) {
        tabs.push(Tab {
            id: CLOSED,
            icon: &FILE,
            title: "Closed".to_owned(),
            count: (done > 0).then_some(done),
            meta,
            fixed: true,
            preview: false,
        });
    }
    let closed = view.is_some_and(|view| view.closed);
    Tabs {
        tabs,
        active: closed.then_some(CLOSED),
        adds: false,
        tool: None,
    }
}

fn on_tab(win: &mut Win, pane: PaneId, ask: TabAsk) {
    let TabAsk::Show(tab) = ask else {
        return;
    };
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(view) = hub.panes.get_mut::<View>(Some(pane))
    {
        view.closed = tab == CLOSED;
    }
}

/// The scope chip: this thread or this project; a press changes which.
pub fn scope_chip(pane: &PaneCx<'_>, key: &str) -> El {
    let project = pane.state::<View>().is_some_and(|view| view.project);
    let words = if project {
        "This project"
    } else {
        "This thread"
    };
    let at = pane.pane;
    chip(pane.tokens(), Chip::Plain, words, None, true)
        .key(format!("{key}.scope"))
        .on_press(move |win: &mut Win| {
            if let Ok(mut hub) = win.hub.try_borrow_mut()
                && let Some(view) = hub.panes.get_mut::<View>(Some(at))
            {
                view.project = !view.project;
            }
        })
}

pub fn chips(tokens: &Tokens, items: &[&Item]) -> Vec<El> {
    let each = |said: String| chip(tokens, Chip::Plain, &said, None, false);
    severities(items).into_iter().map(each).collect()
}

/// Frame 6's list: the counts, the rows, the rule it ends with.
fn compact(pane: &PaneCx<'_>, key: &str, items: &[&Item]) -> El {
    let tokens = pane.tokens();
    let new = stage_count(items, &[Stage::New]);
    let fixing = stage_count(items, &[Stage::Fixing, Stage::Proving]);
    let said = format!("{new} new · {fixing} being fixed");
    let left = [lead(tokens, &format!("{} open", open_count(items)))]
        .into_iter()
        .chain(chips(tokens, items))
        .chain([text(said).font(LABEL).color(colour(tokens.dim))])
        .collect();
    let mut table = Table::default();
    table.apply(FeedEvent::Reset(
        items.iter().map(|item| item.row.clone()).collect(),
    ));
    let rule = text(SOURCES).font(META).color(colour(tokens.faint));
    let open = Run::new(HUB).on(Target::Pane(pane.pane));
    let hub = chip(tokens, Chip::Plain, "Hub", None, false)
        .key(format!("{key}.hub"))
        .help("Open the findings hub")
        .on_press_tx(move |win: &mut Win, tx: &mut Tx| {
            command_box::run(win, tx, open.clone());
        });
    col()
        .key(key.to_owned())
        .w_full()
        .h_full()
        .fill(colour(tokens.pane_tint.alpha(tokens.glass_pane)))
        .child(
            header(tokens, left, vec![scope_chip(pane, key), hub])
                .key(format!("{key}.pane-header")),
        )
        .child(match threadless(pane) {
            Some((status, words)) => yd_ui::states::status::view(pane, key, &status, words),
            None => worded(
                pane,
                PaneKind::Findings,
                "",
                &DESC,
                Some(&table),
                &WORDS,
                false,
            ),
        })
        .child(footer(tokens, vec![rule], Vec::new()).key(format!("{key}.footer")))
}

fn view(pane: &PaneCx<'_>, cx: &mut Cx<Win>) -> El {
    let key = pane.key(PaneKind::Findings);
    let items = gather(pane.hub, pane.pane);
    let view = pane.state::<View>();
    let shown = listed(&items, view);
    match view.map(|view| view.form).unwrap_or_default() {
        Form::List => compact(pane, &key, &shown),
        Form::Hub => crate::findings::hub::view(pane, cx, &key, &items, &shown),
    }
}
