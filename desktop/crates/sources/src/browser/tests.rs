//! The browser source against a stand-in page: what it asks of Chrome and what it shows.

use std::collections::VecDeque;
use std::error::Error;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use yd_core::command::table::Run;
use yd_core::command::tools::web;
use yd_core::feed::{FeedStatus, SourceCmd};
use yd_core::inbox::inbox;
use yd_core::model::Model;
use yi_browser::Update;

use yd_core::feed::{Pointer, ScreenInput, ScreenKey};
use yi_types::cdp::{KeyInput, MouseInput};

use super::{Chrome, FEED, MAX_HISTORY, Missing, Page, Tab, configured_chrome, decoded, serve};

/// A page with no address outside this folder.
const PAGE: &str = "<body style=\"margin:0;background:#2f9a57\"><b>Upload limit</b></body>";
/// A frame as Chrome sends one: base64 of a PNG, here a one by one red pixel.
const DOT: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC";

/// The width and height a PNG's header gives.
fn sides(png: &[u8]) -> Option<(u32, u32)> {
    let side = |at: usize| Some(u32::from_be_bytes(png.get(at..at + 4)?.try_into().ok()?));
    Some((side(16)?, side(20)?))
}

#[test]
fn a_page_that_keeps_changing_its_address_keeps_only_the_last_hundred() -> Result<(), Box<dyn Error>>
{
    let mut tab = Tab::default();
    for n in 0..1_000 {
        tab.landed(format!("http://localhost:1/#{n}"));
    }
    assert_eq!(
        (tab.entries.len(), tab.url()),
        (MAX_HISTORY, "http://localhost:1/#999")
    );
    tab.step = Some(-1);
    tab.landed("http://localhost:1/#998".to_owned());
    assert_eq!((tab.at, tab.entries.len()), (MAX_HISTORY - 2, MAX_HISTORY));
    Ok(())
}

/// A browser that records what it is asked and replays updates it is fed.
#[derive(Clone, Default)]
struct Script {
    calls: Arc<Mutex<Vec<String>>>,
    fed: Arc<Mutex<VecDeque<Update>>>,
}

impl Script {
    fn say(&self, call: String) {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(call);
        }
    }
}

impl Page for Script {
    fn poll(&mut self) -> Vec<Update> {
        self.fed
            .lock()
            .map(|mut fed| fed.drain(..).collect())
            .unwrap_or_default()
    }
    fn navigate(&mut self, url: &str) {
        self.say(format!("navigate {url}"));
    }
    fn go(&mut self, step: i64) {
        self.say(format!("go {step}"));
    }
    fn reload(&mut self) {
        self.say("reload".to_owned());
    }
    fn resize(&mut self, (w, h): (u32, u32), scale: u32) {
        self.say(format!("resize {w}x{h}@{scale}"));
    }
    fn text(&mut self, text: &str) {
        self.say(format!("text {text}"));
    }
    fn mouse(&mut self, input: &MouseInput) {
        let (dx, dy) = (input.delta_x, input.delta_y);
        let held = (input.buttons, input.click_count);
        let said = format!(
            "{:?} {},{} {held:?} {dx},{dy}",
            input.kind, input.x, input.y
        );
        self.say(said);
    }
    fn key(&mut self, input: &KeyInput) {
        let code = input.windows_virtual_key_code;
        let said = format!("{:?} {} {code} {:?}", input.kind, input.key, input.text);
        self.say(said);
    }
}

fn run(
    cmd: mpsc::Sender<SourceCmd>,
    id: yd_core::ids::CommandId,
    text: Option<&str>,
) -> Sender<SourceCmd> {
    let mut run = Run::new(id);
    run.text = text.map(str::to_owned);
    let _ = cmd.send(SourceCmd::Run(run));
    cmd
}

