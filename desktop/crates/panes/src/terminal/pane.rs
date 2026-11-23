//! Terminal pane: shell tabs, and a read-only tab of the agent's commands.

use rune::{Cx, El, ScrollSpec, Tx, col, row, text};
use yd_core::feed::{FeedEvent, FeedStatus, SourceCmd, Table};
use yd_core::ids::{FeedId, PaneId, RowId, TabId, WindowId};
use yd_core::model::Model;
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::reduce::threads::Thread;
use yd_core::reduce::turns::{ToolCall, ToolState};
use yd_core::row::Row;
use yd_core::tokens::Tokens;
use yd_ui::chrome::tabs::{self, Tabs};
use yd_ui::pane::{PaneCx, PaneDef, SCENE_PANE, Scene, Stage, TabAsk};
use yd_ui::states::status::{Status, bar, view as state_view};
use yd_ui::term::view::state::{FEED, Shell, Shells, Tab};
use yd_ui::term::view::{send, surface};
use yd_ui::theme::look::{CODE_LINE, colour};
use yd_ui::theme::scale::TERMINAL_BACKGROUND;
use yd_ui::win::{Hub, Win, dispatch};

use super::states::WORDS;

pub const DEF: PaneDef = PaneDef {
    kind: PaneKind::Terminal,
    view,
    scenes: &[
        Scene {
            name: "terminal/mockup",
            stage: Stage::Pane(PaneKind::Terminal),
            setup: mockup,
        },
        Scene {
            name: "terminal/agent-tab",
            stage: Stage::Pane(PaneKind::Terminal),
            setup: agent_tab,
        },
        Scene {
            name: "terminal/missing",
            stage: Stage::Pane(PaneKind::Terminal),
            setup: missing,
        },
    ],
    commands: &[],
    settings: &[],
    tabs: Some(tabs),
    on_tab: Some(on_tab),
    finder: None,
};

/// The agent's tab. Invariant: shells are numbered from 1, so 0 is never a shell's.
const AGENT: TabId = TabId(0);

/// The pane's tabs for its bar: one a shell, then the agent's commands, and a `+`.
fn tabs(pane: &PaneCx<'_>) -> Tabs {
    let Some(shells) = pane.state::<Shells>() else {
        return Tabs::default();
    };
    let agent = agent(pane.thread());
    let thread = agent.as_ref().map(|(thread, _)| *thread);
    let mut tabs: Vec<tabs::Tab> = shells
        .tabs
        .iter()
        .map(|shell| tabs::Tab {
            id: TabId(shell.feed.instance),
            title: shell.name().to_owned(),
            meta: shell_meta(pane.model(), shell.feed, &shell.cwd, thread),
            ..tabs::Tab::of(PaneKind::Terminal)
        })
        .collect();
    if let Some((thread, calls)) = &agent {
        let said = calls.last();
        let title = said.map(|call| format!("agent: {} · {}", first_line(call), state(call)));
        tabs.push(tabs::Tab {
            id: AGENT,
            title: title.unwrap_or_default(),
            meta: thread.cwd.clone(),
            ..tabs::Tab::of(PaneKind::Terminal)
        });
    }
    let active = match shells.active {
        Tab::Shell(feed) => TabId(feed.instance),
        Tab::Agent => AGENT,
    };
    Tabs {
        tabs,
        active: Some(active),
        adds: true,
        tool: None,
    }
}

/// What the bar asks: a tab in front, a shell closed, one more shell.
fn on_tab(win: &mut Win, pane: PaneId, ask: TabAsk) {
    let shell = |tab: TabId| FEED.instance(tab.0);
    match ask {
        TabAsk::Show(AGENT) => show(win, pane, Tab::Agent),
        TabAsk::Show(tab) => show(win, pane, Tab::Shell(shell(tab))),
        // The agent's tab lists what it ran: there is nothing to hang up.
        TabAsk::Close(AGENT) => {}
        TabAsk::Close(tab) => close(win, pane, shell(tab)),
        TabAsk::New => open(win, pane),
    }
}

