//! Containers wiring: the row commands the Docker source carries out.

use rune::{Cursor, El, Tx, row, text};
use yd_core::command::table::{Run, Target};
use yd_core::command::tools::container::{LOGS, REMOVE, RESTART, SHELL, START, STOP};
use yd_core::feed::{CONTAINERS, SourceCmd};
use yd_core::ids::CommandId;
use yd_core::kind::Kind;
use yd_core::msg::Msg;
use yd_core::row::Rows;
use yd_core::tokens::Tokens;
use yd_ui::theme::look::{META, colour};
use yd_ui::win::{Hub, Win, dispatch};

/// Every container command the window carries out; the box, the menu and a row's buttons
/// reach them through [`carry`]. Their rows are `yd-core`'s, which the right-click menu reads.
pub const IDS: &[CommandId] = &[LOGS, SHELL, RESTART, STOP, START, REMOVE];

/// A stop or a remove waiting on the user's answer: nothing runs until it is yes.
#[derive(Debug, Default)]
pub struct Asking(Option<(CommandId, Target, String)>);

/// Carries out a container command: stop and remove ask first, a shell opens a tab in a
/// terminal pane, the rest go to the source. False for anything that is not a container.
pub fn carry(win: &mut Win, tx: &mut Tx, command: CommandId, target: &Target) -> bool {
    let Some(name) = container_name(target).filter(|_| IDS.contains(&command)) else {
        return false;
    };
    match command {
        STOP | REMOVE => ask(win, command, target, name),
        SHELL => shell(win, tx, &name),
        LOGS => {
            let run = Run::new(command).on(target.clone());
            dispatch(win, Msg::Source(CONTAINERS, SourceCmd::Run(run)));
            // The lines come back under the open row; the Logs pane shows only a source it has.
            let source = format!("docker:{name}");
            if logged(win, &source) {
                crate::logs::pane::only(win, tx, &source);
            }
            true
        }
        _ => {
            let run = Run::new(command).on(target.clone());
            dispatch(win, Msg::Source(CONTAINERS, SourceCmd::Run(run)));
            true
        }
    }
}

/// Whether some line of the logs feed comes from `source`.
fn logged(win: &Win, source: &str) -> bool {
    let Ok(hub) = win.hub.try_borrow() else {
        return false;
    };
    let Some(logs) = hub.model.tables.get(crate::logs::pane::FEED) else {
        return false;
    };
    let from = |at: usize| logs.get(at).and_then(|line| line.words("source")) == Some(source);
    (0..logs.len()).any(from)
}

/// The container a command is aimed at: an address of that kind whose id is a Docker name,
/// the only text that ever reaches an argument or a shell line.
fn container_name(target: &Target) -> Option<String> {
    let Target::Thing(address) = target else {
        return None;
    };
    let valid = address
        .id()
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
        && address
            .id()
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c));
    (address.kind() == Kind::Container && valid).then(|| address.id().to_owned())
}

fn ask(win: &mut Win, command: CommandId, target: &Target, name: String) -> bool {
    let Ok(mut hub) = win.hub.try_borrow_mut() else {
        return false;
    };
    let Some(asking) = hub.panes.get_mut::<Asking>(None) else {
        return false;
    };
    asking.0 = Some((command, target.clone(), name));
    true
}

/// Shows the Terminal pane and types the `docker exec` line into a new tab of it.
fn shell(win: &mut Win, tx: &mut Tx, name: &str) -> bool {
    crate::terminal::pane::type_in(win, tx, &format!("docker exec -it {name} sh\r"))
}

/// The user's answer to [`Asking`]: yes carries the command out, no drops it.
pub fn answer(win: &mut Win, yes: bool) {
    let asked = win
        .hub
        .try_borrow_mut()
        .ok()
        .and_then(|mut hub| hub.panes.get_mut::<Asking>(None)?.0.take());
    if let (true, Some((command, target, _))) = (yes, asked) {
        let run = Run::new(command).on(target);
        dispatch(win, Msg::Source(CONTAINERS, SourceCmd::Run(run)));
    }
}

