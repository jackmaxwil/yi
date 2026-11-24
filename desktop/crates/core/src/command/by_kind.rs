//! The right-click items each kind of thing adds (record 4.10), the fixed groups around them,
//! and the kind-specific values their hints show.

use serde_json::Value;

use crate::address::uri::Address;
use crate::command::table::{Command, Route};
use crate::command::tools::{container, lsp, plugin};
use crate::feed::LSP;
use crate::grid::windows::Windows;
use crate::ids::{CommandId, PaneId};
use crate::kind::Kind;
use crate::pane::{Opens, PaneKind, Place};
use crate::reduce::step::commands as wire;
use crate::reduce::threads::Thread;

/// The record's line icons by name; the view draws each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Plus,
    Chat,
    Branch,
    Search,
    Reload,
    Target,
    Folder,
    Diff,
    Stop,
    Sliders,
    File,
    Term,
    Monitor,
    Edit,
    Check,
    Eye,
    Shield,
    Panel,
    Out,
    X,
}

/// What an item shows at its right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hint {
    None,
    /// Its command's shortcut, as a keycap.
    Key,
    /// Words of its own: a key that only works inside the menu, or a `›` for more.
    Said(&'static str),
    /// The turn of the shown thread that wrote the thing.
    Turn,
    /// A tick while the item's word on its pane opening by itself is the one in force.
    Chosen,
}

/// One row of the right-click menu.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Item {
    pub command: CommandId,
    /// What the menu says, in the record's words.
    pub label: &'static str,
    pub icon: Icon,
    pub hint: Hint,
}

const fn item(command: CommandId, label: &'static str, icon: Icon, hint: Hint) -> Item {
    Item {
        command,
        label,
        icon,
        hint,
    }
}

const fn plain(command: CommandId, label: &'static str, icon: Icon) -> Item {
    item(command, label, icon, Hint::None)
}

macro_rules! ids {
    ($($name:ident $id:literal),+ $(,)?) => { $(pub const $name: CommandId = CommandId($id);)+ };
}

ids! {
    ATTACH "thing.attach", ASK "thing.ask", HAND_OFF "thing.hand-off",
    FINDING "thing.finding", NEW_FINDING "finding.new", WATCH "thing.watch", OPEN_PANE "thing.open-pane",
    COPY_ADDRESS "thing.copy-address",
    REFERENCES "lines.references", WHICH_TURN "lines.which-turn", PUT_BACK "lines.put-back",
    BREAK_HERE "lines.break", REVEAL "file.reveal", FILE_CHANGES "file.changes",
    FILE_THREADS "file.threads", THREAD_FORK "thread.fork-end", THREAD_PAUSE "thread.pause",
    THREAD_TRACE "thread.trace", AGENT_PAUSE "agent.pause", AGENT_TRACE "agent.trace",
    AGENT_BUDGET "agent.budget", LOG_AROUND "log-line.around", LOG_TURN "log-line.turn",
    LOG_SOURCE "log-line.source", FINDING_ASSIGN "finding.assign",
    FINDING_SIMILAR "finding.similar", FINDING_DISMISS "finding.dismiss",
    FINDING_ACCEPT "finding.accept", FINDING_PROOF "finding.ask-proof",
    FINDING_ALLOW_TEST "finding.allow-test", FINDING_MERGE "finding.merge",
    FINDING_NOT_A_PROBLEM "finding.not-a-problem",
    PROBLEM_OPEN "problem.open", PROBLEM_FIX "problem.fix", SERVER_LOGS "lsp.logs",
    SPAN_LOGS "span.logs", SPAN_RERUN "span.rerun", SPAN_SLOW "span.slow",
    CONTAINER_LOGS "container.logs", CONTAINER_SHELL "container.shell",
    CONTAINER_RESTART "container.restart", VM_SCREEN "vm.screen", VM_SNAPSHOT "vm.snapshot",
    VM_RESTART "vm.restart", SERVICE_LOGS "service.logs", SERVICE_RESTART "service.restart",
    SERVICE_STOP "service.stop", APPROVAL_EXPLAIN "approval.explain",
    ANSWER_TRACE "answer.trace", ANSWER_REWIND "answer.rewind", ANSWER_COPY "answer.copy",
    MESSAGE_EDIT "message.edit", TAB_SPLIT_RIGHT "tab.split-right",
    TAB_SPLIT_DOWN "tab.split-down", TAB_PULL_OUT "tab.pull-out", FRAME_OPEN "stack-frame.open",
    FRAME_EXPLAIN "stack-frame.explain", VARIABLE_WATCH "variable.watch",
    VARIABLE_SET "variable.where-set", ELEMENT_DESIGN "element.design",
    ELEMENT_SOURCE "element.source", SCREEN_SHOT "screen.screenshot", SCREEN_MARK "screen.mark-up",
    SETTING_COPY "setting.copy-key", SETTING_RESET "setting.reset",
    PANE_ALWAYS "pane.always-here", PANE_NEVER "pane.never-alone",
    TAB_KEEP "tab.keep", TAB_KEEP_ACROSS "tab.keep-across", TAB_CLOSE_OTHERS "tab.close-others",
    PANE_TO_WORK "pane.to-work", PANE_TO_SIDE "pane.to-side", PANE_TO_SHELF "pane.to-shelf",
}

