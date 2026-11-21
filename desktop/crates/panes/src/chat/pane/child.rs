//! A chat pane bound to one child agent: its turns, its question, its stop and the way back.

use rune::{Cursor, El, Tx, col, el, row, spacer, text};
use yd_core::command::table::{Command, Route, Run, Target};
use yd_core::grid::step::GridMsg;
use yd_core::ids::{AgentId, CommandId, PaneId, ThreadId, WindowId};
use yd_core::kind::Kind;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_core::reduce::agents::{Agent, Doing, Flag};
use yd_core::reduce::approvals::Ask;
use yd_core::reduce::step::commands::{CHILD_REPLAY, CHILD_STOP, child_address, child_of};
use yd_core::reduce::threads::Thread;
use yd_core::reduce::turns::CallId;
use yd_core::tokens::Tokens;
use yd_ui::grid_view::drag::GLIDE;
use yd_ui::grid_view::view::{Act, act, show_pane};
use yd_ui::kit::button::{Button, button};
use yd_ui::menu::command_box;
use yd_ui::pane::PaneCx;
use yd_ui::theme::look::{BODY, LABEL, META, colour};
use yd_ui::win::{Hub, Win, dispatch};

use crate::chat::composer::{Composer, Form, Send, drawn};
use crate::chat::messages::{line, model_name, said};

/// Opens a chat pane on a child agent beside its parent's, or brings the open one forward.
pub const OPEN: CommandId = CommandId("agents.open-chat");

pub const COMMAND: Command = Command {
    id: OPEN,
    title: "Open Agent Chat",
    key: None,
    menu: None,
    kinds: &[Kind::Agent],
    route: Route::Model,
};

/// The child agent a chat pane shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Child {
    pub thread: ThreadId,
    pub agent: AgentId,
}

/// The child a chat pane is about: the one its thing in the grid names, kept with the grid.
pub fn bound(hub: &Hub, pane: PaneId) -> Option<Child> {
    let thing = hub.model.grid.thing(pane)?.clone();
    let (thread, agent) = child_of(&Target::Thing(thing))?;
    Some(Child { thread, agent })
}

pub fn agent<'a>(thread: &'a Thread, child: &Child) -> Option<&'a Agent> {
    thread.agents.iter().find(|agent| agent.id == child.agent)
}

fn address(child: &Child) -> Option<Target> {
    child_address(&child.thread, &child.agent)
        .ok()
        .map(Target::Thing)
}

/// The child an Open Agent Chat means: a child's address, an Agents row of the window's
/// thread (`yi://agent/<id>`), else the window's newest child.
fn aimed(hub: &Hub, window: WindowId, target: &Target) -> Option<Child> {
    if let Some((thread, agent)) = child_of(target) {
        return Some(Child { thread, agent });
    }
    let thread = yd_ui::shown::thread(hub, window)?;
    let named = match target {
        Target::Thing(address) if address.kind() == Kind::Agent => Some(address.id()),
        Target::Thing(_) | Target::App | Target::Pane(_) => None,
    };
    let agents = thread.agents.iter();
    let agent = match named {
        Some(id) => agents.into_iter().find(|agent| agent.id.as_str() == id),
        None => agents.into_iter().max_by_key(|agent| agent.first_seen),
    }?;
    Some(Child {
        thread: thread.id.clone(),
        agent: agent.id.clone(),
    })
}

/// Carries out Open Agent Chat; false for any other command or a target that is no child.
pub fn carry(win: &mut Win, tx: &mut Tx, command: CommandId, target: &Target) -> bool {
    if command != OPEN {
        return false;
    }
    let child = win
        .hub
        .try_borrow()
        .ok()
        .and_then(|hub| aimed(&hub, win.window, target));
    let Some(child) = child else {
        return false;
    };
    open(win, tx, child);
    true
}

/// Shows the chat about `child` in front: the pane the window has, else a new tab beside
/// the session's own chat, in the place work goes.
fn open(win: &mut Win, tx: &mut Tx, child: Child) {
    let Ok(thing) = child_address(&child.thread, &child.agent) else {
        return;
    };
    tx.motion_curve(GLIDE);
    let (window, kind) = (win.window, PaneKind::Chat);
    let about = GridMsg::About {
        window,
        kind,
        thing: thing.clone(),
    };
    dispatch(win, Msg::Grid(about));
    let made = win.hub.try_borrow().ok().and_then(|hub| {
        let mut panes = hub.model.grid.panes.iter();
        let found = panes.find(|pane| pane.thing.as_ref() == Some(&thing));
        found.map(|pane| pane.id)
    });
    if let Some(made) = made {
        act(win, tx, Act::Focus(made));
    }
}

