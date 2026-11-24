//! The bell's list, system notifications, the dock badge and the threads kept attached.

use std::collections::BTreeSet;

use rune::{App, AppHandle, Taskbar, WindowHandle};
use rune_notify::Notification;
use yd_core::address::uri::Address;
use yd_core::command::table::{Route, Run, Target};
use yd_core::ids::{Millis, ThreadId, WindowId};
use yd_core::kind::Kind;
use yd_core::msg::{Msg, Notice};
use yd_core::reduce::state::{Link, Wire};
use yd_core::reduce::step::commands::{CLOSE, OPEN};
use yd_core::reduce::threads::Want;
use yd_panes::notifications::view::{self, Item};
use yd_ui::shown::show;
use yd_ui::win::{Hub, Win, dispatch, drain};

use crate::menubar;

/// What the app keeps between frames.
#[derive(Default)]
struct Notify {
    /// The windows the user is in; the app is in front while any is.
    front: BTreeSet<WindowId>,
    /// When the link first came up: what happened before that is not news.
    since: Option<Millis>,
    /// The threads this app holds open only because they need the user.
    kept: BTreeSet<ThreadId>,
    badge: u32,
    dock: Option<WindowHandle>,
}

/// What a frame decides: notices to post, the badge, the threads to attach and let go.
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    pub post: Vec<Notice>,
    pub badge: Option<u32>,
    pub attach: Vec<ThreadId>,
    pub detach: Vec<ThreadId>,
}