/// The question a stop or a remove waits on, naming the container; none when nothing waits.
/// The pane that draws the list puts it over the list.
pub fn question(hub: &Hub, tokens: &Tokens) -> Option<El> {
    let (command, _, name) = hub.panes.get::<Asking>(None)?.0.as_ref()?;
    let (words, yes) = match *command {
        REMOVE => (format!("Remove {name}? It cannot be undone."), "Remove"),
        _ => (format!("Stop {name}?"), "Stop"),
    };
    let button = |key: &str, words: &str, ink, yes| {
        text(words.to_owned())
            .key(format!("containers.ask.{key}"))
            .font(META)
            .color(colour(ink))
            .cursor(Cursor::Pointer)
            .on_press(move |win: &mut Win| answer(win, yes))
    };
    Some(
        row()
            .key("containers.ask")
            .gap(8.0)
            .px(8.0)
            .py(4.0)
            .center_y()
            .child(text(words).font(META).color(colour(tokens.dim)))
            .child(button("yes", yes, tokens.bad, true))
            .child(button("no", "Keep", tokens.dim, false)),
    )
}

#[cfg(test)]
#[rustfmt::skip]
mod tests {
    use std::cell::RefCell;
    use std::error::Error;
    use std::rc::Rc;

    use rune::{Cx, Env, Runtime, col};
    use yd_core::address::uri::Address;
    use yd_core::grid::step::GridMsg;
    use yd_core::ids::WindowId;
    use yd_core::msg::{Effect, step};
    use yd_core::pane::PaneKind;
    use yd_ui::menu::command_box::{install, run as run_command};
    use yd_core::feed::FeedEvent;
    use yd_core::row::Row;
    use yd_ui::win::Shared;

    use super::*;

    type Check = Result<(), Box<dyn Error>>;
    type Seen = Rc<RefCell<Vec<Effect>>>;

    const VERBS: [(&str, CommandId); 6] = [("logs", LOGS), ("shell", SHELL), ("restart", RESTART), ("stop", STOP), ("start", START), ("remove", REMOVE)];

    fn at(name: &str) -> Target {
        Address::new(Kind::Container, name).map(Target::Thing).unwrap_or_default()
    }

    /// A window with one button per command aimed at `atlas-api`, and the question under them.
    fn root(win: &Win, _cx: &mut Cx<Win>) -> El {
        let mut buttons = row();
        for (verb, id) in VERBS {
            let go = run_target(id);
            buttons = buttons.child(text(verb).key(format!("t.{verb}")).on_press_tx(move |win: &mut Win, tx: &mut Tx| { run_command(win, tx, go.clone()); }));
        }
        let ask = win.hub.try_borrow().ok().and_then(|hub| question(&hub, &hub.model.tokens));
        col().child(buttons).children(ask)
    }

    fn run_target(id: CommandId) -> Run {
        Run::new(id).on(at("atlas-api"))
    }

    fn window(terminal: bool) -> Result<(Runtime<Win>, Seen, Shared), Box<dyn Error>> {
        let hub = Hub::new(&[], Vec::new());
        let seen: Seen = Rc::default();
        {
            let mut hub = hub.try_borrow_mut()?;
            install(&mut hub, IDS, carry);
            let log = Rc::clone(&seen);
            hub.set_sink(move |effect| log.borrow_mut().push(effect));
            if terminal {
                step(&mut hub.model, Msg::Grid(GridMsg::Show { window: WindowId(0), saved: "everything" }));
            }
        }
        let mut rt = Runtime::new(Win::new(hub.clone(), WindowId(0)), root, (400.0, 120.0), Env::new(true, 1.0));
        rt.frame(0.016);
        Ok((rt, seen, hub))
    }

