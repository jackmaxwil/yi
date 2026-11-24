//! Daemons wiring: the row commands the launchd source carries out, and the question asked
//! before a service is stopped or stops starting at login.

use rune::{Cursor, El, row, text};
use yd_core::command::table::{Command, Route, Run, Target};
use yd_core::command::tools::service;
use yd_core::feed::SourceCmd;
use yd_core::ids::{CommandId, PaneId, RowId};
use yd_core::kind::Kind;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_core::row::Value;
use yd_ui::pane::{PaneCx, SCENE_PANE};
use yd_ui::theme::look::{META, colour};
use yd_ui::win::{Hub, Win, dispatch};

use super::pane::FEED;

/// The two service commands the shared table has no row for; the rest are in `by_kind`.
pub const COMMANDS: &[Command] = &[
    row_for(service::START, "Start Service"),
    row_for(service::LOGIN, "Toggle Service Login"),
    row_for(super::desc::SIGN_IN, "Sign In"),
];

const fn row_for(id: CommandId, title: &'static str) -> Command {
    Command {
        id,
        title,
        key: None,
        menu: None,
        kinds: &[Kind::Service],
        route: Route::Model,
    }
}

/// A command waiting on the user's yes, and the words that ask it.
#[derive(Debug, Clone, PartialEq)]
pub struct Ask {
    pub run: Run,
    pub words: String,
    pub yes: &'static str,
}

/// What a service command does when pressed.
#[derive(Debug, PartialEq)]
pub enum Step {
    Send(Run),
    Ask(Ask),
}

/// Stopping a service, and turning its login off, ask first; everything else goes at once.
pub fn plan(run: Run, label: &str, login_on: bool) -> Step {
    let ask = |words: String, yes| {
        Step::Ask(Ask {
            run: run.clone(),
            words,
            yes,
        })
    };
    if run.command == service::STOP {
        let until = if login_on {
            "It is set to run at login, so it starts again the next time you log in."
        } else {
            "It stays stopped until you start it."
        };
        return ask(format!("Stop {label}? {until}"), "Stop");
    }
    if run.command == service::LOGIN && login_on {
        return ask(
            format!("Stop {label} starting when you log in?"),
            "Turn off",
        );
    }
    Step::Send(run)
}

/// The question a pane holds while it waits for the answer.
#[derive(Debug, Default)]
struct Asking(Option<Ask>);

fn panes(hub: &Hub) -> Vec<PaneId> {
    let all = hub.model.grid.panes.iter();
    let mut found: Vec<PaneId> = all
        .filter(|p| p.kind == PaneKind::Daemons)
        .map(|p| p.id)
        .collect();
    if found.is_empty() {
        found.push(SCENE_PANE);
    }
    found
}

/// Carries out a service command aimed at a service row: at once, or after the pane's question
/// is answered. False for any other command or target.
pub fn carry(win: &mut Win, _tx: &mut rune::Tx, command: CommandId, target: &Target) -> bool {
    let Target::Thing(address) = target else {
        return false;
    };
    let ours = [
        service::START,
        service::STOP,
        service::RESTART,
        service::LOGS,
        service::LOGIN,
    ];
    if address.kind() != Kind::Service || !ours.contains(&command) {
        return false;
    }
    let step = {
        let Ok(hub) = win.hub.try_borrow() else {
            return false;
        };
        let row = hub
            .model
            .tables
            .get(FEED)
            .and_then(|t| t.find(&RowId::new(address.id())));
        let login_on = row.and_then(|r| r.get("login")) == Some(&Value::Flag(true));
        plan(Run::new(command).on(target.clone()), address.id(), login_on)
    };
    match step {
        Step::Send(run) => dispatch(win, Msg::Source(FEED, SourceCmd::Run(run))),
        Step::Ask(ask) => {
            if let Ok(mut hub) = win.hub.try_borrow_mut() {
                for pane in panes(&hub) {
                    if let Some(kept) = hub.panes.get_mut::<Asking>(Some(pane)) {
                        kept.0 = Some(ask.clone());
                    }
                }
            }
        }
    }
    true
}

/// Closes the question; on a yes the command goes to the source, on a no nothing does.
fn answer(win: &mut Win, pane: PaneId, yes: bool) {
    let asked = win
        .hub
        .try_borrow_mut()
        .ok()
        .and_then(|mut hub| hub.panes.get_mut::<Asking>(Some(pane))?.0.take());
    if let (Some(ask), true) = (asked, yes) {
        dispatch(win, Msg::Source(FEED, SourceCmd::Run(ask.run)));
    }
}