/// Opens the chat of the child that `call` started, from a handler that knows the thread.
pub fn open_spawned(win: &mut Win, tx: &mut Tx, thread: &ThreadId, agent: &AgentId) {
    if let Ok(target) = child_address(thread, agent) {
        command_box::run(win, tx, Run::new(OPEN).on(Target::Thing(target)));
    }
}

/// Under a step that started a child: the way into that child's own chat.
pub fn spawned(key: &str, tokens: &Tokens, thread: &Thread, call: &str) -> Option<El> {
    let by = |agent: &&Agent| agent.spawned_by.as_ref().map(CallId::as_str) == Some(call);
    let agent = thread.agents.iter().find(by)?;
    let (id, name) = (agent.id.clone(), agent.update.name.clone());
    let thread = thread.id.clone();
    let words = format!("Open {}'s chat", line(&name, 40));
    let link = text(words.clone())
        .key(format!("{key}.spawned.{}", id.as_str()))
        .font(META)
        .color(colour(tokens.accent))
        .cursor(Cursor::Pointer)
        .label(words)
        .on_press_tx(move |win: &mut Win, tx: &mut Tx| open_spawned(win, tx, &thread, &id));
    Some(row().h(16.0).pl(19.0).center_y().child(link))
}

/// In an Agents pane: the agent row just opened, as a zero-size element whose first layout
/// opens that child's chat. A press and Return both open a row, so both come here.
pub fn opened(key: &str, thread: &Thread, open: Option<&str>) -> Option<El> {
    let agent = thread.agents.iter().find(|a| Some(a.id.as_str()) == open)?;
    let (thread, id) = (thread.id.clone(), agent.id.clone());
    let opens = el()
        .key(format!("{key}.opens.{}", id.as_str()))
        .absolute(0.0, 0.0)
        .size(0.0, 0.0)
        .on_layout_tx(move |win: &mut Win, _: [f32; 4], tx: &mut Tx| {
            open_spawned(win, tx, &thread, &id);
        });
    Some(opens)
}

/// What the child is doing now, or how it ended.
fn state(agent: &Agent) -> String {
    if let Some(ended) = agent.ended() {
        return ended.to_owned();
    }
    match (agent.flag(), agent.doing()) {
        (Some(Flag::NeedsYou), _) => "asks you".to_owned(),
        (Some(Flag::Stuck), _) => "stuck".to_owned(),
        (Some(Flag::Other) | None, Doing::Waiting) => "waiting".to_owned(),
        (Some(Flag::Other) | None, Doing::Writing) => "writing".to_owned(),
        (Some(Flag::Other) | None, Doing::Executing) => "running a tool".to_owned(),
        (Some(Flag::Other) | None, Doing::Other) => "working".to_owned(),
    }
}

/// The model its last answer names, as the thread's options name it.
fn model(thread: &Thread, agent: &Agent) -> Option<String> {
    let messages = agent.transcript.messages.iter().rev();
    let said = messages
        .filter_map(said)
        .find(|said| !said.user && !said.model.is_empty())?;
    Some(model_name(thread, &said.model))
}

/// The tab of a child's pane: its name, and its state while in front.
pub fn tab(thread: &Thread, child: &Child) -> (String, String) {
    match agent(thread, child) {
        Some(agent) => (line(&agent.update.name, 40), state(agent)),
        None => (line(child.agent.as_str(), 40), String::new()),
    }
}