fn view(pane: &PaneCx<'_>, _cx: &mut Cx<Win>) -> El {
    let key = pane.key(PaneKind::Terminal);
    let tokens = pane.tokens();
    let id = pane.pane;
    let root = col()
        .key(key.clone())
        .w_full()
        .h_full()
        .min_h(0.0)
        .on_layout_tx(move |win: &mut Win, _: [f32; 4], _: &mut Tx| made(win, id));
    let Some(shells) = pane.state::<Shells>() else {
        return root;
    };
    let body = match shells.active {
        Tab::Shell(feed) => shells
            .shell(feed)
            .map(|shell| shell_body(pane, &key, shell)),
        Tab::Agent => agent(pane.thread()).map(|(_, calls)| agent_body(&key, tokens, &calls)),
    };
    root.fill(colour(tokens.pane_tint.alpha(tokens.glass_pane)))
        .children(body)
}

/// A shell's screen and its state. Invariant: the screen is laid out while the shell starts,
/// since its first size is what starts it; the state lies over it until it speaks.
fn shell_body(pane: &PaneCx<'_>, key: &str, shell: &Shell) -> El {
    // Incident: the screen sat one level deeper while the shell started than once it spoke, so
    // when it did the keyboard lost its place and went to the pane's close button.
    let held = |over: Option<Status>| {
        let over = over.map(|status| {
            let over = state_view(pane, key, &status, &WORDS);
            over.absolute(0.0, 0.0).w_full().h_full()
        });
        col()
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .child(surface(pane, shell))
            .children(over)
    };
    match pane.model().tables.get(shell.feed).map(Table::status) {
        None | Some(FeedStatus::Loading) => held(Some(Status::Loading)),
        Some(FeedStatus::Live | FeedStatus::Empty) if blank(shell) => held(Some(Status::Empty)),
        Some(FeedStatus::Live | FeedStatus::Empty) => held(None),
        Some(FeedStatus::Offline) => col()
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .child(bar(pane, key, &Status::Offline(None), &WORDS))
            .child(surface(pane, shell)),
        Some(FeedStatus::Error(why)) => state_view(pane, key, &Status::Error(why.clone()), &WORDS),
    }
}

/// Whether the shell has printed nothing: no line scrolled off and a blank screen. A screen
/// the source is writing counts as printed.
fn blank(shell: &Shell) -> bool {
    let surface = &shell.surface;
    let scrolled = surface
        .lines
        .try_lock()
        .map_or(true, |lines| !lines.is_empty());
    let shown = surface
        .parser
        .try_lock()
        .map_or(true, |parser| !parser.screen().contents().trim().is_empty());
    !scrolled && !shown
}

/// Whether a terminal has something to show in `window`: a command its thread's agent ran,
/// or a shell of one of its Terminal panes that has printed.
pub(crate) fn has(hub: &Hub, window: WindowId) -> bool {
    if agent(yd_ui::shown::thread(hub, window)).is_some() {
        return true;
    }
    let grid = &hub.model.grid;
    let here = |pane: PaneId| {
        let layout = grid.window(window).map(|window| &window.layout);
        layout.is_some_and(|layout| layout.slot(pane).is_some())
    };
    let terminals = grid
        .panes
        .iter()
        .filter(|pane| pane.kind == PaneKind::Terminal);
    let mut shells = terminals
        .filter(|pane| here(pane.id))
        .filter_map(|pane| hub.panes.get::<Shells>(Some(pane.id)))
        .flat_map(|shells| &shells.tabs);
    shells.any(|shell| !blank(shell))
}

/// `<cwd> · exit N`: the folder is the shell's own, or its thread's while it has none.
fn shell_meta(model: &Model, feed: FeedId, cwd: &str, thread: Option<&Thread>) -> String {
    let cwd = if cwd.is_empty() {
        thread.map_or("", |thread| thread.cwd.as_str())
    } else {
        cwd
    };
    let exit = model
        .tables
        .get(feed)
        .and_then(|t| t.find(&RowId::new("shell")))
        .and_then(|r| r.words("exit"));
    match exit {
        Some(code) => format!("{cwd} · exit {code}"),
        None => cwd.to_owned(),
    }
}

/// The shown thread when its agent ran shell commands, and those commands in the order
/// they started.
fn agent(thread: Option<&Thread>) -> Option<(&Thread, Vec<&ToolCall>)> {
    let thread = thread?;
    // A command a child agent runs is the thread's too: the ask for it shows in its chat.
    let children = thread.agents.iter().map(|agent| &agent.transcript);
    let all = std::iter::once(&thread.transcript).chain(children);
    let tools = all.flat_map(|transcript| transcript.tools.values());
    let mut calls: Vec<&ToolCall> = tools.filter(|call| call.name == "bash").collect();
    calls.sort_by_key(|call| (call.started, call.id.clone()));
    (!calls.is_empty()).then_some((thread, calls))
}

