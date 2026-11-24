//! Start-up: the user's own files, the inbox, the wire client, the effect sink and the
//! sources a window needs, and what quitting stops and writes.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rune::AppHandle;
use yd_core::command::table::{Route, Run};
use yd_core::feed::{FeedEvent, FeedStatus, REMOTE, REPORT, SourceCmd, TERMINAL, WEB};
use yd_core::findings::triage::{self, Triage, TriageMsg};
use yd_core::ids::{FeedId, SettingKey};
use yd_core::inbox::{Sender, inbox};
use yd_core::msg::{Effect, Msg};
use yd_core::pane::PaneKind;
use yd_core::reduce::step::commands::CANCEL_ASKS;
use yd_core::settings::schema::{Control, Setting, SettingValue, Store};
use yd_core::wire::client::{Client, Clock, Config};
use yd_core::wire::event::{Down, WireEvent};
use yd_panes::settings::pane::{bind, file};
use yd_sources::browser::{self, Chrome};
use yd_sources::machines;
use yd_sources::pty::{self, Screen, Spawn};
use yd_sources::{registry, server};
use yd_ui::disk::Disk;
use yd_ui::term::view::state::Shells;
use yd_ui::theme;
use yd_ui::win::{Hub, Win, dispatch};

mod asks;
mod folder;

/// The flag that points the app at another server; a journey's private one, say.
pub const SOCKET_FLAG: &str = "--socket";
pub const SOCKET_KEY: &str = "app.socket";
/// Where Yi's daemon listens unless the setting or the flag says otherwise.
pub const SOCKET_DEFAULT: &str = "~/.yi/daemon.sock";
pub const SOCKET: Setting = Setting {
    key: SettingKey::fixed(SOCKET_KEY),
    group: std::borrow::Cow::Borrowed("Server"),
    name: std::borrow::Cow::Borrowed("Socket"),
    description: std::borrow::Cow::Borrowed("Where yi serve listens."),
    control: Control::Text,
    store: Store::App,
    default: Some(SettingValue::Text(std::borrow::Cow::Borrowed(
        SOCKET_DEFAULT,
    ))),
    value: None,
};
const POLL: Duration = Duration::from_secs(5);
/// How long quitting waits for the wire's threads; a handshake in flight can take longer.
const GRACE: Duration = Duration::from_secs(2);
/// How long quitting waits for the sources: a shell is hung up, then killed after two seconds.
const SOURCES_GRACE: Duration = Duration::from_secs(3);
/// Asks queued for one source before more are refused: a source that falls this far behind
/// is stuck, and the UI thread never waits on it.
const ASKS: usize = 256;

/// What the app started: the client, each running source's ask channel, and messages the
/// sink made that must be stepped after it returns.
pub struct Running {
    client: Option<Client>,
    sources: HashMap<FeedId, SyncSender<SourceCmd>>,
    /// Each started source's thread, so quitting can wait for it to stop its child.
    threads: Vec<JoinHandle<Option<()>>>,
    pending: Vec<Msg>,
    inbox: Sender,
    clock: Clock,
    hub: Weak<RefCell<Hub>>,
}

/// Where the hub keeps the running app, so the sink, `poll` and quitting share it.
#[derive(Default)]
pub struct Started(Option<Rc<RefCell<Running>>>);

/// The app's folder name under the user's config folder, shared with the windows' places.
const APP: &str = "yi-desktop";

/// Where the user's own files are: the settings file, the themes folder, the triage file and
/// the folder of plugin panes.
pub struct Files {
    pub settings: file::Disk,
    pub themes: PathBuf,
    pub triage: PathBuf,
    pub panes: PathBuf,
    /// The approvals that wait, as Yi asked them: Yi answers only the id it asked under.
    pub asks: PathBuf,
}

impl Files {
    /// The native app's: under its config folder. None when the Mac names no home.
    pub fn app() -> Option<Self> {
        let folder = rune::dirs::AppDirs::new(APP)?.config().to_path_buf();
        Some(Self {
            settings: file::Disk::app(),
            ..Self::under(&folder)
        })
    }

    /// All of them in `folder`: what a test keeps under the lane's `target/`.
    pub fn under(folder: &Path) -> Self {
        Self {
            settings: file::Disk::at(folder.join("settings.json")),
            themes: folder.join("themes"),
            triage: folder.join("findings.json"),
            panes: folder.join("panes"),
            asks: folder.join("asks.jsonl"),
        }
    }
}