    fn press(rt: &mut Runtime<Win>, key: &str) -> Check {
        let tree = rt.elements();
        let hit = tree.iter().find(|e| e.key.as_deref() == Some(key) && !e.exiting);
        let [x, y, w, h] = hit.ok_or_else(|| format!("no {key}"))?.rect;
        let (x, y) = (x + w / 2.0, y + h / 2.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..3 { rt.frame(0.016); }
        Ok(())
    }

    fn said(rt: &Runtime<Win>) -> Vec<String> {
        rt.elements().iter().filter(|e| !e.exiting).filter_map(|e| e.text.clone()).collect()
    }

    fn sent(command: CommandId) -> Effect {
        Effect::Source(CONTAINERS, SourceCmd::Run(run_target(command)))
    }

    #[test]
    fn stop_and_remove_ask_by_name_and_run_only_on_yes() -> Check {
        for (verb, id, question, yes) in [("stop", STOP, "Stop atlas-api?", "Stop"), ("remove", REMOVE, "Remove atlas-api? It cannot be undone.", "Remove")] {
            let (mut rt, seen, _) = window(false)?;
            press(&mut rt, &format!("t.{verb}"))?;
            assert!(seen.borrow().is_empty(), "{verb} ran before the user answered");
            let words = said(&rt);
            assert!(words.contains(&question.to_owned()) && words.contains(&yes.to_owned()), "{words:?}");
            press(&mut rt, "containers.ask.no")?;
            assert!(seen.borrow().is_empty() && !said(&rt).contains(&question.to_owned()), "declining {verb} still ran or still asks");
            press(&mut rt, &format!("t.{verb}"))?;
            press(&mut rt, "containers.ask.yes")?;
            assert_eq!(*seen.borrow(), [sent(id)]);
        }
        Ok(())
    }

    #[test]
    fn start_restart_and_logs_go_straight_to_the_source() -> Check {
        let (mut rt, seen, _) = window(false)?;
        for verb in ["start", "restart", "logs"] { press(&mut rt, &format!("t.{verb}"))?; }
        assert_eq!(*seen.borrow(), [sent(START), sent(RESTART), sent(LOGS)]);
        Ok(())
    }

    #[test]
    fn logs_show_the_logs_pane_only_once_a_source_lists_the_container() -> Check {
        let filtered = |hub: &Shared| -> Result<bool, Box<dyn Error>> {
            let hub = hub.try_borrow()?;
            Ok(hub.model.grid.panes.iter().any(|p| p.kind == PaneKind::Logs))
        };
        let (mut rt, _, hub) = window(true)?;
        press(&mut rt, "t.logs")?;
        assert!(!filtered(&hub)?, "no source lists it: no empty Logs pane");
        let line = Row::new("t1").text("source", "docker:atlas-api").text("message", "up");
        rt.update(move |win: &mut Win, _| dispatch(win, Msg::Feed(crate::logs::pane::FEED, FeedEvent::Reset(vec![line]))));
        press(&mut rt, "t.logs")?;
        assert!(filtered(&hub)?, "a source lists it: the Logs pane shows its lines");
        Ok(())
    }

    #[test]
    fn shell_opens_a_tab_in_the_windows_terminal_and_types_the_exec_line_and_does_nothing_with_no_window() -> Check {
        let (mut rt, seen, _) = window(false)?;
        press(&mut rt, "t.shell")?;
        assert!(seen.borrow().is_empty());
        let (mut rt, seen, hub) = window(true)?;
        press(&mut rt, "t.shell")?;
        let seen = seen.borrow();
        let typed: Vec<_> = seen.iter().filter_map(|effect| match effect { Effect::Source(feed, SourceCmd::Input(line)) => Some((*feed, line.clone())), _ => None }).collect();
        let [(feed, line)] = &typed[..] else { return Err(format!("{seen:?}").into()) };
        assert_eq!(line, b"docker exec -it atlas-api sh\r");
        let hub = hub.try_borrow()?;
        let terminal = hub.model.grid.panes.iter().find(|p| p.kind == PaneKind::Terminal).ok_or("no terminal")?.id;
        let tabs = hub.panes.get::<yd_ui::term::view::state::Shells>(Some(terminal)).map(|s| s.tabs.iter().map(|t| t.feed).collect::<Vec<_>>());
        assert_eq!(tabs.as_deref().and_then(|t| t.last()), Some(feed));
        Ok(())
    }

    #[test]
    fn a_target_that_is_not_a_docker_name_is_refused() {
        for bad in ["a b; rm -rf /", "-v", "é"] {
            let target = Address::new(Kind::Container, bad).map(Target::Thing).unwrap_or_default();
            assert_eq!(container_name(&target), None, "{bad}");
        }
        assert_eq!(container_name(&at("atlas-api.2_x")).as_deref(), Some("atlas-api.2_x"));
    }
}