fn command(call: &ToolCall) -> &str {
    call.args
        .get("command")
        .and_then(|c| c.as_str())
        .unwrap_or(&call.name)
}

/// A tab is one line: the command's first, cut at 60 characters, with `…` where it was cut.
fn first_line(call: &ToolCall) -> String {
    let said = command(call).trim();
    let first = said.lines().next().unwrap_or_default();
    let mut kept: String = first.chars().take(60).collect();
    if kept.len() < said.len() {
        kept.push('…');
    }
    kept
}

fn state(call: &ToolCall) -> String {
    match call.state {
        ToolState::Queued => "queued".to_owned(),
        ToolState::Asking => "waiting".to_owned(),
        ToolState::Lost => "not answered".to_owned(),
        ToolState::Running => "running".to_owned(),
        ToolState::Done => match call.details.get("exitCode").and_then(|c| c.as_i64()) {
            Some(code) => format!("exit {code}"),
            None => "done".to_owned(),
        },
        ToolState::Failed => "failed".to_owned(),
    }
}

/// The most lines of one command's output the agent's tab draws, the last ones.
const AGENT_LINES: usize = 40;

/// Each command as the shell would echo it, what it printed, and where it stands under it.
fn agent_body(key: &str, tokens: &Tokens, calls: &[&ToolCall]) -> El {
    let lines = calls.iter().map(|call| {
        let ms = call
            .duration()
            .map(|d| format!(" · {} ms", d.0))
            .unwrap_or_default();
        let printed = crate::chat::tools::output(call);
        let all = printed.lines().count();
        let kept = printed.lines().skip(all.saturating_sub(AGENT_LINES));
        let cut = (all > AGENT_LINES).then(|| {
            let said = format!(
                "[…] last {AGENT_LINES} of {all} lines shown (the agent tab's cap): the step in the chat holds the whole result"
            );
            text(said).font(CODE_LINE).color(colour(tokens.faint)).wrap()
        });
        let shown = kept.map(|line| {
            text(line.to_owned())
                .font(CODE_LINE)
                .color(colour(tokens.fg))
                .wrap()
        });
        col()
            .key(format!("{key}.agent.{}", call.id.as_str()))
            .child(
                row()
                    .child(text("$ ").font(CODE_LINE).color(colour(tokens.accent)))
                    .child(
                        text(command(call).to_owned())
                            .font(CODE_LINE)
                            .color(colour(tokens.dim)),
                    ),
            )
            .children(cut)
            .children(shown)
            .child(
                text(format!("{}{ms}", state(call)))
                    .font(CODE_LINE)
                    .color(colour(tokens.faint)),
            )
    });
    col()
        .w_full()
        .h_full()
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: true })
        .fill(colour(TERMINAL_BACKGROUND.of(tokens)))
        .px(8.0)
        .py(4.0)
        .children(lines)
}

/// The pane's state is made on its first layout, so the first shell exists to be started.
fn made(win: &mut Win, pane: PaneId) {
    if let Ok(mut hub) = win.hub.try_borrow_mut() {
        hub.panes.get_mut::<Shells>(Some(pane));
    }
}

fn show(win: &mut Win, pane: PaneId, tab: Tab) {
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(shells) = hub.panes.get_mut::<Shells>(Some(pane))
    {
        shells.active = tab;
    }
}

fn open(win: &mut Win, pane: PaneId) {
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(shells) = hub.panes.get_mut::<Shells>(Some(pane))
    {
        shells.open();
    }
}

/// Shows this window's Terminal pane, opens a shell tab in it and types `line` there: another
/// pane's way to a shell aimed at its thing. False when the grid has no room for the pane.
pub fn type_in(win: &mut Win, tx: &mut Tx, line: &str) -> bool {
    let Some(pane) = yd_ui::grid_view::view::show_pane(win, tx, PaneKind::Terminal) else {
        return false;
    };
    let opened = win.hub.try_borrow_mut().ok().and_then(|mut hub| {
        let shells = hub.panes.get_mut::<Shells>(Some(pane))?;
        Some(shells.open())
    });
    let Some(feed) = opened else {
        return false;
    };
    let typed = SourceCmd::Input(line.as_bytes().to_vec());
    dispatch(win, Msg::Source(feed, typed));
    true
}