/// The place a bar's row moves its pane to. The chat's place is the chat's alone.
pub fn moves(command: CommandId) -> Option<Place> {
    match command {
        PANE_TO_WORK => Some(Place::Work),
        PANE_TO_SIDE => Some(Place::Side),
        PANE_TO_SHELF => Some(Place::Shelf),
        _ => None,
    }
}

/// What a right-click on a pane's bar offers, the same on the pane and on its tab: where it
/// goes, how long it stays, and the person's word on its kind opening by itself.
const BAR_ITEMS: [Item; 11] = [
    plain(PANE_TO_WORK, "Move to work", Icon::Panel),
    plain(PANE_TO_SIDE, "Move to side", Icon::Panel),
    plain(PANE_TO_SHELF, "Move to shelf", Icon::Panel),
    plain(TAB_SPLIT_RIGHT, "Split right", Icon::Panel),
    plain(TAB_SPLIT_DOWN, "Split down", Icon::Panel),
    plain(TAB_PULL_OUT, "Pull out as a window", Icon::Out),
    item(TAB_KEEP, "Keep", Icon::Shield, Hint::Chosen),
    item(
        TAB_KEEP_ACROSS,
        "Keep across sessions",
        Icon::Branch,
        Hint::Chosen,
    ),
    plain(TAB_CLOSE_OTHERS, "Close others", Icon::X),
    item(PANE_ALWAYS, "Always open here", Icon::Target, Hint::Chosen),
    item(PANE_NEVER, "Never open by itself", Icon::Stop, Hint::Chosen),
];

/// The pane `address` names, a pane's or its tab's (`<pane>/<tab>`).
pub fn pane_named(address: &Address) -> Option<PaneId> {
    let named = matches!(address.kind(), Kind::Pane | Kind::Tab);
    let pane = address.id().split('/').next().filter(|_| named)?;
    Some(PaneId(pane.parse().ok()?))
}

/// Whether the tick of a bar row is on for `pane`: it is kept, here or across sessions.
/// None for a row that is no keeping.
pub fn keeps(grid: &Windows, command: CommandId, pane: PaneId) -> Option<bool> {
    let across = match command {
        TAB_KEEP => false,
        TAB_KEEP_ACROSS => true,
        _ => return None,
    };
    Some(grid.kept(pane) == Some(across))
}

/// What `command` says of the pane `address` names, a pane's or its tab's: the pane, its kind
/// and the word, always in the place the pane is in now. None for a pane no word covers.
pub fn word(
    grid: &Windows,
    command: CommandId,
    address: &Address,
) -> Option<(PaneId, PaneKind, Opens)> {
    let pane = pane_named(address)?;
    let kind = grid.ruled(pane)?;
    let word = match command {
        PANE_NEVER => Opens::Never,
        PANE_ALWAYS => Opens::Always(grid.slot_place(pane)?),
        _ => return None,
    };
    Some((pane, kind, word))
}