/// The triage file, and the revision of the triage last handed to be written. No path
/// means nothing is saved: no file was asked for, or the one there did not read.
#[derive(Default)]
struct Kept {
    triage: Option<PathBuf>,
    revision: u64,
    asks: Option<PathBuf>,
    /// The asks as last handed to be written.
    written: String,
}

/// Reads the user's files into the hub and starts the worker that writes them: the themes,
/// the settings and the theme they name, the folder of plugin panes, then the triage.
pub fn files(hub: &mut Hub, files: Files) {
    theme::files::open(hub, files.themes);
    bind::list_themes(hub);
    file::restore(hub, files.settings);
    bind::theme(hub);
    yd_panes::script_pane::pane::open(hub, files.panes);
    let read = triage::open(&files.triage);
    if !read.may_save {
        let path = files.triage.display();
        let _ = writeln!(
            std::io::stderr(),
            "yi-desktop: {path} did not read; triage is not saved over it"
        );
    }
    let revision = read.triage.revision;
    yd_core::msg::step(&mut hub.model, Msg::Triage(TriageMsg::Loaded(read.triage)));
    asks::restore(hub, &files.asks);
    let written = hub.model.wire.kept();
    if let Some(kept) = hub.panes.get_mut::<Kept>(None) {
        *kept = Kept {
            triage: read.may_save.then_some(files.triage),
            revision,
            asks: Some(files.asks),
            written,
        };
    }
    match (Disk::start(), hub.panes.get_mut::<Disk>(None)) {
        (Ok(started), Some(disk)) => *disk = started,
        (Err(why), _) => {
            let _ = writeln!(
                std::io::stderr(),
                "yi-desktop: no thread for file writes: {why}"
            );
        }
        (Ok(_), None) => {}
    }
}

/// The triage to write and where, when it changed since it was last handed over.
fn unsaved(hub: &mut Hub) -> Option<(PathBuf, Triage)> {
    let revision = hub.model.findings.triage.revision;
    let kept = hub.panes.get_mut::<Kept>(None)?;
    let path = kept.triage.clone().filter(|_| kept.revision != revision)?;
    kept.revision = revision;
    Some((path, hub.model.findings.triage.clone()))
}

fn save(path: &Path, triage: &Triage) {
    if let Err(why) = triage::save(path, triage) {
        let _ = writeln!(std::io::stderr(), "yi-desktop: {why}");
    }
}

/// Hands the triage to the worker when its revision has moved. The poll calls it; nothing
/// a window draws changes.
pub fn keep(win: &mut Win) {
    let Ok(mut hub) = win.hub.try_borrow_mut() else {
        return;
    };
    if let Some((path, asks)) = asks::unsaved(&mut hub) {
        match hub.panes.get::<Disk>(None) {
            Some(disk) => disk.run(move || asks::save(&path, &asks)),
            None => asks::save(&path, &asks),
        }
    }
    let Some((path, triage)) = unsaved(&mut hub) else {
        return;
    };
    match hub.panes.get::<Disk>(None) {
        Some(disk) => disk.run(move || save(&path, &triage)),
        None => save(&path, &triage),
    }
}

/// Lets the worker finish, then writes what no poll handed over: quitting waits for both.
fn settle_files(win: &mut Win) {
    let Ok(mut hub) = win.hub.try_borrow_mut() else {
        return;
    };
    if let Some(disk) = hub.panes.get_mut::<Disk>(None) {
        disk.finish();
    }
    theme::files::close(&mut hub);
    if let Some((path, triage)) = unsaved(&mut hub) {
        save(&path, &triage);
    }
    if let Some((path, asks)) = asks::unsaved(&mut hub) {
        asks::save(&path, &asks);
    }
}

/// The socket from `--socket <path>`, else the setting, with `~/` taken as `home`.
pub fn socket(args: &[String], setting: Option<&str>, home: &Path) -> PathBuf {
    let flag = args
        .iter()
        .position(|arg| arg == SOCKET_FLAG)
        .and_then(|at| args.get(at.saturating_add(1)));
    let path = flag.map_or(setting.unwrap_or(SOCKET_DEFAULT), String::as_str);
    path.strip_prefix("~/")
        .map_or_else(|| PathBuf::from(path), |rest| home.join(rest))
}

