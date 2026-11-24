//! Every pane's empty, loading, offline and error states: each draws a body, loading waits
//! before it shows, a lost server offers Reconnect, and a failed question keeps the rows.

mod support;

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use rune::{ElementInfo, Runtime};
use yd_core::feed::{FeedEvent, FeedStatus, LSP};
use yd_core::msg::{Effect, Msg};
use yd_core::pane::PaneKind;
use yd_ui::states::status::LOADING_AFTER;
use yd_ui::win::{Win, dispatch};

use support::scene::{WINDOW, runtime};

type Check = Result<(), Box<dyn Error>>;

const STATES: [&str; 4] = ["empty", "loading", "offline", "error"];
/// Themes always holds the built-in ones, so it has no state of its own to draw.
const LEFT: [PaneKind; 1] = [PaneKind::Themes];

fn found<'a>(all: &'a [ElementInfo], key: &str) -> Option<&'a ElementInfo> {
    all.iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key))
}

/// The words a pane's own body draws, where they can be seen; the tab bar is the chrome's.
fn body_words(all: &[ElementInfo], kind: PaneKind) -> Result<Vec<String>, Box<dyn Error>> {
    let root = found(all, &format!("{}-0", kind.slug())).ok_or("no pane root")?;
    let [x, y, w, h] = root.rect;
    let inside = |e: &&ElementInfo| {
        let [ex, ey, ..] = e.rect;
        ex >= x && ex <= x + w && ey >= y && ey <= y + h && e.opacity > 0.0
    };
    let words = all.iter().filter(|e| !e.exiting).filter(inside);
    Ok(words
        .filter_map(|e| e.text.clone())
        .filter(|t| !t.trim().is_empty())
        .collect())
}

fn settled(name: &str) -> Result<Runtime<Win>, Box<dyn Error>> {
    let mut rt = runtime(name, WINDOW)?;
    for _ in 0..30 {
        rt.frame(0.05);
    }
    Ok(rt)
}

#[test]
fn every_pane_draws_its_body_in_each_state() -> Check {
    let kinds = PaneKind::ALL.iter().filter(|kind| !LEFT.contains(kind));
    for kind in kinds {
        for state in STATES {
            let name = format!("{}/{state}", kind.slug());
            let rt = settled(&name)?;
            let words = body_words(&rt.elements(), *kind)?;
            assert!(!words.is_empty(), "{name} draws a blank body");
        }
    }
    Ok(())
}

/// A state that says only what is wrong leaves the reader stuck: offline and error each
/// add what to do, and an error also the source's own sentence.
#[test]
fn every_offline_and_error_state_says_what_to_do() -> Check {
    let mut silent = Vec::new();
    for kind in PaneKind::ALL.iter().filter(|kind| !LEFT.contains(kind)) {
        for (state, lines) in [("offline", 2), ("error", 3)] {
            let name = format!("{}/{state}", kind.slug());
            let rt = settled(&name)?;
            let all = rt.elements();
            // A pane with a state block says it there; the plugin pane says it in its own form.
            let pane = format!("/{}-0", kind.slug());
            let block = |e: &&ElementInfo| e.path.contains(&pane) && e.path.contains(".state/");
            let said = all.iter().filter(|e| !e.exiting).filter(block);
            let said = said.filter_map(|e| e.text.clone());
            let mut said: Vec<String> = said.filter(|t| !t.trim().is_empty()).collect();
            if said.is_empty() {
                said = body_words(&all, *kind)?;
            }
            if said.len() < lines {
                silent.push(format!("{name}: {said:?}"));
            }
        }
    }
    assert!(silent.is_empty(), "{}", silent.join("\n"));
    Ok(())
}

#[test]
fn loading_is_not_drawn_before_its_wait() -> Check {
    let mut rt = runtime("containers/loading", WINDOW)?;
    let shown = |rt: &Runtime<Win>| {
        let all = rt.elements();
        found(&all, "containers-0.state").map(|e| e.opacity)
    };
    rt.frame(LOADING_AFTER * 0.5);
    let early = shown(&rt).ok_or("no state block")?;
    assert!(early < 0.01, "loading showed {early} at half its wait");
    for _ in 0..10 {
        rt.frame(0.05);
    }
    let late = shown(&rt).ok_or("no state block")?;
    assert!(
        late > 0.99,
        "loading is still hidden after its wait: {late}"
    );
    Ok(())
}