fn settle(until: impl Fn() -> bool) -> Result<(), Box<dyn Error>> {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !until() {
        if Instant::now() > deadline {
            return Err("timed out".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

#[test]
fn the_source_sends_the_browser_back_reload_and_resize() -> Result<(), Box<dyn Error>> {
    let script = Script::default();
    let (sender, inbox) = inbox(|| {});
    let (cmds, rx) = mpsc::channel();
    let (page, shown) = (script.clone(), Arc::new(Mutex::new(Vec::<Vec<u8>>::new())));
    let into = shown.clone();
    let launched = Arc::new(Mutex::new(Vec::new()));
    let said = launched.clone();
    let source = std::thread::spawn(move || {
        let launch = move |url: &str, (w, h): (u32, u32), scale: u32| {
            said.lock()
                .map(|mut l| l.push(format!("{url} {w}x{h}@{scale}")))
                .ok();
            Ok::<_, Missing>(page.clone())
        };
        let show = move |frame: Vec<u8>| {
            into.lock().map(|mut s| s.push(frame)).ok();
            Ok(())
        };
        serve(launch, show, sender.feed(FEED), rx)
    });
    cmds.send(SourceCmd::Resize {
        cols: 400,
        rows: 300,
    })?;
    let cmds = run(cmds, web::OPEN, Some("localhost:5173/upload"));
    script.fed.lock().map_err(|_| "poisoned")?.extend([
        Update::Url("http://localhost:5173/upload".to_owned()),
        Update::Frame(DOT.to_owned()),
    ]);
    settle(|| shown.lock().is_ok_and(|s| !s.is_empty()))?;
    let cmds = run(cmds, web::BACK, None);
    let cmds = run(cmds, web::RELOAD, None);
    cmds.send(SourceCmd::Resize {
        cols: 500,
        rows: 250,
    })?;
    let cmds = run(cmds, web::WIDTH, Some("1000"));
    let calls = || script.calls.lock().map(|c| c.clone()).unwrap_or_default();
    settle(|| calls().len() >= 4)?;
    drop(cmds);
    source.join().map_err(|_| "the source died")??;
    assert_eq!(
        launched.lock().map(|l| l.clone()).unwrap_or_default(),
        ["http://localhost:5173/upload 400x300@2"]
    );
    assert_eq!(
        calls(),
        ["go -1", "reload", "resize 500x250@2", "resize 1000x500@2"]
    );
    let frame = shown
        .lock()
        .map_err(|_| "poisoned")?
        .first()
        .cloned()
        .ok_or("no frame")?;
    assert_eq!(
        frame,
        decoded(DOT)?,
        "the frame reaches the screen as its PNG"
    );
    let mut model = Model::default();
    inbox.drain(&mut model);
    let row = model
        .tables
        .get(FEED)
        .and_then(|t| t.find(&yd_core::ids::RowId::new("page:0")).cloned());
    let row = row.ok_or("no page row")?;
    assert_eq!(row.words("url"), Some("http://localhost:5173/upload"));
    assert_eq!(row.words("title"), Some("localhost:5173"));
    assert_eq!(row.words("width"), Some("1000"));
    Ok(())
}

/// A frame of which only the header is real: the source reads the picture's size from it.
fn header(width: u32, height: u32) -> String {
    use base64::Engine;
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.extend(width.to_be_bytes());
    png.extend(height.to_be_bytes());
    base64::engine::general_purpose::STANDARD.encode(png)
}

/// The page is 400 by 300 CSS pixels and its picture twice that, as a Mac's own density.
#[test]
fn a_press_a_wheel_and_a_named_key_reach_the_page_at_its_own_points() -> Result<(), Box<dyn Error>>
{
    let script = Script::default();
    let (sender, _inbox) = inbox(|| {});
    let (cmds, rx) = mpsc::channel();
    let page = script.clone();
    let source = std::thread::spawn(move || {
        let launch = move |_: &str, _: (u32, u32), _: u32| Ok::<_, Missing>(page.clone());
        serve(launch, |_: Vec<u8>| Ok(()), sender.feed(FEED), rx)
    });
    cmds.send(SourceCmd::Resize {
        cols: 400,
        rows: 300,
    })?;
    let cmds = run(cmds, web::OPEN, Some("localhost:5173"));
    let fed = |update| script.fed.lock().map(|mut fed| fed.push_back(update));
    fed(Update::Frame(header(800, 600))).map_err(|_| "poisoned")?;
    settle(|| script.fed.lock().is_ok_and(|fed| fed.is_empty()))?;
    let at = |phase| ScreenInput::Pointer {
        phase,
        x: 400,
        y: 300,
    };
    for input in [
        at(Pointer::Press),
        at(Pointer::Release),
        ScreenInput::Wheel { dx: 0, dy: -30 },
        ScreenInput::Key(ScreenKey::Enter),
        ScreenInput::Text("hi".to_owned()),
    ] {
        cmds.send(SourceCmd::Screen(input))?;
    }
    let calls = || script.calls.lock().map(|c| c.clone()).unwrap_or_default();
    settle(|| calls().len() >= 6)?;
    drop(cmds);
    source.join().map_err(|_| "the source died")??;
    assert_eq!(
        calls(),
        [
            "MousePressed 200,150 (1, 1) 0,0",
            "MouseReleased 200,150 (0, 1) 0,0",
            "MouseWheel 200,150 (0, 0) 0,30",
            "KeyDown Enter 13 Some(\"\\r\")",
            "KeyUp Enter 13 None",
            "text hi",
        ]
    );
    Ok(())
}

/// Where a test may write: the lane's own `target/`.
fn scratch(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let exe = std::env::current_exe()?;
    let target = exe
        .ancestors()
        .find(|p| p.ends_with("target"))
        .ok_or("no target")?;
    let dir = target.join("browser-tests").join(name);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[test]
fn the_chrome_yis_config_names_is_the_one_the_app_starts() -> Result<(), Box<dyn Error>> {
    let home = scratch("configured")?;
    std::fs::create_dir_all(home.join(".yi"))?;
    let _ = std::fs::remove_file(home.join(".yi/config.json"));
    assert_eq!(configured_chrome(&home), None);
    let config = r#"{"model": "faux/faux-1", "browser": {"path": "/opt/chrome é/run"}}"#;
    std::fs::write(home.join(".yi/config.json"), config)?;
    assert_eq!(
        configured_chrome(&home),
        Some(PathBuf::from("/opt/chrome é/run"))
    );
    std::fs::write(home.join(".yi/config.json"), r#"{"browser": {"path": 30}}"#)?;
    assert_eq!(configured_chrome(&home), None);
    Ok(())
}

#[test]
fn a_missing_chrome_is_a_status_with_where_it_looked() -> Result<(), Box<dyn Error>> {
    let dir = scratch("missing")?;
    let chrome = Chrome {
        yi: std::env::current_exe()?,
        chrome: Some(dir.join("no-chrome-here")),
        profiles: dir.clone(),
    };
    let (sender, inbox) = inbox(|| {});
    let (cmds, rx) = mpsc::channel();
    let source = std::thread::spawn(move || {
        let launch = move |url: &str, size, scale| chrome.launch(url, size, scale);
        serve(launch, |_: Vec<u8>| Ok(()), sender.feed(FEED), rx)
    });
    drop(run(cmds, web::OPEN, Some("http://localhost:1/")));
    source.join().map_err(|_| "the source died")??;
    let mut model = Model::default();
    inbox.drain(&mut model);
    let status = model.tables.get(FEED).map(|t| t.status().clone());
    let Some(FeedStatus::Error(why)) = status else {
        return Err(format!("not an error: {status:?}").into());
    };
    assert!(why.starts_with("Chrome was not found; looked in"), "{why}");
    assert!(why.contains("no-chrome-here"), "{why}");
    Ok(())
}

/// What `yi __fds` printed when a GUI app's Metal cache sat on descriptor 4.
const REFUSED: &str = "error: __fds: file descriptors 3 and 4 were already open";

#[test]
fn a_shim_that_refuses_says_why_in_the_pane_not_that_the_pipe_closed() -> Result<(), Box<dyn Error>>
{
    let dir = scratch("refused")?;
    let shim = dir.join("shim.sh");
    std::fs::write(&shim, format!("#!/bin/sh\necho '{REFUSED}' >&2\nexit 1\n"))?;
    let mut perms = std::fs::metadata(&shim)?.permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&shim, perms)?;
    let chrome = Chrome {
        yi: shim,
        chrome: Some(PathBuf::from("/bin/sh")),
        profiles: dir.clone(),
    };
    let (sender, inbox) = inbox(|| {});
    let (cmds, rx) = mpsc::channel();
    let source = std::thread::spawn(move || {
        let launch = move |url: &str, size, scale| chrome.launch(url, size, scale);
        serve(launch, |_: Vec<u8>| Ok(()), sender.feed(FEED), rx)
    });
    drop(run(cmds, web::OPEN, Some("http://localhost:1/")));
    source.join().map_err(|_| "the source died")??;
    let mut model = Model::default();
    inbox.drain(&mut model);
    let status = model.tables.get(FEED).map(|t| t.status().clone());
    let Some(FeedStatus::Error(why)) = status else {
        return Err(format!("not an error: {status:?}").into());
    };
    assert_eq!(
        why,
        "`yi __fds` would not start Chrome: file descriptors 3 and 4 were already open"
    );
    Ok(())
}

/// Set in the copy of this binary the test below starts, which runs the check alone.
const HOLD_CHILD: &str = "YD_HOLD_CHILD";

/// A framework's file opened without close-on-exec after start, as Metal's cache is, never
/// lands on 3 or 4, so the shim always finds them free. Checked in a fresh process.
#[test]
fn a_file_a_framework_leaks_to_children_never_takes_the_pipe_descriptors()
-> Result<(), Box<dyn Error>> {
    use std::os::fd::AsRawFd;
    if std::env::var_os(HOLD_CHILD).is_some() {
        super::hold_pipe_fds();
        let flags = rustix::fs::OFlags::RDWR;
        let leaked = rustix::fs::open("/dev/null", flags, rustix::fs::Mode::empty())?;
        assert!(leaked.as_raw_fd() > 4, "leaked onto {}", leaked.as_raw_fd());
        return Ok(());
    }
    let name =
        "browser::tests::a_file_a_framework_leaks_to_children_never_takes_the_pipe_descriptors";
    #[allow(clippy::disallowed_methods, reason = "this test run again alone")]
    let ran = std::process::Command::new(std::env::current_exe()?)
        .args(["--exact", name, "--test-threads=1"])
        .env(HOLD_CHILD, "1")
        .output()?;
    let said = String::from_utf8_lossy(&ran.stdout);
    assert!(ran.status.success() && said.contains("1 passed"), "{said}");
    Ok(())
}

#[test]
fn the_child_is_stopped_when_the_source_ends() -> Result<(), Box<dyn Error>> {
    let dir = scratch("child")?;
    let pid_file = dir.join("pid");
    let _ = std::fs::remove_file(&pid_file);
    let shim = dir.join("shim.sh");
    let body = format!(
        "#!/bin/sh\n[ \"$2\" = /usr/bin/true ] && exit 0\necho $$ > '{}'\nexec sleep 600\n",
        pid_file.display()
    );
    std::fs::write(&shim, body)?;
    let mut perms = std::fs::metadata(&shim)?.permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&shim, perms)?;
    let chrome = Chrome {
        yi: shim,
        chrome: Some(PathBuf::from("/bin/sh")),
        profiles: dir.clone(),
    };
    let (sender, _inbox) = inbox(|| {});
    let (cmds, rx) = mpsc::channel();
    let source = std::thread::spawn(move || {
        let launch = move |url: &str, size, scale| chrome.launch(url, size, scale);
        serve(launch, |_: Vec<u8>| Ok(()), sender.feed(FEED), rx)
    });
    let cmds = run(cmds, web::OPEN, Some("http://localhost:1/"));
    settle(|| std::fs::read_to_string(&pid_file).is_ok_and(|p| p.ends_with('\n')))?;
    let pid: i32 = std::fs::read_to_string(&pid_file)?.trim().parse()?;
    let pid = rustix::process::Pid::from_raw(pid).ok_or("no pid")?;
    assert!(
        rustix::process::test_kill_process(pid).is_ok(),
        "the child runs"
    );
    drop(cmds);
    source.join().map_err(|_| "the source died")??;
    assert_eq!(
        rustix::process::test_kill_process(pid),
        Err(rustix::io::Errno::SRCH),
        "the child is gone once the source ends"
    );
    Ok(())
}

/// Real Chrome on a page from this folder; no network address is ever opened.
#[test]
#[ignore = "starts real Chrome; run by hand"]
fn real_chrome_shows_a_local_page_on_the_screen() -> Result<(), Box<dyn Error>> {
    let mut chrome = Chrome::installed();
    if !chrome.yi.is_file() || yi_browser::find(None, None).is_err() {
        eprintln!("skipped: no installed yi or Chrome");
        return Ok(());
    }
    chrome.profiles = scratch("real")?;
    let page = scratch("real")?.join("page.html");
    std::fs::write(&page, PAGE)?;
    let (sender, _inbox) = inbox(|| {});
    let (cmds, rx) = mpsc::channel();
    let shown = Arc::new(Mutex::new(None::<(u32, u32)>));
    let into = shown.clone();
    let source = std::thread::spawn(move || {
        let launch = move |url: &str, size, scale| chrome.launch(url, size, scale);
        let show = move |png: Vec<u8>| {
            into.lock().map(|mut s| *s = sides(&png)).ok();
            Ok(())
        };
        serve(launch, show, sender.feed(FEED), rx)
    });
    cmds.send(SourceCmd::Resize {
        cols: 200,
        rows: 120,
    })?;
    let url = format!("file://{}", page.display());
    let cmds = run(cmds, web::OPEN, Some(&url));
    // Incident: headless Chrome's screencast came at the CSS size, whatever the scale asked.
    let seen = settle(|| shown.lock().is_ok_and(|s| *s == Some((200, 120))));
    drop(cmds);
    source.join().map_err(|_| "the source died")??;
    seen.map_err(|_| format!("no 200 by 120 frame; last {:?}", shown.lock().ok()))?;
    Ok(())
}