/// The quick questions under the header, each asked of the thing with [`ASK`].
pub const ASKS: &[&str] = &["Explain", "Why is this here?", "Is it safe?"];

/// What every menu opens with: attach, ask, hand off.
pub const TOP: &[Item] = &[
    item(ATTACH, "Attach to chat", Icon::Plus, Hint::Said("⏎")),
    item(ASK, "Ask Yi about this…", Icon::Chat, Hint::Said("⌘K")),
    item(
        HAND_OFF,
        "Hand it to another agent",
        Icon::Branch,
        Hint::Said("›"),
    ),
];

/// The groups after the kind's own items, each under a separator.
pub const ENDS: &[&[Item]] = &[
    &[
        plain(FINDING, "Make it a finding", Icon::Shield),
        plain(WATCH, "Watch for this", Icon::Eye),
    ],
    &[
        item(
            OPEN_PANE,
            "Open in a new pane",
            Icon::Panel,
            Hint::Said("›"),
        ),
        item(COPY_ADDRESS, "Copy its address", Icon::File, Hint::Key),
    ],
];

/// The line under the items.
pub const FOOTER: &str = "Same menu on anything. Hold ⌥ for the system menu.";

const ALLOW_ITEMS: &[Item] = &[
    plain(wire::ALLOW, "Allow once", Icon::Check),
    plain(wire::ALWAYS, "Always allow this", Icon::Check),
    plain(APPROVAL_EXPLAIN, "What will it do?", Icon::Search),
];