/// How many threads wait on the user: an ask to answer, a question, a child that is stuck.
pub fn needing(wire: &Wire) -> u32 {
    let waiting = |want: &Want| matches!(want, Want::Approval(_) | Want::NeedsYou | Want::Child(_));
    let count = wire
        .attention()
        .alerts
        .iter()
        .filter(|alert| waiting(&alert.want))
        .count();
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// The threads to hold open: each that needs the user, and each with an ask not yet settled.
/// Invariant: closing a thread cancels its asks, answered but unconfirmed ones included.
fn wanted(wire: &Wire) -> BTreeSet<ThreadId> {
    let asked = wire.asks.iter().map(|ask| ask.thread.clone());
    let waiting = wire.attention().alerts;
    let waiting = waiting
        .into_iter()
        .filter(|alert| !matches!(alert.want, Want::Failed | Want::Done));
    asked.chain(waiting.map(|alert| alert.thread)).collect()
}

impl Notify {
    /// Decides one frame from the wire and the news the bell found. A notice is for a row
    /// that is new, unread, newer than the link, and only while the app is not in front.
    fn plan(&mut self, wire: &Wire, news: &[Item]) -> Plan {
        if self.since.is_none()
            && let Link::Up { since, .. } = wire.link
        {
            self.since = Some(since);
        }
        let away = self.front.is_empty();
        let started = self.since;
        let fresh = |item: &&Item| !item.read && started.is_some_and(|since| item.since >= since);
        let post = news.iter().filter(fresh).filter(|_| away);
        let post = post.map(|item| Notice {
            thread: Some(item.thread.clone()),
            title: item.title.clone(),
            body: item.detail.clone(),
        });
        let count = needing(wire);
        let badge = (count != self.badge).then_some(count);
        self.badge = count;
        let wanted = wanted(wire);
        let attach = wanted.difference(&self.kept).cloned().collect();
        let detach = self.kept.difference(&wanted).cloned().collect();
        self.kept = wanted;
        Plan {
            post: post.collect(),
            badge,
            attach,
            detach,
        }
    }
}

fn aim(command: yd_core::ids::CommandId, thread: &ThreadId, win: &mut Win) {
    if let Ok(address) = Address::new(Kind::Thread, thread.as_str()) {
        dispatch(
            win,
            Msg::Command(Route::Wire, Run::new(command).on(Target::Thing(address))),
        );
    }
}

/// Posts a notice through the system. Only a bundled app can; anywhere else it is dropped.
pub fn post(notice: &Notice) {
    let Some(thread) = &notice.thread else { return };
    let note = Notification::new(thread.as_str(), &notice.title)
        .body(&notice.body)
        .info("thread", thread.as_str());
    let _ = rune_notify::post(&note);
}

/// What the workers sent, then the bell, the notices, the badge, the status items and the
/// threads kept attached. Invariant: a wake polls a hidden window, so the items stay live.
pub fn poll(win: &mut Win) -> bool {
    let drained = drain(win);
    let news = view::sync(win);
    let plan = {
        let Ok(mut hub) = win.hub.try_borrow_mut() else {
            return false;
        };
        let hub = &mut *hub;
        let Some(state) = hub.panes.get_mut::<Notify>(None) else {
            return false;
        };
        let plan = state.plan(&hub.model.wire, &news);
        if let (Some(count), Some(dock)) = (plan.badge, &state.dock) {
            let words = format!("{count} threads need you");
            dock.set_taskbar(Taskbar::new().badge(count, words));
        }
        plan
    };
    plan.post.iter().for_each(post);
    menubar::refresh(&win.hub);
    for thread in &plan.attach {
        aim(OPEN, thread, win);
    }
    for thread in &plan.detach {
        aim(CLOSE, thread, win);
    }
    drained || !plan.attach.is_empty() || !plan.detach.is_empty()
}

/// Records that `win` is in front or not; the app is in front while any window is. Every
/// window's focus hook calls it.
pub fn focus(win: &mut Win, focused: bool) {
    let window = win.window;
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(state) = hub.panes.get_mut::<Notify>(None)
    {
        if focused {
            state.front.insert(window);
        } else {
            state.front.remove(&window);
        }
    }
}

/// Runs once the native window has its first frame: keeps the window the badge goes on, and
/// puts the status items in the system bar.
pub fn ready(win: &mut Win, window: WindowHandle, handle: &AppHandle<Win>) {
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(state) = hub.panes.get_mut::<Notify>(None)
    {
        state.dock = Some(window);
    }
    menubar::start(handle);
}

/// The Mac menus and a click on a notification. Call it on the main window's `App` before it
/// runs, [`ready`] from its ready hook, [`focus`] from every focus hook and [`poll`] from poll.
pub fn install(app: App<Win>, hub: &Hub) -> App<Win> {
    let handle = app.handle();
    let clicked = handle.clone();
    rune_notify::on_click(move |click| {
        let Some(thread) = click.get("thread").map(ThreadId::new) else {
            return;
        };
        clicked.update(move |win, _| show(win, thread));
        clicked.show();
        clicked.focus();
    });
    menubar::put_up(hub);
    menubar::items(hub)
        .into_iter()
        .fold(app.standard_menus(), App::menu_item)
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use yd_core::address::uri::Address;
    use yd_core::reduce::step::commands::ANSWER;
    use yd_core::wire::replay::play;
    use yd_panes::notifications::view::Bell;

    use super::*;
    use crate::menubar::tests::window;

    /// The server settles the ask and says the thread runs again.
    const SETTLED: &str = r#"{"dir":"recv","frame":{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"rate-limit","update":{"sessionUpdate":"_yi/event","event":{"type":"permission_resolved","toolCallId":"c1","allowed":true},"seq":12}}}}
{"dir":"recv","frame":{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"rate-limit","update":{"sessionUpdate":"state_update","state":"running"}}}}"#;
    /// The mockup's clock: the moment the Rate limit ask arrived.
    const NOW: Millis = Millis(1_794_679_124_000);

    fn items(win: &Win) -> Result<Vec<Item>, Box<dyn Error>> {
        let hub = win.hub.try_borrow()?;
        let bell = hub.panes.get::<Bell>(None).ok_or("no bell")?;
        Ok(bell.shown().into_iter().cloned().collect())
    }

    fn views(win: &Win, thread: &str) -> Result<u32, Box<dyn Error>> {
        let hub = win.hub.try_borrow()?;
        let found = hub.model.wire.threads.get(&ThreadId::new(thread));
        Ok(found.ok_or("no such thread")?.views)
    }

    fn titles(plan: &Plan) -> Vec<&str> {
        plan.post
            .iter()
            .map(|notice| notice.title.as_str())
            .collect()
    }

    #[test]
    fn what_is_news_is_posted_only_while_the_app_is_behind_and_newer_than_the_link()
    -> Result<(), Box<dyn Error>> {
        let (win, _) = window()?;
        let news = items(&win)?;
        let hub = win.hub.try_borrow()?;
        let mut state = Notify {
            since: Some(NOW),
            ..Notify::default()
        };
        let plan = state.plan(&hub.model.wire, &news);
        // Cold start and Checkout were already waiting when the link came up; Notes 2.4 is read.
        assert_eq!(titles(&plan), ["Rate limit"]);
        state.front.insert(WindowId(0));
        assert_eq!(
            titles(&state.plan(&hub.model.wire, &news)),
            [] as [&str; 0],
            "in front"
        );
        Ok(())
    }

    #[test]
    fn the_badge_counts_threads_that_need_the_user_and_is_set_when_it_changes()
    -> Result<(), Box<dyn Error>> {
        let (win, _) = window()?;
        let hub = win.hub.try_borrow()?;
        assert_eq!(
            needing(&hub.model.wire),
            2,
            "an ask and a question; a finished thread is not"
        );
        let mut state = Notify::default();
        assert_eq!(state.plan(&hub.model.wire, &[]).badge, Some(2));
        assert_eq!(state.plan(&hub.model.wire, &[]).badge, None, "unchanged");
        Ok(())
    }

    #[test]
    fn a_thread_that_needs_the_user_stays_open_until_its_ask_is_settled()
    -> Result<(), Box<dyn Error>> {
        let (mut win, sent) = window()?;
        assert!(poll(&mut win), "two threads need the user");
        assert_eq!(
            (views(&win, "rate-limit")?, views(&win, "cold-start")?),
            (1, 1)
        );
        assert!(!poll(&mut win), "nothing changed, nothing asked");
        assert_eq!(views(&win, "checkout")?, 0, "a finished thread is not held");

        let ask = Address::new(Kind::Approval, "perm_rate-limit_1")?;
        let run = Run::new(ANSWER).on(Target::Thing(ask));
        let answer = Run {
            text: Some("allow_once".to_owned()),
            ..run
        };
        dispatch(&mut win, Msg::Command(Route::Wire, answer));
        sent.borrow_mut().clear();
        poll(&mut win);
        assert_eq!(views(&win, "rate-limit")?, 1, "answered, not yet confirmed");
        let lines = sent.borrow().clone();
        assert!(
            !lines.iter().any(|line| line.contains("cancelled")),
            "{lines:?}"
        );

        play(&mut win.hub.try_borrow_mut()?.model, SETTLED)?;
        assert!(poll(&mut win));
        assert_eq!(views(&win, "rate-limit")?, 0, "settled, so it is let go");
        assert_eq!(views(&win, "cold-start")?, 1, "the question still waits");
        Ok(())
    }

    #[test]
    fn a_wire_change_reaches_the_status_counts_with_no_window_drawn() -> Result<(), Box<dyn Error>>
    {
        const ASKED: &str = r#"{"dir":"send","frame":{"jsonrpc":"2.0","id":9,"method":"session/list","params":{}}}"#;
        const LISTED: &str = r#"{"jsonrpc":"2.0","id":9,"result":{"sessions":[{"sessionId":"sessions","cwd":"/work/atlas-api","title":"Sessions","_meta":{"yi":{"unseen":0,"lastState":"running","lastEventMs":1794678944000,"pinnedMs":null}}}]}}"#;
        let (mut win, _) = window()?;
        let (sender, inbox) = yd_core::inbox::inbox(|| {});
        {
            let mut hub = win.hub.try_borrow_mut()?;
            hub.set_inbox(inbox);
            play(&mut hub.model, ASKED)?;
        }
        let line = yd_core::wire::event::WireEvent::Line(LISTED.to_owned());
        sender.send(Msg::Wire(line))?;
        poll(&mut win);
        let rows = menubar::menu(&*win.hub.try_borrow()?).rows;
        let working = yd_mac::Row::Header("1 working".to_owned());
        assert!(
            rows.contains(&working),
            "a hidden window's wake polls, and nothing else drains"
        );
        Ok(())
    }

    /// The real-window run: a window behind others draws no frame until it is shown, and a
    /// wake still polls it. What the server says then must not wait for the window.
    #[test]
    fn a_server_event_moves_a_window_nobody_draws_and_its_next_frame_is_current()
    -> Result<(), Box<dyn Error>> {
        const RUNNING: &str = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"cold-start","update":{"sessionUpdate":"state_update","state":"running"}}}"#;
        let (win, _) = window()?;
        let (sender, inbox) = yd_core::inbox::inbox(|| {});
        win.hub.try_borrow_mut()?.set_inbox(inbox);
        let size = (1200.0, 750.0);
        let mut rt = rune::Runtime::new(win, crate::root::view, size, rune::Env::new(true, 1.0));
        let polled = |rt: &mut rune::Runtime<Win>| {
            rt.update(|win, tx| {
                crate::root::poll(win, 0.0, tx);
            });
        };
        let drawn = |rt: &rune::Runtime<Win>| -> Vec<String> {
            let all = rt.elements().into_iter().filter(|e| !e.exiting);
            let words = all.filter_map(|e| e.text);
            words
                .filter(|t| t.ends_with(" need you") || t.ends_with(" working"))
                .collect()
        };
        polled(&mut rt);
        rt.frame(1.0 / 60.0);
        assert_eq!(drawn(&rt), ["2 need you"], "what the window showed last");

        // Behind other windows: the thread that asked a question runs again, the wake polls,
        // and no frame is drawn.
        let line = yd_core::wire::event::WireEvent::Line(RUNNING.to_owned());
        sender.send(Msg::Wire(line))?;
        polled(&mut rt);
        {
            let hub = rt.state.hub.try_borrow()?;
            assert_eq!(needing(&hub.model.wire), 1, "the model did not move");
            let menu = menubar::menu(&hub);
            assert_eq!(menu.title, "● 1");
            let badge = hub.panes.get::<Notify>(None).map(|state| state.badge);
            assert_eq!(badge, Some(1), "the dock badge is behind");
        }
        assert_eq!(
            drawn(&rt),
            ["2 need you"],
            "no frame was drawn while hidden"
        );

        rt.frame(1.0 / 60.0);
        assert_eq!(
            drawn(&rt),
            ["1 need you", "1 working"],
            "the first frame shown"
        );
        Ok(())
    }
}
