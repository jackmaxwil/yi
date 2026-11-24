use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use rune::{Cx, El, ElementInfo, Env, Runtime, col};
use yd_core::feed::SourceCmd;
use yd_core::ids::WindowId;
use yd_core::msg::Effect;
use yd_core::pane::PaneKind;
use yd_ui::grid_view::view::{Act, act, alone};
use yd_ui::pane::{PaneDef, SCENE_PANE, Stage};
use yd_ui::term::view::state::{Shells, Tab};
use yd_ui::win::{Hub, Win};

use super::{ASK, DEF, agent_tab, heredoc, mockup};

static DEFS: &[&PaneDef] = &[&DEF];

/// The pane as a window draws it alone: under the chrome's bar, which shows the pane's tabs.
fn root(win: &Win, cx: &mut Cx<Win>) -> El {
    let Ok(hub) = win.hub.try_borrow() else {
        return col();
    };
    alone(win, &hub, cx, PaneKind::Terminal)
}

type Sent = Rc<RefCell<Vec<SourceCmd>>>;

fn scene(setup: fn(&mut Hub)) -> Result<(Runtime<Win>, Sent), Box<dyn Error>> {
    let shared = Hub::new(DEFS, Vec::new());
    let sent: Sent = Rc::default();
    {
        let mut hub = shared.try_borrow_mut()?;
        hub.stage = Stage::Pane(PaneKind::Terminal);
        let log = Rc::clone(&sent);
        hub.set_sink(move |effect| {
            if let Effect::Source(_, ask) = effect {
                log.borrow_mut().push(ask);
            }
        });
        setup(&mut hub);
    }
    let mut rt = Runtime::new(
        Win::new(shared, WindowId(1)),
        root,
        (600.0, 400.0),
        Env::new(true, 1.0),
    );
    for _ in 0..3 {
        rt.frame(1.0 / 60.0);
    }
    Ok((rt, sent))
}

fn texts(tree: &[ElementInfo]) -> Vec<String> {
    tree.iter()
        .filter_map(|e| e.text.clone().or_else(|| e.label.clone()))
        .collect()
}

fn height(tree: &[ElementInfo], suffix: &str) -> Result<f32, Box<dyn Error>> {
    let hit = tree
        .iter()
        .find(|e| e.key.as_deref().is_some_and(|k| k.ends_with(suffix)));
    Ok(hit.ok_or(format!("no {suffix}"))?.rect[3])
}

/// Dies with a mockup frame that is not the record's: the zsh tab, the agent's waiting
/// `cargo test`, the folder and exit, and the screen the parser drew from the sample.
#[test]
fn the_mockup_shows_frame_fourteen() -> Result<(), Box<dyn Error>> {
    let (rt, _) = scene(mockup)?;
    let tree = rt.elements();
    let shown = texts(&tree);
    for want in ["zsh", "agent: cargo test · waiting", "~/atlas-api · exit 0"] {
        assert!(shown.iter().any(|t| t == want), "{want:?} not in {shown:?}");
    }
    let screen = shown
        .iter()
        .find(|t| t.contains("git status"))
        .ok_or("no screen")?;
    let mut lines = screen.lines();
    assert_eq!(
        lines.next(),
        Some("atlas-api rate-limit-uploads $ git status --short")
    );
    assert_eq!(lines.next(), Some(" M src/routes/upload.rs"));
    assert_eq!(height(&tree, ".pane-tab-bar")?, 20.0);
    assert_eq!(height(&tree, ".pane-tab")?, 19.0);
    let bars = tree.iter().filter(|e| {
        let key = e.key.as_deref().unwrap_or_default();
        !e.exiting && key.ends_with(".pane-tab-bar")
    });
    assert_eq!(bars.count(), 1, "the pane draws no bar of its own");
    Ok(())
}

/// Dies with an agent tab that does not list what the agent ran, or how it ended.
#[test]
fn the_agent_tab_lists_the_commands_the_agent_ran() -> Result<(), Box<dyn Error>> {
    let (rt, _) = scene(agent_tab)?;
    let shown = texts(&rt.elements());
    assert!(
        shown
            .iter()
            .any(|t| t == "agent: echo one > one.txt · exit 0"),
        "{shown:?}"
    );
    assert!(shown.iter().any(|t| t == "echo one > one.txt"));
    assert!(
        shown.iter().any(|t| t.starts_with("exit 0 · ")),
        "{shown:?}"
    );
    Ok(())
}

/// The recorded bash call, allowed and finished, whose result is `printed`: what the server
/// sends when a command ends (`tool_execution_end`).
fn ran(hub: &mut Hub, printed: &str) {
    let recording = ASK.replace("(no output)", printed);
    let _ = yd_core::wire::replay::play(&mut hub.model, &recording);
    if let Some(shells) = hub.panes.get_mut::<Shells>(Some(SCENE_PANE)) {
        shells.active = Tab::Agent;
    }
}

/// Dies with an agent tab that shows `exit 0` and nothing the command printed.
#[test]
fn the_agent_tab_shows_what_the_command_printed() -> Result<(), Box<dyn Error>> {
    let (rt, _) = scene(|hub| ran(hub, "running 4 tests\\ntest a ... ok\\ntest b ... FAILED"))?;
    let shown = texts(&rt.elements());
    for want in ["running 4 tests", "test a ... ok", "test b ... FAILED"] {
        assert!(shown.iter().any(|t| t == want), "{want:?} not in {shown:?}");
    }
    Ok(())
}

