//! The agent points at a file: the console opens it at the line, in front when the user has
//! been idle, else as a tab that waits, and answers which it did.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use yi_types::acp::{AcpShowOutcome, AcpShowParams, AcpShowResult, AcpShowStrength};

use super::{App, Outbound};
use crate::model::SessionId;

/// The owner's rule for the model's strongest request: "Front only if you are idle there".
/// This long without a key or the mouse is idle.
const IDLE: Duration = Duration::from_secs(10);

/// The file and line an address names: `yi://file/<path>#L<n>`, `<path>:<n>` or a path, under
/// `cwd` when it is relative. None for an address of any other thing.
fn target(address: &str, cwd: &str) -> Option<(PathBuf, Option<usize>)> {
    let (path, line) = match address.strip_prefix("yi://") {
        Some(named) => {
            let file = named.strip_prefix("file/")?;
            let (path, line) = file.split_once("#L").unwrap_or((file, ""));
            (path, line.parse().ok())
        }
        None => match address.rsplit_once(':') {
            Some((path, line)) if !path.is_empty() && line.parse::<usize>().is_ok() => {
                (path, line.parse().ok())
            }
            _ => (address, None),
        },
    };
    Some((Path::new(cwd).join(path), line))
}

impl App {
    /// A key or the mouse came in: the user is at the console. A resize is the terminal's.
    pub(super) fn touched(&mut self, event: &ratatui::crossterm::event::Event) {
        if !matches!(event, ratatui::crossterm::event::Event::Resize(..)) {
            self.last_input = Some(self.clock.now());
        }
    }

    /// A method the worker sent that is no update: a show is carried out and answered, and a
    /// request the console does not know is refused, where it used to go unanswered.
    pub(super) fn asked(&mut self, outbound: &Outbound, method: &str, value: &Value) {
        let params = value.get("params").cloned();
        let show = (method == yi_types::acp::SHOW_METHOD)
            .then(|| serde_json::from_value::<AcpShowParams>(params?).ok())
            .flatten();
        match (value.get("id"), show) {
            (Some(id), Some(params)) => self.show(outbound, id, &params),
            (_, _) => {
                self.state.dropped_frames = self.state.dropped_frames.saturating_add(1);
                if let Some(id) = value.get("id") {
                    let error = json!({"code": -32601, "message": "Method not found"});
                    let _ = outbound.send(&json!({"jsonrpc": "2.0", "id": id, "error": error}));
                }
            }
        }
    }

    /// Answers a worker's `_yi/show`, having done what the answer says.
    fn show(&mut self, outbound: &Outbound, request_id: &Value, params: &AcpShowParams) {
        let result = self.shown(params);
        let response = json!({"jsonrpc": "2.0", "id": request_id, "result": result});
        if !outbound.send(&response) {
            self.note("send failed: show answer not delivered");
        }
        self.dirty = true;
    }

    /// Whether the showing tab has a pane on `session`: a file of a session the user is not
    /// looking at does not arrive in the one they are.
    fn on_screen(&self, session: &SessionId) -> bool {
        let shown = self.state.tab().map(|tab| tab.layout.pane_ids());
        let panes = &self.state.panes;
        let on = |id: &crate::layout::PaneId| panes.get(id).and_then(|pane| pane.session());
        shown.is_some_and(|ids| ids.iter().any(|id| on(id) == Some(session)))
    }