/// Whether [`start`] will connect: the native app, or a run told its socket.
pub fn connects(native: bool) -> bool {
    native || rune::extra_args().iter().any(|arg| arg == SOCKET_FLAG)
}

/// Wires the window to the world. Only the native app may reach the default socket; any
/// other run connects only when told one, so a scene or a test never reaches the user's own.
pub fn start(win: &mut Win, app: AppHandle<Win>, native: bool) {
    let args = rune::extra_args();
    if !connects(native) {
        return;
    }
    let Ok(mut hub) = win.hub.try_borrow_mut() else {
        return;
    };
    if hub
        .panes
        .get::<Started>(None)
        .is_some_and(|s| s.0.is_some())
    {
        return;
    }
    let setting = match hub.model.settings.value(SOCKET_KEY) {
        Some(SettingValue::Text(path)) => Some(path.to_string()),
        _ => None,
    };
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let socket = socket(args, setting.as_deref(), &home);
    let wake = app.clone();
    let (sender, inbox) = inbox(move || wake.wake());
    hub.set_inbox(inbox);
    let running = Rc::new(RefCell::new(Running::new(
        Rc::downgrade(&win.hub),
        sender.clone(),
        server::clock(),
    )));
    let config = Config {
        socket,
        poll: Some(POLL),
        start: Some(server::starter(
            server::installed_yi().unwrap_or_else(|missing| missing.fallback),
        )),
    };
    if let Ok(mut run) = running.try_borrow_mut() {
        match Client::spawn(config, sender, run.clock.clone()) {
            Ok(client) => run.client = Some(client),
            Err(error) => {
                let at = (run.clock)();
                let why = Down::StartFailed(error.to_string());
                run.pending.push(Msg::Wire(WireEvent::Down { at, why }));
            }
        }
    }
    let sink = Rc::clone(&running);
    hub.set_sink(move |effect| {
        if let Ok(mut run) = sink.try_borrow_mut() {
            run.carry(effect);
        }
    });
    if let Some(started) = hub.panes.get_mut::<Started>(None) {
        started.0 = Some(running);
    }
}

/// Steps what the sink could not step itself; true when there was any.
pub fn settle(win: &mut Win) -> bool {
    let pending = running(win).map(|run| {
        run.try_borrow_mut()
            .map(|mut run| std::mem::take(&mut run.pending))
            .unwrap_or_default()
    });
    let pending = pending.unwrap_or_default();
    let any = !pending.is_empty();
    pending.into_iter().for_each(|msg| dispatch(win, msg));
    any
}

/// Answers every waiting ask `cancelled`, writes the user's files, then stops the client and
/// every source the app started. The daemon is left running: other clients share it.
pub fn quit(win: &mut Win) {
    dispatch(win, Msg::Command(Route::Wire, Run::new(CANCEL_ASKS)));
    settle_files(win);
    let Some(running) = running(win) else { return };
    if let Ok(mut run) = running.try_borrow_mut() {
        // Dropping an ask channel ends its source: a shell is hung up, a watcher stops.
        run.sources.clear();
        let threads = std::mem::take(&mut run.threads);
        let asked = Instant::now();
        if let Some(client) = run.client.take() {
            client.close(GRACE).ok();
        }
        join_within(threads, SOURCES_GRACE.saturating_sub(asked.elapsed()));
    }
}