/// The kind's own items, the middle group. A kind the record gives none has none.
pub fn items(kind: Kind) -> &'static [Item] {
    match kind {
        Kind::Lines => {
            const {
                &[
                    item(
                        REFERENCES,
                        "Find references",
                        Icon::Search,
                        Hint::Said("⌘ click"),
                    ),
                    item(
                        WHICH_TURN,
                        "Which turn wrote this",
                        Icon::Branch,
                        Hint::Turn,
                    ),
                    plain(PUT_BACK, "Put these lines back as they were", Icon::Reload),
                    plain(BREAK_HERE, "Stop here in the debugger", Icon::Target),
                ]
            }
        }
        Kind::File => {
            const {
                &[
                    plain(REVEAL, "Show in the file tree", Icon::Folder),
                    plain(FILE_CHANGES, "What this thread changed in it", Icon::Diff),
                    plain(FILE_THREADS, "Every thread that touched it", Icon::Branch),
                ]
            }
        }
        Kind::Thread => {
            const {
                &[
                    plain(THREAD_FORK, "Fork from here", Icon::Branch),
                    plain(THREAD_PAUSE, "Pause it", Icon::Stop),
                    plain(THREAD_TRACE, "Show its trace", Icon::Sliders),
                ]
            }
        }
        Kind::Agent => {
            const {
                &[
                    plain(AGENT_PAUSE, "Pause it", Icon::Stop),
                    plain(AGENT_TRACE, "Show its trace", Icon::Sliders),
                    plain(AGENT_BUDGET, "Give it more budget", Icon::Plus),
                ]
            }
        }
        Kind::LogLine => {
            const {
                &[
                    plain(LOG_AROUND, "Show logs around this moment", Icon::File),
                    plain(LOG_TURN, "Jump to the turn at this time", Icon::Branch),
                    plain(LOG_SOURCE, "Only this source", Icon::Search),
                ]
            }
        }
        Kind::Finding => {
            const {
                &[
                    plain(FINDING_ACCEPT, "Accept it", Icon::Check),
                    plain(FINDING_ASSIGN, "Give it to an agent", Icon::Branch),
                    plain(FINDING_PROOF, "Ask for proof", Icon::Shield),
                    plain(FINDING_ALLOW_TEST, "Allow its test", Icon::Check),
                    plain(FINDING_MERGE, "Merge into the same place", Icon::Diff),
                    plain(FINDING_SIMILAR, "Show ones like it", Icon::Search),
                    plain(FINDING_NOT_A_PROBLEM, "Not a problem", Icon::X),
                    plain(
                        FINDING_DISMISS,
                        "Not a problem, and stop raising it",
                        Icon::X,
                    ),
                ]
            }
        }
        Kind::Span => {
            const {
                &[
                    plain(SPAN_LOGS, "Logs for this step", Icon::File),
                    plain(SPAN_RERUN, "Run this step again", Icon::Reload),
                    plain(SPAN_SLOW, "Why was it slow?", Icon::Sliders),
                ]
            }
        }
        Kind::Container => {
            const {
                &[
                    plain(CONTAINER_LOGS, "Logs", Icon::File),
                    plain(CONTAINER_SHELL, "Shell", Icon::Term),
                    plain(CONTAINER_RESTART, "Restart", Icon::Reload),
                    plain(container::STOP, "Stop", Icon::Stop),
                    plain(container::START, "Start", Icon::Check),
                    plain(container::REMOVE, "Remove", Icon::X),
                ]
            }
        }
        Kind::Vm => {
            const {
                &[
                    plain(VM_SCREEN, "Open its screen", Icon::Monitor),
                    plain(VM_SNAPSHOT, "Snapshot", Icon::Plus),
                    plain(VM_RESTART, "Restart", Icon::Reload),
                ]
            }
        }
        Kind::Service => {
            const {
                &[
                    plain(SERVICE_LOGS, "Logs", Icon::File),
                    plain(SERVICE_RESTART, "Restart", Icon::Reload),
                    plain(SERVICE_STOP, "Stop", Icon::Stop),
                ]
            }
        }
        Kind::Screen => {
            const {
                &[
                    plain(SCREEN_SHOT, "Screenshot it", Icon::Monitor),
                    plain(SCREEN_MARK, "Mark it up", Icon::Edit),
                ]
            }
        }
        Kind::Command | Kind::Approval => ALLOW_ITEMS,
        Kind::Answer => {
            const {
                &[
                    plain(ANSWER_TRACE, "Show the trace of this answer", Icon::Sliders),
                    plain(ANSWER_REWIND, "Go back to before this answer", Icon::Reload),
                    plain(ANSWER_COPY, "Copy as text", Icon::File),
                ]
            }
        }
        Kind::Message => {
            const {
                &[
                    plain(MESSAGE_EDIT, "Edit and send again", Icon::Edit),
                    plain(wire::FORK, "Fork from here", Icon::Branch),
                ]
            }
        }
        Kind::Tab | Kind::Pane => &BAR_ITEMS,
        Kind::StackFrame => {
            const {
                &[
                    plain(FRAME_OPEN, "Open this line", Icon::File),
                    plain(FRAME_EXPLAIN, "Explain how we got here", Icon::Search),
                ]
            }
        }
        Kind::Variable => {
            const {
                &[
                    plain(VARIABLE_WATCH, "Watch it", Icon::Eye),
                    plain(VARIABLE_SET, "Where is it set?", Icon::Search),
                ]
            }
        }
        Kind::Element => {
            const {
                &[
                    plain(ELEMENT_DESIGN, "Design this", Icon::Sliders),
                    plain(ELEMENT_SOURCE, "Show the code that draws it", Icon::File),
                    plain(SCREEN_SHOT, "Screenshot it", Icon::Monitor),
                ]
            }
        }
        Kind::Setting => {
            const {
                &[
                    plain(SETTING_COPY, "Copy its key", Icon::File),
                    plain(SETTING_RESET, "Put the default back", Icon::Reload),
                ]
            }
        }
        Kind::Problem => {
            const {
                &[
                    plain(PROBLEM_OPEN, "Open the line", Icon::File),
                    plain(PROBLEM_FIX, "Quick fix", Icon::Edit),
                ]
            }
        }
        Kind::LanguageServer => {
            const {
                &[
                    plain(lsp::RESTART, "Restart it", Icon::Reload),
                    plain(SERVER_LOGS, "Show its log", Icon::File),
                ]
            }
        }
        // A stat or a row a plugin drew answers for its plugin.
        Kind::Plugin | Kind::Number => {
            const {
                &[
                    plain(plugin::OPEN_SOURCE, "Open its source", Icon::File),
                    plain(plugin::RELOAD, "Reload it", Icon::Reload),
                ]
            }
        }
        Kind::Comment
        | Kind::Step
        | Kind::Turn
        | Kind::Card
        | Kind::Page
        | Kind::Asset
        | Kind::Reference
        | Kind::Breakpoint
        | Kind::Notification
        | Kind::Theme
        | Kind::ThemeValue
        | Kind::Check
        | Kind::Place
        | Kind::StandingCheck
        | Kind::Property => &[],
    }
}