#[test]
fn a_lost_server_offers_reconnect_and_pressing_it_asks_the_server() -> Check {
    let mut rt = settled("chat/offline")?;
    let sent: Rc<RefCell<Vec<Effect>>> = Rc::default();
    let log = Rc::clone(&sent);
    rt.state
        .hub
        .try_borrow_mut()?
        .set_sink(move |effect| log.borrow_mut().push(effect));
    let all = rt.elements();
    let words = body_words(&all, PaneKind::Chat)?;
    assert!(words.iter().any(|w| w == "Yi is not running"), "{words:?}");
    let button = found(&all, "chat-0.state.server.reconnect").ok_or("no Reconnect")?;
    let [x, y, w, h] = button.rect;
    rt.pointer_move(x + w / 2.0, y + h / 2.0);
    rt.pointer_down(x + w / 2.0, y + h / 2.0);
    rt.pointer_up(x + w / 2.0, y + h / 2.0);
    rt.frame(0.016);
    let asked = sent.borrow().iter().any(|effect| {
        matches!(effect, Effect::Wire(request) if request.line.contains("\"session/list\""))
    });
    assert!(asked, "Reconnect sent nothing: {:?}", sent.borrow());
    Ok(())
}

#[test]
fn a_failed_question_keeps_the_servers_and_says_so_above_them() -> Check {
    let mut rt = settled("lsp/mockup")?;
    let rows = |rt: &Runtime<Win>| {
        let all = rt.elements();
        let servers = all.iter().filter(|e| !e.exiting);
        let servers = servers.filter_map(|e| e.key.as_deref());
        servers.filter(|k| k.contains(".servers.row.")).count()
    };
    let before = rows(&rt);
    assert!(before > 0, "the mockup lists servers");
    let why = FeedStatus::Error("no answer within 10 s".to_owned());
    rt.update(move |win: &mut Win, _| dispatch(win, Msg::Feed(LSP, FeedEvent::Status(why))));
    for _ in 0..3 {
        rt.frame(0.016);
    }
    assert_eq!(rows(&rt), before, "a failed question blanked the servers");
    let words = body_words(&rt.elements(), PaneKind::LanguageServers)?;
    let said = "Could not update: no answer within 10 s";
    assert!(words.iter().any(|w| w == said), "{words:?}");
    Ok(())
}

/// Record 5.7's marks: covered is filled green, a gap a red ring, a check that does not apply
/// a faint fill, each read off the mockup's own rows.
#[test]
fn an_assets_check_is_a_mark_not_a_switch() -> Check {
    let rt = settled("assets/mockup")?;
    let all = rt.elements();
    let tokens = yd_core::tokens::Tokens::default();
    let mark = |id: &str, field: &str| found(&all, &format!("assets-0.row.{id}.{field}"));
    let covered = mark("hv-01", "reach").ok_or("no covered mark")?;
    assert_eq!(covered.fill, yd_ui::theme::look::colour(tokens.ok));
    let gap = mark("edge-01", "logs").ok_or("no gap mark")?;
    assert_eq!(gap.fill.a, 0.0, "a gap is hollow");
    let none = mark("nas-01", "backup").ok_or("no mark for a check that does not apply")?;
    assert!(none.fill.a > 0.0 && none.fill != covered.fill);
    Ok(())
}

/// Every word the scene `name` draws.
fn words_of(name: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let all = settled(name)?.elements();
    let live = all.into_iter().filter(|e| !e.exiting);
    Ok(live.filter_map(|e| e.text).collect())
}

/// The review's picture: an offline Containers header still read "docker · hv-01", the
/// mockup's host, which no row had named.
#[test]
fn a_list_with_no_rows_names_no_host() -> Check {
    for scene in [
        "containers/empty",
        "containers/offline",
        "vms/empty",
        "vms/error",
    ] {
        let words = words_of(scene)?;
        let named: Vec<&String> = words.iter().filter(|w| w.contains("hv-01")).collect();
        assert!(named.is_empty(), "{scene} names a host: {named:?}");
    }
    let listed = words_of("vms/mockup")?;
    assert!(
        listed.iter().any(|w| w == "hv-01"),
        "the rows' own host is gone"
    );
    Ok(())
}