/// Waits up to `grace` for the source threads to end; how many were still running.
fn join_within(mut threads: Vec<JoinHandle<Option<()>>>, grace: Duration) -> usize {
    let deadline = Instant::now().checked_add(grace);
    loop {
        let (done, running): (Vec<_>, Vec<_>) =
            threads.into_iter().partition(JoinHandle::is_finished);
        done.into_iter().for_each(|thread| drop(thread.join()));
        threads = running;
        if threads.is_empty() || deadline.is_none_or(|at| Instant::now() >= at) {
            return threads.len();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn running(win: &Win) -> Option<Rc<RefCell<Running>>> {
    let hub = win.hub.try_borrow().ok()?;
    hub.panes.get::<Started>(None)?.0.clone()
}

impl Running {
    pub fn new(hub: Weak<RefCell<Hub>>, inbox: Sender, clock: Clock) -> Self {
        Self {
            client: None,
            sources: HashMap::new(),
            threads: Vec::new(),
            pending: Vec::new(),
            inbox,
            clock,
            hub,
        }
    }

    /// The sink: a request to the client, an ask to its source, a window effect to the
    /// window layer, a notice to the system.
    pub fn carry(&mut self, effect: Effect) {
        match effect {
            Effect::Wire(request) => {
                let id = request.id.clone();
                let sent = self.client.as_ref().map(|client| client.send(request));
                if let (Some(id), Some(Err(_)) | None) = (id, sent) {
                    let at = (self.clock)();
                    self.pending.push(Msg::Wire(WireEvent::Unsent { at, id }));
                }
            }
            Effect::Source(feed, ask) => self.ask(feed, ask),
            Effect::Window(_) => {
                if let Some(hub) = self.hub.upgrade() {
                    crate::windows::hub::reconcile(&hub);
                }
            }
            Effect::Notify(notice) => crate::notify::post(&notice),
        }
    }

    /// Hands `ask` to the source of `feed`, starting it on its first ask.
    fn ask(&mut self, feed: FeedId, ask: SourceCmd) {
        let close = ask == SourceCmd::Close;
        let sent = match self.sources.get(&feed) {
            Some(tx) => tx.try_send(ask),
            None => Err(TrySendError::Disconnected(ask)),
        };
        let ask = match sent {
            Ok(()) => None,
            Err(TrySendError::Full(_)) => {
                self.fail(feed, format!("{} is not keeping up", feed.source));
                None
            }
            Err(TrySendError::Disconnected(ask)) => Some(ask),
        };
        if let Some(ask) = ask.filter(|_| !close) {
            match self.spawn(feed) {
                Ok((tx, thread)) => {
                    tx.try_send(ask).ok();
                    self.threads.retain(|thread| !thread.is_finished());
                    self.threads.push(thread);
                    self.sources.insert(feed, tx);
                }
                Err(why) => self.fail(feed, why),
            }
        }
        if close {
            self.sources.remove(&feed);
        }
    }

    fn fail(&mut self, feed: FeedId, why: String) {
        let status = FeedEvent::Status(FeedStatus::Error(why));
        self.pending.push(Msg::Feed(feed, status));
    }

    fn spawn(
        &self,
        feed: FeedId,
    ) -> Result<(SyncSender<SourceCmd>, JoinHandle<Option<()>>), String> {
        let (tx, rx) = sync_channel(ASKS);
        let out = self.inbox.feed(feed);
        let name = format!("yd-{}-{}", feed.source, feed.instance);
        let builder = std::thread::Builder::new().name(name);
        let spawned = if feed.source == TERMINAL.source {
            let (screen, spawn) = self.shell(feed).ok_or("no terminal tab holds this shell")?;
            builder.spawn(move || pty::serve(&screen, &spawn, out, rx).ok())
        } else if feed.source == WEB.source {
            let screen = self.page(feed).ok_or("no pane shows this page")?;
            let chrome = Chrome::installed();
            let launch = move |url: &str, size, scale| chrome.launch(url, size, scale);
            // The frame is decoded here, on the source's thread, into the pane's own picture.
            let show = move |png: Vec<u8>| screen.push_png(&png).map_err(|why| why.to_string());
            builder.spawn(move || browser::serve(launch, show, out, rx).ok())
        } else if feed.source == REMOTE.source {
            let screen = self.page(feed).ok_or("no pane shows this screen")?;
            let show = move |w, h, rgba| screen.push(w, h, rgba).map_err(|why| why.to_string());
            builder.spawn(move || machines::remote_screen(out, rx, &show).ok())
        } else {
            let def = registry::find(feed).ok_or("no source fills this feed")?;
            builder.spawn(move || (def.run)(out, rx).ok())
        };
        let thread = spawned.map_err(|error| error.to_string())?;
        Ok((tx, thread))
    }

    /// The screen a page is drawn to: the web pane's for the first page of the browser
    /// source, the report pane's for its second. The first pane of the kind has the page.
    fn page(&self, feed: FeedId) -> Option<yd_ui::screen::surface::Screen> {
        use yd_panes::{remote, report, web};
        let hub = self.hub.upgrade()?;
        let hub = hub.try_borrow().ok()?;
        let kind = match feed {
            WEB => PaneKind::Web,
            REPORT => PaneKind::Report,
            REMOTE => PaneKind::Remote,
            _ => return None,
        };
        // ponytail: one page a kind; a page a pane when two web panes are wanted at once.
        let panes = hub.model.grid.panes.iter();
        let ids = panes.filter(|pane| pane.kind == kind).map(|pane| pane.id);
        ids.chain([yd_ui::pane::SCENE_PANE]).find_map(|pane| {
            let pane = Some(pane);
            match kind {
                PaneKind::Report => {
                    Some(hub.panes.get::<report::pane::State>(pane)?.screen.clone())
                }
                PaneKind::Remote => {
                    Some(hub.panes.get::<remote::pane::State>(pane)?.screen.clone())
                }
                _ => Some(hub.panes.get::<web::pane::State>(pane)?.screen.clone()),
            }
        })
    }

    /// The screen a terminal tab draws and how to start its shell: a terminal ask names
    /// only its feed instance, so the tab is found among the terminal panes' state.
    pub fn shell(&self, feed: FeedId) -> Option<(Screen, Spawn)> {
        let shared = self.hub.upgrade()?;
        let (pane, screen, cwd, project) = {
            let hub = shared.try_borrow().ok()?;
            let panes = hub.model.grid.panes.iter();
            let terminals = panes.filter(|pane| pane.kind == PaneKind::Terminal);
            let ids = terminals
                .map(|pane| pane.id)
                .chain([yd_ui::pane::SCENE_PANE]);
            ids.into_iter().find_map(|pane| {
                let shell = hub.panes.get::<Shells>(Some(pane))?.shell(feed)?;
                let surface = &shell.surface;
                let screen = Screen {
                    parser: surface.parser.clone(),
                    lines: surface.lines.clone(),
                };
                let cwd = match shell.cwd.as_str() {
                    "" => folder::of_pane(&hub, pane),
                    cwd => Some(cwd.to_owned()),
                };
                let project = yd_ui::shown::of_pane(&hub, pane).map(|t| t.cwd.clone());
                Some((pane, screen, cwd.unwrap_or_default(), project))
            })?
        };
        let cwd = folder::start_in(&screen, cwd, project);
        // The tab keeps where its shell started: a thread that works elsewhere hangs it up.
        if let Ok(mut hub) = shared.try_borrow_mut()
            && let Some(shell) = hub
                .panes
                .get_mut::<Shells>(Some(pane))
                .and_then(|shells| shells.shell_mut(feed))
        {
            shell.cwd.clone_from(&cwd);
        }
        Some((screen, Spawn::installed(PathBuf::from(cwd))))
    }
}

/// The app's own log, the file the Logs pane tails; none when the Mac names no home.
pub fn app_log() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    Some(yd_sources::logs::places(&[], &home).app)
}

/// Says a panic in one line on stderr and at the end of `log`, then runs the default hook.
pub fn on_panic(log: Option<PathBuf>) {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let at = info.location().map(ToString::to_string).unwrap_or_default();
        let said = info
            .payload_as_str()
            .unwrap_or("no message")
            .replace('\n', " ");
        // Incident: std::thread::current panics once a thread's storage is gone, and a panic in
        // the hook aborts, so the thread is named only by the default hook that runs after.
        let line = format!("{APP}: panicked at {at}: {said}");
        let _ = writeln!(std::io::stderr(), "{line}");
        if let Some(log) = &log {
            let _ = log.parent().map(std::fs::create_dir_all);
            let file = std::fs::File::options().create(true).append(true).open(log);
            let _ = file.and_then(|mut file| writeln!(file, "{line}"));
        }
        default(info);
    }));
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::time::Instant;

    use yd_core::feed::FILES;
    use yd_core::ids::Millis;

    use super::*;

    #[test]
    fn the_socket_is_the_flag_else_the_setting_else_the_daemons() {
        let home = Path::new("/Users/someone");
        let flag = [
            "--scene".to_owned(),
            "x".to_owned(),
            "--socket".to_owned(),
            "/t/y.sock".to_owned(),
        ];
        assert_eq!(
            socket(&flag, Some("/s.sock"), home),
            PathBuf::from("/t/y.sock")
        );
        assert_eq!(socket(&[], Some("/s.sock"), home), PathBuf::from("/s.sock"));
        assert_eq!(socket(&[], None, home), home.join(".yi/daemon.sock"));
    }

    #[test]
    fn a_folder_ask_starts_the_files_source_and_its_rows_arrive() -> Result<(), Box<dyn Error>> {
        let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/shell");
        let win = crate::root::init();
        let (sender, inbox) = inbox(|| {});
        let clock: Clock = std::sync::Arc::new(|| Millis(0));
        let mut running = Running::new(Rc::downgrade(&win.hub), sender, clock);
        running.carry(Effect::Source(FILES, SourceCmd::Root(folder)));
        assert!(
            running.sources.contains_key(&FILES),
            "started on its first ask"
        );
        let mut hub = win.hub.try_borrow_mut().map_err(|_| "hub")?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            inbox.drain(&mut hub.model);
            let table = hub.model.tables.get(FILES);
            let rows = table.map_or(0, yd_core::row::Rows::len);
            if rows > 0 {
                break;
            }
            if Instant::now() > deadline {
                return Err("the files source sent no rows".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }

    /// No address is asked for, so no browser starts: the source waits for one.
    #[test]
    fn a_page_ask_starts_the_browser_source_drawing_to_the_panes_own_screen()
    -> Result<(), Box<dyn Error>> {
        use yd_panes::{report, web};
        use yd_ui::pane::SCENE_PANE;

        let win = crate::root::init();
        let (sender, _inbox) = inbox(|| {});
        let clock: Clock = std::sync::Arc::new(|| Millis(0));
        let mut running = Running::new(Rc::downgrade(&win.hub), sender, clock);
        assert!(
            running.page(WEB).is_none(),
            "a page with no pane to draw it"
        );
        {
            let mut hub = win.hub.try_borrow_mut().map_err(|_| "hub")?;
            hub.panes.get_mut::<web::pane::State>(Some(SCENE_PANE));
            hub.panes.get_mut::<report::pane::State>(Some(SCENE_PANE));
        }
        let page = running.page(WEB).ok_or("no screen for the web page")?;
        page.push(2, 1, vec![255; 8])?;
        let report = running.page(REPORT).ok_or("no screen for the report")?;
        report.push(1, 3, vec![255; 12])?;
        let sizes = {
            let hub = win.hub.try_borrow().map_err(|_| "hub")?;
            let web = hub.panes.get::<web::pane::State>(Some(SCENE_PANE));
            let report = hub.panes.get::<report::pane::State>(Some(SCENE_PANE));
            (
                web.and_then(|state| state.screen.size()),
                report.and_then(|state| state.screen.size()),
            )
        };
        assert_eq!(
            sizes,
            (Some((2, 1)), Some((1, 3))),
            "each pane's own picture"
        );
        assert!(running.page(TERMINAL).is_none(), "a feed that is no page");
        let size = SourceCmd::Resize {
            cols: 400,
            rows: 300,
        };
        running.carry(Effect::Source(WEB, size));
        assert!(
            running.sources.contains_key(&WEB),
            "started on its first ask"
        );
        assert_eq!(running.pending.len(), 0, "{:?}", running.pending);
        Ok(())
    }

    #[test]
    fn a_window_effect_opens_the_window_the_grid_made() -> Result<(), Box<dyn Error>> {
        use crate::windows::hub::{Launch, Registry, specs};
        use yd_core::grid::step::GridMsg;
        use yd_core::ids::{PaneId, WindowId};

        let shared = crate::root::init().hub;
        let launch = Launch {
            headless: true,
            places: crate::windows::places::Places::memory(),
        };
        let _specs = specs(&shared, &[], launch);
        let (sender, _inbox) = inbox(|| {});
        let clock: Clock = std::sync::Arc::new(|| Millis(0));
        let mut running = Running::new(Rc::downgrade(&shared), sender, clock);
        let leave = Msg::Grid(GridMsg::Leave(PaneId(4)));
        let effects = yd_core::msg::step(&mut shared.try_borrow_mut()?.model, leave);
        assert!(matches!(effects.as_slice(), [Effect::Window(_)]));
        effects.into_iter().for_each(|effect| running.carry(effect));
        let hub = shared.try_borrow()?;
        let registry = hub.panes.get::<Registry>(None).ok_or("no registry")?;
        assert!(
            registry.shows(WindowId(1)),
            "the sink reached the window layer"
        );
        Ok(())
    }

    /// Dies with a quit that leaves before a source has stopped its child, or one that
    /// waits on a source that never ends.
    #[test]
    fn quitting_waits_for_sources_up_to_the_grace() -> Result<(), Box<dyn Error>> {
        let (done, heard) = std::sync::mpsc::channel();
        let quick = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            done.send(()).ok()
        });
        let stuck = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_secs(3));
            Some(())
        });
        let asked = Instant::now();
        let left = join_within(vec![quick, stuck], Duration::from_millis(600));
        assert!(
            asked.elapsed() < Duration::from_secs(2),
            "quit waited past the grace"
        );
        assert_eq!(left, 1, "the source that ended was waited for");
        assert!(
            heard.try_recv().is_ok(),
            "the quick source finished before quit returned"
        );
        Ok(())
    }

    #[test]
    fn quitting_writes_a_cancelled_answer_for_the_ask_that_waits() -> Result<(), Box<dyn Error>> {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixListener;

        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/in5");
        std::fs::create_dir_all(&dir)?;
        let socket = dir.join(format!("quit-{}.sock", std::process::id()));
        if socket.exists() {
            std::fs::remove_file(&socket)?;
        }
        let listener = UnixListener::bind(&socket)?;
        let (report, heard) = std::sync::mpsc::channel();
        std::thread::Builder::new().spawn(move || {
            let serve = || -> Option<bool> {
                let (stream, _) = listener.accept().ok()?;
                let mut writer = stream.try_clone().ok()?;
                let mut lines = BufReader::new(stream).lines();
                let hello = lines.next()?.ok()?;
                let id = hello.split("\"id\":").nth(1)?.split([',', '}']).next()?;
                let agent = r#"{"protocolVersion":2,"agentInfo":{"name":"yi","version":"0"}}"#;
                writeln!(writer, r#"{{"jsonrpc":"2.0","id":{id},"result":{agent}}}"#).ok()?;
                let heard = lines.map_while(Result::ok).any(|l| l.contains("cancelled"));
                Some(heard)
            };
            report.send(serve()).ok();
        })?;

        let mut win = crate::root::init();
        let (sender, _inbox) = inbox(|| {});
        let clock: Clock = std::sync::Arc::new(|| Millis(0));
        let mut running = Running::new(Rc::downgrade(&win.hub), sender.clone(), clock.clone());
        let config = Config {
            socket: socket.clone(),
            poll: None,
            start: None,
        };
        running.client = Some(Client::spawn(config, sender, clock)?);
        let running = Rc::new(RefCell::new(running));
        {
            let mut hub = win.hub.try_borrow_mut()?;
            let whole = crate::shell::view::SCENES.iter();
            let scene = whole.clone().find(|scene| scene.name == "shell/everything");
            (scene.ok_or("no scene")?.setup)(&mut hub);
            assert!(!hub.model.wire.asks.is_empty(), "an approval waits");
            let sink = Rc::clone(&running);
            hub.set_sink(move |effect| sink.borrow_mut().carry(effect));
            hub.panes.get_mut::<Started>(None).ok_or("no slot")?.0 = Some(running);
        }
        quit(&mut win);
        let cancelled = heard.recv_timeout(Duration::from_secs(10))?;
        std::fs::remove_file(&socket)?;
        assert_eq!(cancelled, Some(true), "the server heard it cancelled");
        Ok(())
    }

    #[test]
    fn a_run_that_is_not_the_native_app_and_names_no_socket_connects_nowhere() {
        let mut win = crate::root::init();
        let app = crate::root::app().handle();
        start(&mut win, app, false);
        assert!(running(&win).is_none(), "no client, no sink, no daemon");
    }

    #[test]
    fn a_request_with_no_client_comes_back_unsent() {
        let win = crate::root::init();
        let (sender, _inbox) = inbox(|| {});
        let clock: Clock = std::sync::Arc::new(|| Millis(7));
        let mut running = Running::new(Rc::downgrade(&win.hub), sender, clock);
        let thread = yd_core::ids::ThreadId::new("t1");
        let params = yd_core::wire::request::of_thread(&thread);
        let request = yd_core::wire::request::Request::call(3, "session/load", params);
        running.carry(Effect::Wire(request));
        let unsent = matches!(
            running.pending.as_slice(),
            [Msg::Wire(WireEvent::Unsent { at: Millis(7), .. })]
        );
        assert!(unsent, "the model hears it never left");
    }
}