/// The turn of `thread` that last wrote the file at `path`: the turn under way when the
/// newest tool call naming the path started.
pub fn turn_of(thread: &Thread, path: &str) -> Option<u32> {
    let names = |value: &Value| match value {
        Value::String(named) => named == path || named.ends_with(&format!("/{path}")),
        _ => false,
    };
    let touched = thread
        .transcript
        .tools
        .values()
        .filter(|tool| match &tool.args {
            Value::Object(args) => args.values().any(names),
            other => names(other),
        });
    let at = touched.filter_map(|tool| tool.started).max()?;
    let turns = thread.transcript.turns();
    let under_way = turns.iter().rev().find(|turn| turn.started <= at)?;
    Some(under_way.number)
}

const fn aimed(id: CommandId, title: &'static str, kinds: &'static [Kind]) -> Command {
    Command {
        id,
        title,
        key: None,
        menu: None,
        kinds,
        route: Route::Model,
    }
}

/// The rows of every id the menu adds; the wire's allow, always and fork are the wire's own.
pub const COMMANDS: &[Command] = &[
    aimed(ATTACH, "Attach to Chat", &[]),
    aimed(ASK, "Ask Yi About This", &[]),
    aimed(HAND_OFF, "Hand It to Another Agent", &[]),
    aimed(FINDING, "Make It a Finding", &[]),
    aimed(NEW_FINDING, "New Finding", &[]),
    aimed(WATCH, "Watch for This", &[]),
    aimed(OPEN_PANE, "Open in a New Pane", &[]),
    Command {
        key: Some("cmd+shift+c"),
        ..aimed(COPY_ADDRESS, "Copy Its Address", &[])
    },
    aimed(REFERENCES, "Find References", &[Kind::Lines]),
    aimed(WHICH_TURN, "Which Turn Wrote This", &[Kind::Lines]),
    aimed(PUT_BACK, "Put These Lines Back", &[Kind::Lines]),
    aimed(BREAK_HERE, "Stop Here in the Debugger", &[Kind::Lines]),
    aimed(REVEAL, "Show in the File Tree", &[Kind::File]),
    aimed(FILE_CHANGES, "What This Thread Changed", &[Kind::File]),
    aimed(FILE_THREADS, "Every Thread That Touched It", &[Kind::File]),
    aimed(THREAD_FORK, "Fork Thread", &[Kind::Thread]),
    aimed(THREAD_PAUSE, "Pause Thread", &[Kind::Thread]),
    aimed(THREAD_TRACE, "Show Thread Trace", &[Kind::Thread]),
    aimed(AGENT_PAUSE, "Pause Agent", &[Kind::Agent]),
    aimed(AGENT_TRACE, "Show Agent Trace", &[Kind::Agent]),
    aimed(AGENT_BUDGET, "Give Agent More Budget", &[Kind::Agent]),
    aimed(LOG_AROUND, "Show Logs Around This Moment", &[Kind::LogLine]),
    aimed(LOG_TURN, "Jump to the Turn at This Time", &[Kind::LogLine]),
    aimed(LOG_SOURCE, "Only This Source", &[Kind::LogLine]),
    aimed(FINDING_ASSIGN, "Give Finding to an Agent", &[Kind::Finding]),
    aimed(FINDING_SIMILAR, "Show Similar Findings", &[Kind::Finding]),
    aimed(FINDING_DISMISS, "Dismiss Finding", &[Kind::Finding]),
    aimed(FINDING_ACCEPT, "Accept Finding", &[Kind::Finding]),
    aimed(FINDING_PROOF, "Ask for Proof", &[Kind::Finding]),
    aimed(FINDING_ALLOW_TEST, "Allow Its Test", &[Kind::Finding]),
    aimed(FINDING_MERGE, "Merge Into the Same Place", &[Kind::Finding]),
    aimed(FINDING_NOT_A_PROBLEM, "Not a Problem", &[Kind::Finding]),
    aimed(PROBLEM_OPEN, "Open the Problem's Line", &[Kind::Problem]),
    aimed(PROBLEM_FIX, "Quick Fix", &[Kind::Problem]),
    Command {
        route: Route::Source(LSP),
        ..aimed(
            lsp::RESTART,
            "Restart Language Server",
            &[Kind::LanguageServer],
        )
    },
    aimed(
        SERVER_LOGS,
        "Show Language Server Logs",
        &[Kind::LanguageServer],
    ),
    aimed(SPAN_LOGS, "Logs for This Step", &[Kind::Span]),
    aimed(SPAN_RERUN, "Run This Step Again", &[Kind::Span]),
    aimed(SPAN_SLOW, "Why Was It Slow", &[Kind::Span]),
    aimed(CONTAINER_LOGS, "Container Logs", &[Kind::Container]),
    aimed(CONTAINER_SHELL, "Container Shell", &[Kind::Container]),
    aimed(CONTAINER_RESTART, "Restart Container", &[Kind::Container]),
    aimed(container::STOP, "Stop Container", &[Kind::Container]),
    aimed(container::START, "Start Container", &[Kind::Container]),
    aimed(container::REMOVE, "Remove Container", &[Kind::Container]),
    aimed(
        plugin::OPEN_SOURCE,
        "Open Plugin Source",
        &[Kind::Plugin, Kind::Number],
    ),
    aimed(
        plugin::RELOAD,
        "Reload Plugin",
        &[Kind::Plugin, Kind::Number],
    ),
    aimed(VM_SCREEN, "Open VM Screen", &[Kind::Vm]),
    aimed(VM_SNAPSHOT, "Snapshot VM", &[Kind::Vm]),
    aimed(VM_RESTART, "Restart VM", &[Kind::Vm]),
    aimed(SERVICE_LOGS, "Service Logs", &[Kind::Service]),
    aimed(SERVICE_RESTART, "Restart Service", &[Kind::Service]),
    aimed(SERVICE_STOP, "Stop Service", &[Kind::Service]),
    aimed(
        APPROVAL_EXPLAIN,
        "What Will It Do",
        &[Kind::Approval, Kind::Command],
    ),
    aimed(ANSWER_TRACE, "Show Answer Trace", &[Kind::Answer]),
    aimed(
        ANSWER_REWIND,
        "Go Back to Before This Answer",
        &[Kind::Answer],
    ),
    aimed(ANSWER_COPY, "Copy Answer as Text", &[Kind::Answer]),
    aimed(MESSAGE_EDIT, "Edit and Send Again", &[Kind::Message]),
    aimed(SETTING_COPY, "Copy Setting Key", &[Kind::Setting]),
    aimed(SETTING_RESET, "Reset Setting", &[Kind::Setting]),
    aimed(TAB_SPLIT_RIGHT, "Split Right", &[Kind::Pane, Kind::Tab]),
    aimed(TAB_SPLIT_DOWN, "Split Down", &[Kind::Pane, Kind::Tab]),
    aimed(
        TAB_PULL_OUT,
        "Pull Out as a Window",
        &[Kind::Pane, Kind::Tab],
    ),
    aimed(PANE_TO_WORK, "Move Pane to Work", &[Kind::Pane, Kind::Tab]),
    aimed(PANE_TO_SIDE, "Move Pane to Side", &[Kind::Pane, Kind::Tab]),
    aimed(
        PANE_TO_SHELF,
        "Move Pane to Shelf",
        &[Kind::Pane, Kind::Tab],
    ),
    aimed(TAB_KEEP, "Keep Tab", &[Kind::Pane, Kind::Tab]),
    aimed(
        TAB_KEEP_ACROSS,
        "Keep Tab Across Sessions",
        &[Kind::Pane, Kind::Tab],
    ),
    aimed(
        TAB_CLOSE_OTHERS,
        "Close Other Tabs",
        &[Kind::Pane, Kind::Tab],
    ),
    aimed(
        PANE_ALWAYS,
        "Always Open Pane Here",
        &[Kind::Pane, Kind::Tab],
    ),
    aimed(
        PANE_NEVER,
        "Never Open Pane by Itself",
        &[Kind::Pane, Kind::Tab],
    ),
    aimed(FRAME_OPEN, "Open This Line", &[Kind::StackFrame]),
    aimed(
        FRAME_EXPLAIN,
        "Explain How We Got Here",
        &[Kind::StackFrame],
    ),
    aimed(VARIABLE_WATCH, "Watch Variable", &[Kind::Variable]),
    aimed(VARIABLE_SET, "Where Is It Set", &[Kind::Variable]),
    aimed(ELEMENT_DESIGN, "Design This", &[Kind::Element]),
    aimed(
        ELEMENT_SOURCE,
        "Show the Code That Draws It",
        &[Kind::Element],
    ),
    aimed(SCREEN_SHOT, "Screenshot It", &[Kind::Screen, Kind::Element]),
    aimed(SCREEN_MARK, "Mark It Up", &[Kind::Screen]),
];

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;
    use crate::command::table::{Commands, declared};

    #[test]
    fn every_item_the_menu_shows_has_a_command_row() {
        let table = Commands::new(declared());
        let fixed = TOP.iter().chain(ENDS.iter().copied().flatten());
        let middle = Kind::ALL.iter().flat_map(|kind| items(*kind));
        let missing: Vec<&str> = fixed
            .chain(middle)
            .filter(|item| table.find(item.command).is_none())
            .map(|item| item.label)
            .collect();
        assert_eq!(missing, Vec::<&str>::new());
    }

    /// The mockup's own table of the lines menu (`data/context_menu.json`), item by item.
    #[test]
    fn the_lines_menu_reads_as_the_mockup_draws_it() -> Result<(), Box<dyn Error>> {
        let drawn: Value = serde_json::from_str(include_str!(
            "../../../../fixtures/mockup/data/context_menu.json"
        ))?;
        let rows = drawn
            .get("items")
            .and_then(Value::as_array)
            .ok_or("no items")?;
        let said: Vec<&str> = rows
            .iter()
            .filter(|row| row.get("type").and_then(Value::as_str) == Some("item"))
            .filter_map(|row| row.get("text").and_then(Value::as_str))
            .collect();
        let ends = ENDS.iter().copied().flatten();
        let ours: Vec<&str> = TOP
            .iter()
            .chain(items(Kind::Lines))
            .chain(ends)
            .map(|item| item.label)
            .collect();
        assert_eq!(ours, said);
        let chips = drawn
            .get("chips")
            .and_then(Value::as_array)
            .ok_or("no chips")?;
        let chips: Vec<&str> = chips.iter().filter_map(Value::as_str).collect();
        assert_eq!(chips, ASKS);
        let foot = rows
            .last()
            .and_then(|row| row.get("text"))
            .and_then(Value::as_str);
        assert_eq!(foot, Some(FOOTER));
        Ok(())
    }

    #[test]
    fn a_kind_with_no_items_of_its_own_has_an_empty_middle() {
        assert!(items(Kind::Comment).is_empty());
        assert_eq!(items(Kind::Approval), items(Kind::Command));
    }
}
