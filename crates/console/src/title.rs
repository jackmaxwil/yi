use crate::App;
use crate::model::SessionStatus::{DoneUnseen, Idle, Unknown};

/// The scenes the title may draw, by weight, and the seed that picks one.
pub struct TitleOptions {
    pub weights: Vec<(yi_tui::title::Scene, u32)>,
    pub seed: u64,
}

/// A configured weight named a scene that does not exist.
#[derive(Debug, PartialEq, Eq)]
pub struct UnknownScene(pub String);

impl std::fmt::Display for UnknownScene {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let known: Vec<&str> = yi_tui::title::Scene::ALL.iter().map(|(n, _)| *n).collect();
        write!(
            f,
            "console.titleWeights.{}: no such scene (the scenes are {})",
            self.0,
            known.join(", ")
        )
    }
}

impl std::error::Error for UnknownScene {}

impl TitleOptions {
    /// `None` weights are the defaults; a name that is no scene is refused with the key named.
    pub fn new(weights: Option<Vec<(String, u32)>>, seed: u64) -> Result<Self, UnknownScene> {
        let weights = match weights {
            None => yi_tui::title::DEFAULT_WEIGHTS.to_vec(),
            Some(named) => named
                .into_iter()
                .map(|(name, weight)| {
                    yi_tui::title::Scene::named(&name)
                        .map(|scene| (scene, weight))
                        .ok_or(UnknownScene(name))
                })
                .collect::<Result<_, _>>()?,
        };
        Ok(Self { weights, seed })
    }
}

fn focused_status(app: &App) -> Option<crate::model::SessionStatus> {
    app.state
        .focused_pane()
        .and_then(|pane| pane.session())
        .and_then(|id| app.state.sessions.get(id))
        .map(|row| row.status)
}

fn format_window_title(
    label: Option<&str>,
    status: Option<crate::model::SessionStatus>,
    elapsed: std::time::Duration,
) -> String {
    let Some(label) = label else {
        return "yi".to_owned();
    };
    let mark = match status {
        Some(crate::model::SessionStatus::Working) => {
            Some(yi_tui::motion::tab_title_pulse(elapsed))
        }
        Some(crate::model::SessionStatus::Blocked) => Some("!"),
        Some(
            crate::model::SessionStatus::DoneUnseen
            | crate::model::SessionStatus::Idle
            | crate::model::SessionStatus::Unknown,
        )
        | None => None,
    };
    match mark {
        Some(mark) => format!("{mark} yi · {label}"),
        None => format!("yi · {label}"),
    }
}

pub(crate) fn window_title(app: &App) -> String {
    let Some(id) = app.state.focused_session() else {
        return "yi".to_owned();
    };
    let Some(row) = app.state.sessions.get(&id) else {
        return "yi".to_owned();
    };
    let elapsed = app
        .state
        .window_title_pulse_start
        .map_or(std::time::Duration::ZERO, |start| app.clock.since(start));
    format_window_title(Some(&row.label()), Some(row.status), elapsed)
}

pub(crate) fn title_wake(app: &App) -> Option<std::time::Duration> {
    (focused_status(app) == Some(crate::model::SessionStatus::Working))
        .then_some(app.state.window_title_pulse_start?)
        .map(|start| {
            let period = std::time::Duration::from_millis(yi_tui::motion::PULSE_PERIOD_MS as u64);
            let elapsed = app.clock.since(start);
            period
                - std::time::Duration::from_nanos((elapsed.as_nanos() % period.as_nanos()) as u64)
        })
}

/// Invariant: only the focused pane's session status drives the terminal title.
pub(crate) fn title_status_tick(app: &mut App) {
    let now = app.clock.now();
    let status = focused_status(app);
    if status == Some(crate::model::SessionStatus::Working) {
        app.state.window_title_pulse_start.get_or_insert(now);
    } else {
        app.state.window_title_pulse_start = None;
    }
    let title_matches = match status {
        Some(crate::model::SessionStatus::Working) => {
            let elapsed = app
                .state
                .window_title_pulse_start
                .map_or(std::time::Duration::ZERO, |start| app.clock.since(start));
            app.window_title
                .strip_prefix(yi_tui::motion::tab_title_pulse(elapsed))
                .is_some_and(|suffix| suffix.starts_with(" yi · "))
        }
        Some(crate::model::SessionStatus::Blocked) => app
            .window_title
            .strip_prefix("!")
            .is_some_and(|suffix| suffix.starts_with(" yi · ")),
        Some(DoneUnseen | Idle | Unknown) => app.window_title.starts_with("yi · "),
        None => app.window_title == "yi",
    };
    if !app.window_title.is_empty() && !title_matches {
        app.dirty = true;
    }
}

/// The title plays from console start for `OVER`, unless the session in front has been used.
pub(crate) fn title_playing(app: &App) -> bool {
    use crate::model::PaneContent;
    let idle = app
        .state
        .focused_pane_id()
        .and_then(|id| app.state.panes.get(&id))
        .is_none_or(|pane| match &pane.content {
            PaneContent::Session {
                chat: Some(chat), ..
            } => chat.app.title_idle(),
            _ => true,
        });
    idle && app.state.title.is_some()
        && app.clock.since(app.state.title_start) < yi_tui::title::OVER
}