/// The bar over the child's turns: back to the parent, its name or task, model and state, and
/// Stop while it runs.
pub fn header(pane: &PaneCx<'_>, key: &str, thread: &Thread, agent: Option<&Agent>) -> El {
    let tokens = pane.tokens();
    let parent = thread.title.as_deref().unwrap_or(thread.id.as_str());
    let back = format!("← {}", line(parent, 60));
    let back = text(back)
        .key(format!("{key}.parent"))
        .font(META)
        .color(colour(tokens.accent))
        .cursor(Cursor::Pointer)
        .label(format!("Back to the parent thread, {}", line(parent, 60)))
        .truncate()
        .on_press_tx(|win: &mut Win, tx: &mut Tx| {
            show_pane(win, tx, PaneKind::Chat);
        });
    let named = agent.map(|agent| {
        let said = &agent.update;
        let what = said.task.as_deref().filter(|_| said.name.is_empty());
        let mut words = line(what.unwrap_or(&said.name), 60);
        if let Some(model) = model(thread, agent) {
            words.push_str(&format!(" · {model}"));
        }
        words.push_str(&format!(" · {}", state(agent)));
        text(words)
            .key(format!("{key}.agent"))
            .font(LABEL)
            .color(colour(tokens.fg))
            .truncate()
            .shrink()
            .min_w(0.0)
    });
    let live = agent.filter(|agent| agent.ended().is_none() && !agent.gone);
    let stop = live.and_then(|agent| {
        let target = address(&Child {
            thread: thread.id.clone(),
            agent: agent.id.clone(),
        })?;
        let stop = button(tokens, Button::Ghost, "Stop", None)
            .key(format!("{key}.stop"))
            .label(format!("Stop {}", line(&agent.update.name, 40)))
            .on_press(move |win: &mut Win| {
                let run = Run::new(CHILD_STOP).on(target.clone());
                dispatch(win, Msg::Command(Route::Wire, run));
            });
        Some(stop)
    });
    let task = agent.and_then(|agent| agent.update.task.as_deref());
    let task = task.map(|task| {
        text(line(task, 240))
            .font(META)
            .color(colour(tokens.dim))
            .truncate()
    });
    let head = row()
        .w_full()
        .h(20.0)
        .gap(8.0)
        .center_y()
        .children(named)
        .child(spacer())
        .children(stop);
    col()
        .key(format!("{key}.child"))
        .w_full()
        .px(8.0)
        .pt(6.0)
        .gap(2.0)
        .child(back)
        .child(head)
        .children(task)
}

/// While the parent's branch is live, a zero-size element whose first layout asks for the
/// child's whole conversation: on opening, and again after every resume or restart.
pub fn replay(key: &str, thread: &Thread, child: &Child) -> Option<El> {
    let target = address(child).filter(|_| thread.sync.live())?;
    let asks = el()
        .key(format!("{key}.replay.{}", child.agent.as_str()))
        .absolute(0.0, 0.0)
        .size(0.0, 0.0)
        .on_layout_tx(move |win: &mut Win, _: [f32; 4], _: &mut Tx| {
            let run = Run::new(CHILD_REPLAY).on(target.clone());
            dispatch(win, Msg::Command(Route::Wire, run));
        });
    Some(asks)
}

/// The asks the child itself raised: its own calls are in its transcript.
pub fn asked(agent: &Agent, ask: &Ask) -> bool {
    let call = ask.tool.as_ref();
    call.is_some_and(|call| agent.transcript.tools.contains_key(call.as_str()))
}

/// What a child's pane says where its turns would be when there is no child to show.
pub fn missing(key: &str, tokens: &Tokens, agent: Option<&Agent>) -> El {
    let words = match agent {
        Some(agent) if agent.gone => {
            "This agent no longer exists on the server, so its conversation cannot be shown."
        }
        Some(_) | None => "Loading the agent's conversation…",
    };
    text(words)
        .key(format!("{key}.missing"))
        .font(BODY)
        .color(colour(tokens.dim))
        .wrap()
}

/// Under a child's turns: the composer while it asks a question, else a line saying why
/// there is none. Prompting a child is not on the wire; its parent drives it.
pub fn foot(pane: &PaneCx<'_>, key: &str, child: &Child, agent: Option<&Agent>) -> El {
    let tokens = pane.tokens();
    let note = |words: String| {
        text(words)
            .key(format!("{key}.driven"))
            .font(META)
            .color(colour(tokens.faint))
            .wrap()
    };
    let Some(agent) = agent.filter(|agent| !agent.gone) else {
        return col().w_full();
    };
    if let Some(ended) = agent.ended() {
        return note(format!(
            "This agent has {ended}; its conversation stays here to read."
        ));
    }
    let (Some((_, question)), Some(Target::Thing(to))) = (agent.question(), address(child)) else {
        return note("Its parent thread drives this agent: you can answer it here when it asks you something.".to_owned());
    };
    let asks = text(format!("Asks you: {}", line(question, 400)))
        .key(format!("{key}.question"))
        .font(BODY)
        .color(colour(tokens.fg))
        .wrap();
    let composer = Composer {
        key,
        pane: pane.pane,
        tokens,
        thread: None,
        draft: drawn(pane),
        queued: Vec::new(),
        held: false,
        files: None,
        hint: "Answer its question",
        form: Form::Pane,
        send: Send::Answer(to),
    };
    col().w_full().gap(6.0).child(asks).child(composer.view())
}