    fn shown(&mut self, params: &AcpShowParams) -> AcpShowResult {
        let refused = |note: &str| AcpShowResult {
            outcome: AcpShowOutcome::Refused,
            note: Some(note.to_owned()),
        };
        if !self.on_screen(&SessionId(params.session_id.clone())) {
            return refused("That session is not on screen.");
        }
        let Some((path, line)) = target(&params.address, &params.cwd) else {
            return refused("The console shows files; this address is not one.");
        };
        let Some(path) = path.to_str() else {
            return refused("The file's name is not valid UTF-8.");
        };
        let idle = self
            .last_input
            .is_none_or(|at| self.clock.since(at) >= IDLE);
        let front = params.strength == AcpShowStrength::Front && idle;
        if let Err(error) = self.show_file(path, params.cwd.clone(), line, !front) {
            return refused(&error);
        }
        if front {
            let outcome = AcpShowOutcome::Front;
            return AcpShowResult {
                outcome,
                note: None,
            };
        }
        let name = path.rsplit('/').next().unwrap_or(path);
        self.flash = Some((format!("the agent added {name} as a tab"), self.clock.now()));
        let note = match params.strength {
            AcpShowStrength::Front => Some("The user is busy; it waits as a tab."),
            AcpShowStrength::Peek => Some("The console has no peek; it waits as a tab."),
            AcpShowStrength::Tab | AcpShowStrength::Other(_) => None,
        };
        AcpShowResult {
            outcome: AcpShowOutcome::Tab,
            note: note.map(str::to_owned),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::time::Instant;

    use yi_tui::colors::{ColorTier, Theme};

    use super::*;
    use crate::model::PaneContent;

    type TestResult = Result<(), Box<dyn Error>>;

    /// This crate's own folder: real files, written by nobody in the test.
    const HERE: &str = env!("CARGO_MANIFEST_DIR");

    /// A console whose one pane is on session `s1`, its clock held still.
    fn console() -> Result<App, Box<dyn Error>> {
        let mut app = App::new(HERE.to_owned(), Theme::new(ColorTier::TrueColor, true));
        app.clock = yi_tui::motion::Clock::Fixed(Instant::now());
        app.state.zone = crate::model::Zone::Panes;
        let home = app.state.focused_pane_id().ok_or("a pane")?;
        let pane = app.state.panes.get_mut(&home).ok_or("the pane")?;
        pane.content = PaneContent::Notebook {
            session: Some(SessionId("s1".to_owned())),
            full: false,
            drawn: None,
        };
        Ok(app)
    }

    fn ask(address: &str, strength: AcpShowStrength) -> AcpShowParams {
        AcpShowParams {
            session_id: "s1".to_owned(),
            address: address.to_owned(),
            cwd: HERE.to_owned(),
            strength,
        }
    }

    /// The file each editor tab of the focused pane's panel shows, the one in front first,
    /// with its cursor's row.
    fn front(app: &App) -> Option<(String, usize)> {
        let panes = app.state.panes.values();
        let tabs = panes.filter_map(|pane| match &pane.content {
            PaneContent::Editor(tabs) => Some(tabs),
            _ => None,
        });
        let shown = tabs
            .map(|tabs| tabs.files().nth(tabs.shown_index()))
            .next()??;
        let name = shown.path.rsplit('/').next().unwrap_or_default().to_owned();
        Some((name, shown.text.cursor().0))
    }

    /// The owner's rule: "Front only if you are idle there". Idle, the file comes forward with
    /// the keys, on its line; ten seconds short of that it waits as a tab and says so.
    #[test]
    fn front_comes_forward_only_when_the_user_is_idle() -> TestResult {
        let mut app = console()?;
        let first = app.shown(&ask("Cargo.toml:2", AcpShowStrength::Front));
        assert_eq!((first.outcome, first.note), (AcpShowOutcome::Front, None));
        assert_eq!(front(&app), Some(("Cargo.toml".to_owned(), 1)), "on line 2");
        let editor = app.state.focused_pane_id();
        assert!(app.flash.is_none(), "in front needs no notice");
        app.last_input = Some(app.clock.now());
        app.clock.advance(IDLE - Duration::from_millis(1));
        let busy = app.shown(&ask("src/lib.rs:5", AcpShowStrength::Front));
        assert_eq!(busy.outcome, AcpShowOutcome::Tab);
        assert_eq!(
            busy.note.as_deref(),
            Some("The user is busy; it waits as a tab.")
        );
        assert_eq!(
            front(&app),
            Some(("Cargo.toml".to_owned(), 1)),
            "what was in front stays"
        );
        assert_eq!(app.state.focused_pane_id(), editor, "and the keys");
        let said = app.flash.as_ref().map(|(text, _)| text.as_str());
        assert_eq!(said, Some("the agent added lib.rs as a tab"));
        app.clock.advance(Duration::from_millis(1));
        let idle = app.shown(&ask("src/lib.rs:5", AcpShowStrength::Front));
        assert_eq!(idle.outcome, AcpShowOutcome::Front);
        assert_eq!(front(&app), Some(("lib.rs".to_owned(), 4)));
        Ok(())
    }

    /// `tab` and `peek` never interrupt, idle or not: with no editor open the file gets a
    /// split of its own and the keys stay where they were.
    #[test]
    fn a_tab_or_a_peek_never_takes_the_keys() -> TestResult {
        for strength in [AcpShowStrength::Tab, AcpShowStrength::Peek] {
            let mut app = console()?;
            let (home, before) = (app.state.focused_pane_id(), app.state.panes.len());
            let answer = app.shown(&ask("Cargo.toml", strength.clone()));
            assert_eq!(answer.outcome, AcpShowOutcome::Tab, "{strength:?}");
            assert_eq!(app.state.focused_pane_id(), home, "{strength:?}");
            assert_eq!(app.state.panes.len(), before + 1, "a split of its own");
            assert_eq!(front(&app), Some(("Cargo.toml".to_owned(), 0)));
            let peeked = strength == AcpShowStrength::Peek;
            assert_eq!(answer.note.is_some(), peeked, "a peek says it became a tab");
        }
        Ok(())
    }

    /// What the console cannot show it refuses, saying why, and opens nothing.
    #[test]
    fn what_cannot_be_shown_is_refused_and_says_why() -> TestResult {
        let mut app = console()?;
        let panes = app.state.panes.len();
        let agent = app.shown(&ask("yi://agent/a/one", AcpShowStrength::Tab));
        assert_eq!(agent.outcome, AcpShowOutcome::Refused);
        assert!(agent.note.is_some_and(|note| note.contains("shows files")));
        let missing = app.shown(&ask("no/such/file.rs:3", AcpShowStrength::Front));
        assert_eq!(missing.outcome, AcpShowOutcome::Refused);
        assert!(
            missing
                .note
                .is_some_and(|note| note.contains("no/such/file.rs"))
        );
        let mut other = ask("Cargo.toml", AcpShowStrength::Front);
        other.session_id = "s2".to_owned();
        let hidden = app.shown(&other);
        assert_eq!(hidden.outcome, AcpShowOutcome::Refused);
        assert_eq!(
            hidden.note.as_deref(),
            Some("That session is not on screen.")
        );
        assert_eq!(app.state.panes.len(), panes, "nothing opened");
        Ok(())
    }

    /// The three ways a model names a file, and a path that is already whole.
    #[test]
    fn an_address_is_a_path_a_path_and_line_or_a_file_address() {
        let at = |address| target(address, "/repo");
        let file = |path: &str, line| Some((PathBuf::from(path), line));
        assert_eq!(at("src/a.rs"), file("/repo/src/a.rs", None));
        assert_eq!(at("src/a.rs:7"), file("/repo/src/a.rs", Some(7)));
        assert_eq!(at("yi://file/src/a.rs#L7"), file("/repo/src/a.rs", Some(7)));
        assert_eq!(at("yi://file/src/a.rs"), file("/repo/src/a.rs", None));
        assert_eq!(at("/abs/b.rs:2"), file("/abs/b.rs", Some(2)));
        assert_eq!(at("notes: draft.md"), file("/repo/notes: draft.md", None));
        assert_eq!(at("yi://thread/t1"), None);
    }
}