/// The findings hub's header: "N open" and the severity chips beside it count the same
/// findings, so the chips add up to N.
#[test]
fn the_findings_chips_add_up_to_the_open_count() -> Check {
    for scene in ["findings/mockup", "findings-table/mockup"] {
        let words = words_of(scene)?;
        let open = words.iter().find_map(|w| w.strip_suffix(" open"));
        let open: u32 = open.ok_or("no open count")?.parse()?;
        let chip = |name: &str| {
            let said = words.iter().find_map(|w| w.strip_prefix(name));
            said.and_then(|n| n.trim().parse::<u32>().ok())
        };
        let chips = [chip("High "), chip("Med "), chip("Low ")];
        let sum: u32 = chips.iter().flatten().sum();
        assert_eq!(sum, open, "{scene}: {chips:?} beside {open} open");
    }
    Ok(())
}

/// The real-window run: Rune reported the conversation's scroll area 0 points tall. With no
/// thread it holds the state block, with one the transcript: either needs its height.
#[test]
fn the_conversation_has_height_with_no_thread_and_with_one() -> Check {
    for scene in ["grid/everything", "shell/mockup"] {
        let all = settled(scene)?.elements();
        let live = all.iter().filter(|e| !e.exiting);
        let mut talk = live.filter(|e| {
            e.key
                .as_deref()
                .is_some_and(|k| k.ends_with(".conversation"))
        });
        let talk = talk
            .next()
            .ok_or_else(|| format!("{scene} draws no conversation"))?;
        assert!(talk.rect[3] > 100.0, "{scene}: {:?}", talk.rect);
    }
    Ok(())
}

const STEP: f64 = 1.0 / 60.0;

fn first_window() -> Runtime<Win> {
    let mut rt = Runtime::new(
        yi_desktop::root::init(),
        yi_desktop::root::view,
        (1200.0, 750.0),
        rune::Env::new(true, 1.0),
    );
    rt.poll = Some(yi_desktop::root::poll);
    rt.on_key = Some(yi_desktop::root::on_key);
    rt
}

fn run_for(rt: &mut Runtime<Win>, secs: f64) -> u32 {
    let mut busy = 0;
    for _ in 0..(secs / STEP) as u32 {
        busy += u32::from(rt.frame(STEP));
    }
    busy
}

fn said(rt: &Runtime<Win>, words: &str) -> bool {
    let tree = rt.elements();
    tree.iter()
        .any(|e| !e.exiting && e.text.as_deref().is_some_and(|t| t.contains(words)))
}

#[test]
fn remote_desktop_with_no_machine_host_says_so_at_once_not_a_spinner() -> Check {
    let mut rt = first_window();
    run_for(&mut rt, 1.0);
    assert!(
        said(&rt, yd_core::command::tools::remote::NO_HOST),
        "the no-host sentence is not shown"
    );
    assert!(!said(&rt, "Opening the screen"), "the pane still spins");
    Ok(())
}

#[test]
fn a_pane_loading_for_ever_says_what_is_wrong_after_ten_seconds() -> Check {
    let mut rt = first_window();
    run_for(&mut rt, 5.0);
    let late = |rt: &Runtime<Win>| {
        let tree = rt.elements();
        let lines = tree
            .iter()
            .filter(|e| e.key.as_deref().is_some_and(|k| k.ends_with(".state.late")));
        lines.map(|e| e.opacity).fold(None, |most: Option<f32>, o| {
            Some(most.map_or(o, |m| m.max(o)))
        })
    };
    assert_eq!(
        late(&rt),
        Some(0.0),
        "no pane is loading, or the line shows too early"
    );
    run_for(&mut rt, 7.0);
    assert!(late(&rt).is_some_and(|o| o > 0.99), "{:?}", late(&rt));
    Ok(())
}

#[test]
fn a_settled_first_window_draws_no_frame() -> Check {
    let mut rt = first_window();
    run_for(&mut rt, 15.0);
    assert_eq!(
        run_for(&mut rt, 30.0),
        0,
        "frames drawn while nothing changed"
    );
    Ok(())
}