/// The line above the list that asks, with its two answers; none while nothing is asked.
pub fn ask_bar(pane: &PaneCx<'_>) -> Option<El> {
    let ask = pane.state::<Asking>()?.0.as_ref()?;
    let (id, tokens, key) = (pane.pane, pane.tokens(), pane.key(PaneKind::Daemons));
    let button = |words: &str, ink, yes: bool| {
        let key = format!("{key}.ask.{}", if yes { "yes" } else { "no" });
        let press = move |win: &mut Win| answer(win, id, yes);
        text(words)
            .key(key)
            .font(META)
            .color(colour(ink))
            .cursor(Cursor::Pointer)
            .on_press(press)
    };
    let question = text(ask.words.clone()).font(META).color(colour(tokens.dim));
    let bar = row()
        .key(format!("{key}.ask"))
        .px(8.0)
        .py(4.0)
        .gap(8.0)
        .center_y();
    Some(
        bar.child(question)
            .child(button(ask.yes, tokens.bad, true))
            .child(button("Keep", tokens.dim, false)),
    )
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::error::Error;
    use std::rc::Rc;

    use rune::{Env, Runtime, col};
    use yd_core::address::uri::Address;
    use yd_core::feed::FeedEvent;
    use yd_core::ids::WindowId;
    use yd_core::msg::{Effect, step};
    use yd_core::row::Row;
    use yd_ui::pane::PaneDef;

    use super::*;

    type Check = Result<(), Box<dyn Error>>;
    type Sent = Rc<RefCell<Vec<Effect>>>;

    fn on(label: &str) -> Result<Target, Box<dyn Error>> {
        Ok(Target::Thing(Address::new(Kind::Service, label)?))
    }

    /// A window over a hub with one service row, and every effect the window sends.
    fn window(login: bool) -> Result<(Runtime<Win>, Sent), Box<dyn Error>> {
        static DEFS: [&PaneDef; 0] = [];
        let sent = Rc::new(RefCell::new(Vec::new()));
        let hub = Hub::new(&DEFS, Vec::new());
        {
            let mut hub = hub.try_borrow_mut()?;
            let into = sent.clone();
            hub.set_sink(move |effect| into.borrow_mut().push(effect));
            let service = Row::new("com.yi.serve").cell("login", Value::Flag(login));
            step(
                &mut hub.model,
                Msg::Feed(FEED, FeedEvent::Reset(vec![service])),
            );
        }
        let win = Win::new(hub, WindowId(0));
        Ok((
            Runtime::new(win, |_, _| col(), (300.0, 200.0), Env::new(true, 1.0)),
            sent,
        ))
    }

    fn carried(rt: &mut Runtime<Win>, command: CommandId) -> Result<bool, Box<dyn Error>> {
        let target = on("com.yi.serve")?;
        let mut done = false;
        rt.update(|win, tx| done = carry(win, tx, command, &target));
        Ok(done)
    }

    fn run(command: CommandId) -> Result<Effect, Box<dyn Error>> {
        Ok(Effect::Source(
            FEED,
            SourceCmd::Run(Run::new(command).on(on("com.yi.serve")?)),
        ))
    }

    #[test]
    fn start_restart_and_logs_go_to_the_source_at_once() -> Check {
        for command in [service::START, service::RESTART, service::LOGS] {
            let (mut rt, sent) = window(true)?;
            assert!(carried(&mut rt, command)?);
            assert_eq!(*sent.borrow(), [run(command)?], "{command:?}");
        }
        Ok(())
    }

    #[test]
    fn stop_asks_first_and_declining_sends_nothing() -> Check {
        let (mut rt, sent) = window(true)?;
        assert!(carried(&mut rt, service::STOP)?);
        assert!(
            sent.borrow().is_empty(),
            "nothing is sent before the answer"
        );
        rt.update(|win, _| answer(win, SCENE_PANE, false));
        assert!(sent.borrow().is_empty(), "a no sends nothing");
        assert!(carried(&mut rt, service::STOP)?);
        rt.update(|win, _| answer(win, SCENE_PANE, true));
        assert_eq!(*sent.borrow(), [run(service::STOP)?]);
        Ok(())
    }

    #[test]
    fn login_asks_only_when_it_turns_login_off() -> Check {
        let (mut rt, sent) = window(false)?;
        assert!(carried(&mut rt, service::LOGIN)?);
        assert_eq!(
            *sent.borrow(),
            [run(service::LOGIN)?],
            "turning it on is not asked"
        );
        let (mut rt, sent) = window(true)?;
        assert!(carried(&mut rt, service::LOGIN)?);
        assert!(sent.borrow().is_empty(), "turning it off is asked first");
        Ok(())
    }

    #[test]
    fn a_command_or_target_that_is_not_a_service_is_left_alone() -> Check {
        let (mut rt, sent) = window(true)?;
        let mut done = true;
        let container = Target::Thing(Address::new(Kind::Container, "db")?);
        rt.update(|win, tx| done = carry(win, tx, service::STOP, &container));
        assert!(!done && !carried(&mut rt, yd_core::command::tools::container::STOP)?);
        assert!(sent.borrow().is_empty());
        Ok(())
    }
}