/// No event announces a title's motion, so it asks for one frame per interval (every loop turn
/// drew 58 a second) and one last frame when it ends, which wipes it.
pub(crate) fn title_tick(app: &mut App) {
    let showing = title_playing(app);
    let now = app.clock.now();
    let due = app
        .state
        .title_frame
        .is_none_or(|at| now.saturating_duration_since(at) >= yi_tui::title::FRAME);
    if showing && due {
        app.dirty = true;
        app.state.title_frame = Some(now);
    } else if !showing && app.state.title_frame.take().is_some() {
        app.dirty = true;
    }
    title_status_tick(app);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::MouseKind;
    use crate::client;
    use crate::mouse_event;
    use ratatui::crossterm::event::Event as CtEvent;
    use std::time::{Duration, Instant};
    use yi_tui::colors::{ColorTier, Theme};

    fn app_with_session(
        status: crate::model::SessionStatus,
    ) -> Result<App, Box<dyn std::error::Error>> {
        let mut app = App::new("/repo".to_owned(), Theme::new(ColorTier::TrueColor, true));
        let id = crate::model::SessionId("s-title".to_owned());
        app.state.sessions.insert(
            id.clone(),
            crate::model::SessionRow {
                id: id.clone(),
                root: "/repo".to_owned(),
                status,
                attached: true,
                name: Some("focused".to_owned()),
                created_ms: 0,
                last_ms: 0,
                pinned: None,
            },
        );
        let pane = app.state.focused_pane_id().ok_or("initial pane")?;
        app.state
            .panes
            .get_mut(&pane)
            .ok_or("initial pane")?
            .content = crate::model::PaneContent::Session {
            session: Some(id),
            chat: None,
        };
        app.clock = yi_tui::motion::Clock::Fixed(Instant::now());
        Ok(app)
    }

    #[test]
    fn title_format_uses_focused_status_without_changing_session_label() {
        use crate::model::SessionStatus;
        let plain = "yi · focused";
        assert_eq!(format_window_title(None, None, Duration::ZERO), "yi");
        assert_eq!(
            format_window_title(
                Some("focused"),
                Some(SessionStatus::Working),
                Duration::ZERO
            ),
            "◌ yi · focused"
        );
        assert_eq!(
            format_window_title(
                Some("focused"),
                Some(SessionStatus::Blocked),
                Duration::ZERO
            ),
            "! yi · focused"
        );
        for status in [
            SessionStatus::Idle,
            SessionStatus::Unknown,
            SessionStatus::DoneUnseen,
        ] {
            assert_eq!(
                format_window_title(Some("focused"), Some(status), Duration::ZERO),
                plain
            );
        }
    }

    #[test]
    fn working_title_pulse_changes_at_shared_period_boundaries() {
        let markers = [
            (0, "◌"),
            (249, "◌"),
            (250, "◍"),
            (500, "◉"),
            (750, "◍"),
            (1_000, "◌"),
        ];
        for (elapsed, expected) in markers {
            assert_eq!(
                format_window_title(
                    Some("focused"),
                    Some(crate::model::SessionStatus::Working),
                    Duration::from_millis(elapsed),
                ),
                format!("{expected} yi · focused")
            );
        }
    }

    #[test]
    fn focused_status_transitions_dirty_only_when_title_changes()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut idle = App::new("/repo".to_owned(), Theme::new(ColorTier::TrueColor, true));
        idle.window_title = window_title(&idle);
        idle.dirty = false;
        title_status_tick(&mut idle);
        assert!(!idle.dirty, "no session keeps the default title idle");

        let mut app = app_with_session(crate::model::SessionStatus::Working)?;
        title_status_tick(&mut app);
        assert_eq!(title_wake(&app), Some(Duration::from_millis(250)));
        app.window_title = window_title(&app);
        app.dirty = false;

        app.clock.advance(Duration::from_millis(249));
        title_tick(&mut app);
        assert_eq!(title_wake(&app), Some(Duration::from_millis(1)));
        assert!(!app.dirty, "the pulse does not redraw before its boundary");
        app.clock.advance(Duration::from_millis(1));
        title_tick(&mut app);
        assert_eq!(title_wake(&app), Some(Duration::from_millis(250)));
        assert!(app.dirty, "the changed marker redraws the title");
        assert_eq!(window_title(&app), "◍ yi · focused");
        crate::set_window_title(&mut app);
        assert_eq!(
            app.osc_out.last().map(String::as_str),
            Some("\x1b]2;◍ yi · focused\x07")
        );
        let written = app.osc_out.len();
        crate::set_window_title(&mut app);
        assert_eq!(app.osc_out.len(), written, "unchanged title is not resent");
        app.dirty = false;

        let id = app.state.focused_session().ok_or("focused session")?;
        app.state.sessions.get_mut(&id).ok_or("session row")?.status =
            crate::model::SessionStatus::Blocked;
        title_tick(&mut app);
        assert!(app.dirty, "a focused status transition redraws the title");
        assert_eq!(window_title(&app), "! yi · focused");
        crate::set_window_title(&mut app);
        assert_eq!(
            app.osc_out.last().map(String::as_str),
            Some("\x1b]2;! yi · focused\x07")
        );
        assert_eq!(title_wake(&app), None, "blocked sessions do not pulse");
        Ok(())
    }

    /// Incident: asking for a frame on every loop turn drew the title at 58 fps; it now asks once
    /// a frame interval has passed on the app's own clock.
    #[test]
    fn a_title_on_screen_asks_for_one_frame_per_interval() {
        let theme = Theme::new(ColorTier::TrueColor, true);
        let mut app = App::new("/repo".to_owned(), theme);
        app.state.title = Some(yi_tui::title::Scene::Echo);
        app.clock = yi_tui::motion::Clock::Fixed(Instant::now());
        let asks = |app: &mut App| {
            app.dirty = false;
            title_tick(app);
            app.dirty
        };
        assert!(asks(&mut app), "the first frame is due at once");
        assert!(!asks(&mut app), "the same instant asks for nothing");
        app.clock.advance(yi_tui::title::FRAME / 2);
        assert!(!asks(&mut app), "half an interval is not a frame");
        app.clock.advance(yi_tui::title::FRAME / 2);
        assert!(asks(&mut app), "a whole interval is");
        app.clock.advance(yi_tui::title::OVER);
        assert!(asks(&mut app), "one last frame wipes it when it ends");
        assert!(!asks(&mut app), "and then nothing is asked for");
    }

    /// The key that skips the title only skips it: a ctrl+b under the cover would arm the prefix on
    /// a screen nobody could see. Once it is over, the same key arms it at once.
    #[test]
    fn the_key_that_skips_the_title_is_not_also_pressed() {
        let theme = Theme::new(ColorTier::TrueColor, true);
        let mut app = App::new("/repo".to_owned(), theme);
        app.state.title = Some(yi_tui::title::Scene::Echo);
        app.clock = yi_tui::motion::Clock::Fixed(Instant::now());
        app.state.title_start = app.clock.now();
        let (events, _received) = client::events();
        let socket = std::env::temp_dir().join("yi-title-skip-no-daemon.sock");
        let (outbound, _threads) = client::spawn(socket, events);
        let prefix = ratatui::crossterm::event::KeyEvent::new(
            ratatui::crossterm::event::KeyCode::Char('b'),
            ratatui::crossterm::event::KeyModifiers::CONTROL,
        );
        assert!(title_playing(&app));
        app.handle_event(&outbound, CtEvent::Key(prefix));
        assert!(!title_playing(&app), "the key ended the title");
        assert!(
            matches!(app.state.mode, crate::model::Mode::Normal),
            "and did nothing else"
        );
        app.handle_event(&outbound, CtEvent::Key(prefix));
        assert!(
            matches!(app.state.mode, crate::model::Mode::Prefix),
            "the next one is an ordinary key"
        );
        outbound.shutdown();
    }

    /// Pointer motion under the cover is dropped and, once a press has ended it, the same motion
    /// lands: a hover on a screen nobody can see must not light a hint button.
    #[test]
    fn the_pointer_is_dropped_under_the_title_and_counts_after_it() {
        let theme = Theme::new(ColorTier::TrueColor, true);
        let mut app = App::new("/repo".to_owned(), theme);
        app.state.title = Some(yi_tui::title::Scene::Echo);
        app.clock = yi_tui::motion::Clock::Fixed(Instant::now());
        app.state.title_start = app.clock.now();
        let (events, _received) = client::events();
        let socket = std::env::temp_dir().join("yi-title-pointer-no-daemon.sock");
        let (outbound, _threads) = client::spawn(socket, events);
        let at = |kind| CtEvent::Mouse(mouse_event(kind, 3, 4));
        app.handle_event(&outbound, at(MouseKind::Moved));
        assert_eq!(app.pointer, None, "a hover under the cover is dropped");
        assert!(title_playing(&app), "and does not end it");
        app.handle_event(&outbound, at(MouseKind::Down));
        assert!(!title_playing(&app), "a press ends it");
        app.handle_event(&outbound, at(MouseKind::Moved));
        assert_eq!(app.pointer, Some((3, 4)), "afterwards the pointer counts");
        outbound.shutdown();
    }

    #[test]
    fn a_scene_that_does_not_exist_is_refused_with_the_key_named() {
        let refused = TitleOptions::new(
            Some(vec![("kali".to_owned(), 1), ("nope".to_owned(), 2)]),
            0,
        );
        let said = refused.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(
            said.starts_with("console.titleWeights.nope: no such scene"),
            "{said}"
        );
        assert!(said.contains("kali"), "it lists the scenes: {said}");
        let kept = TitleOptions::new(Some(vec![("echo".to_owned(), 3)]), 9);
        assert_eq!(
            kept.map(|t| t.weights),
            Ok(vec![(yi_tui::title::Scene::Echo, 3)])
        );
    }
}