/// Dies with a long result cut with no word of it: the tab keeps the end of the
/// command's output and says how many were kept of how many.
#[test]
fn a_long_result_says_how_much_of_it_the_agent_tab_shows() -> Result<(), Box<dyn Error>> {
    let setup: fn(&mut Hub) = |hub| {
        let many: Vec<String> = (1..=60).map(|n| format!("line {n}")).collect();
        ran(hub, &many.join("\\n"));
    };
    let (rt, _) = scene(setup)?;
    let shown = texts(&rt.elements());
    assert!(shown.iter().any(|t| t == "line 60"), "the end is kept");
    assert!(!shown.iter().any(|t| t == "line 20"), "the head is cut");
    let notice = shown
        .iter()
        .find(|t| t.starts_with("[…] last 40 of 60 lines"));
    assert!(notice.is_some(), "no notice at the cut: {shown:?}");
    Ok(())
}

/// Dies with a new-shell button that opens nothing, or a closed tab whose shell keeps running.
#[test]
fn a_new_shell_opens_and_closing_it_hangs_it_up() -> Result<(), Box<dyn Error>> {
    let (mut rt, sent) = scene(|_| {})?;
    let key = |rt: &Runtime<Win>, suffix: &str| {
        rt.elements()
            .into_iter()
            .find(|e| e.key.as_deref().is_some_and(|k| k.ends_with(suffix)))
    };
    let tap = |rt: &mut Runtime<Win>, suffix: &str| -> Result<(), Box<dyn Error>> {
        let e = key(rt, suffix).ok_or(format!("no {suffix}"))?;
        let (x, y) = (e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..3 {
            rt.frame(1.0 / 60.0);
        }
        Ok(())
    };
    tap(&mut rt, ".pane-tab-new")?;
    let tabs = |rt: &Runtime<Win>| -> Result<(usize, Tab), Box<dyn Error>> {
        let hub = rt.state.hub.try_borrow()?;
        let shells = hub
            .panes
            .get::<Shells>(Some(SCENE_PANE))
            .ok_or("no shells")?;
        Ok((shells.tabs.len(), shells.active))
    };
    let (count, active) = tabs(&rt)?;
    assert_eq!(count, 2);
    let Tab::Shell(second) = active else {
        return Err("the new shell is not shown".into());
    };
    // What the Close Tab command does to the pane in focus.
    rt.update(|win, tx| act(win, tx, Act::CloseTab(SCENE_PANE)));
    assert_eq!(tabs(&rt)?.0, 1);
    assert!(
        sent.borrow().contains(&SourceCmd::Close),
        "{:?}",
        sent.borrow()
    );
    assert_ne!(tabs(&rt)?.1, Tab::Shell(second));
    Ok(())
}

/// Dies with a replacement shell that is never told its size, so nothing starts it: its
/// pane says "Starting the shell" for ever and shows no prompt.
#[test]
fn a_shell_that_replaces_another_is_told_its_size() -> Result<(), Box<dyn Error>> {
    let (mut rt, sent) = scene(|_| {})?;
    let resized = |sent: &Sent| {
        let sent = sent.borrow();
        sent.iter()
            .filter(|ask| matches!(ask, SourceCmd::Resize { .. }))
            .count()
    };
    assert_eq!(resized(&sent), 1, "the first shell is told");
    rt.update(|win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut()
            && let Some(shells) = hub.panes.get_mut::<Shells>(Some(SCENE_PANE))
        {
            *shells = Shells::default();
        }
    });
    for _ in 0..5 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(resized(&sent), 2, "the replacement is told too");
    Ok(())
}

/// Dies with the keyboard thrown out of a shell the moment it starts: the screen moved in the
/// tree when the shell went live, and focus went to the pane's close button, which Return pressed.
#[test]
fn the_keyboard_stays_in_a_shell_when_it_goes_live() -> Result<(), Box<dyn Error>> {
    let (mut rt, _) = scene(|_| {})?;
    let focused = |rt: &Runtime<Win>| {
        let tree = rt.elements();
        tree.iter()
            .find(|e| e.focused && !e.exiting)
            .and_then(|e| e.key.clone())
    };
    let screen = rt
        .elements()
        .into_iter()
        .filter_map(|e| e.key)
        .find(|key| key.contains(".screen.") && !key.ends_with(".body"))
        .ok_or("no screen")?;
    let ask = screen.clone();
    rt.update(move |_, tx| tx.focus(ask, false));
    rt.frame(1.0 / 60.0);
    assert_eq!(
        focused(&rt),
        Some(screen.clone()),
        "the screen has the keyboard to begin with"
    );
    let feed = {
        let hub = rt.state.hub.try_borrow()?;
        let shells = hub
            .panes
            .get::<Shells>(Some(SCENE_PANE))
            .ok_or("no shells")?;
        let shell = shells.tabs.first().ok_or("no shell")?;
        if let Ok(mut parser) = shell.surface.parser.lock() {
            parser.process(b"% ");
        }
        shell.feed
    };
    rt.update(move |win, _| {
        let live = yd_core::feed::FeedEvent::Status(yd_core::feed::FeedStatus::Live);
        yd_ui::win::dispatch(win, yd_core::msg::Msg::Feed(feed, live));
    });
    for _ in 0..5 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(
        focused(&rt),
        Some(screen),
        "the shell went live and took the keyboard's place"
    );
    Ok(())
}

/// Dies with a tab that shows a command's later lines: a heredoc's body and its `EOF`.
#[test]
fn the_agent_tab_is_named_by_the_first_line_of_its_command() -> Result<(), Box<dyn Error>> {
    let (rt, _) = scene(heredoc)?;
    let shown = texts(&rt.elements());
    let tab = shown
        .iter()
        .find(|t| t.starts_with("agent: "))
        .ok_or("no agent tab")?;
    assert_eq!(tab, "agent: git commit -F - <<'EOF'… · exit 0");
    Ok(())
}