/// Closes a shell tab and tells its source to hang the shell up.
fn close(win: &mut Win, pane: PaneId, feed: FeedId) {
    let closed = win.hub.try_borrow_mut().ok().and_then(|mut hub| {
        hub.panes
            .get_mut::<Shells>(Some(pane))
            .map(|s| s.close(feed))
    });
    if closed == Some(true) {
        send(win, feed, SourceCmd::Close);
    }
}

/// The record's sample (4.4), verbatim: project in the accent, branch in green.
const SAMPLE: &str = "\x1b[34matlas-api\x1b[0m \x1b[32mrate-limit-uploads\x1b[0m $ git status --short\r\n M src/routes/upload.rs\r\n?? src/middleware/rate_limit.rs\r\n\x1b[34matlas-api\x1b[0m \x1b[32mrate-limit-uploads\x1b[0m $ ";
const ASK: &str = include_str!("../../../../fixtures/wire/approval-allow.jsonl");

/// The recorded bash ask up to the point it waits, with the mockup's command in it; only for
/// a hub with no thread yet, so a window's scene keeps the thread it already shows.
fn waiting_ask(hub: &mut Hub) {
    if !hub.model.wire.threads.is_empty() {
        return;
    }
    let upto: Vec<&str> = ASK
        .lines()
        .take_while(|l| !l.contains("\"outcome\""))
        .collect();
    let recording = upto.join("\n").replace("echo one > one.txt", "cargo test");
    let _ = yd_core::wire::replay::play(&mut hub.model, &recording);
}

/// Frame 14: a zsh in `~/atlas-api` after `git status`, and the agent's `cargo test` waiting.
fn mockup(hub: &mut Hub) {
    waiting_ask(hub);
    let Some(shells) = hub.panes.get_mut::<Shells>(Some(SCENE_PANE)) else {
        return;
    };
    let Some(shell) = shells.tabs.first_mut() else {
        return;
    };
    shell.program = "/bin/zsh".to_owned();
    shell.cwd = "~/atlas-api".to_owned();
    let feed = shell.feed;
    if let Ok(mut parser) = shell.surface.parser.lock() {
        parser.process(SAMPLE.as_bytes());
    }
    let exit = Row::new("shell").text("exit", "0");
    step(&mut hub.model, Msg::Feed(feed, FeedEvent::Upsert(exit)));
}

/// What the shell source says when no `yi` is installed, under the bare `PATH` of an app
/// started from the Dock.
const NO_YI: &str = "yi is not installed: not on PATH (/usr/bin:/bin:/usr/sbin:/sbin) and not at /Users/you/.local/bin/yi";

/// A shell that could not start because `yi` is nowhere a shell would find it.
fn missing(hub: &mut Hub) {
    let feed = hub
        .panes
        .get_mut::<Shells>(Some(SCENE_PANE))
        .and_then(|shells| {
            let shell = shells.tabs.first_mut()?;
            shell.program = "/bin/zsh".to_owned();
            Some(shell.feed)
        });
    if let Some(feed) = feed {
        let failed = FeedEvent::Status(FeedStatus::Error(NO_YI.to_owned()));
        step(&mut hub.model, Msg::Feed(feed, failed));
    }
}

/// The agent's tab shown: the recorded bash call, allowed, run and finished.
fn agent_tab(hub: &mut Hub) {
    let _ = yd_core::wire::replay::play(&mut hub.model, ASK);
    if let Some(shells) = hub.panes.get_mut::<Shells>(Some(SCENE_PANE)) {
        shells.active = Tab::Agent;
        if let Some(shell) = shells.tabs.first_mut() {
            shell.program = "/bin/zsh".to_owned();
        }
    }
}

/// [`agent_tab`] with a commit message written through a heredoc, as an agent writes one.
#[cfg(test)]
fn heredoc(hub: &mut Hub) {
    let said = ASK.replace(
        "echo one > one.txt",
        r"git commit -F - <<'EOF'\nFix tally tests\nEOF",
    );
    let _ = yd_core::wire::replay::play(&mut hub.model, &said);
}

#[cfg(test)]
mod tests;
