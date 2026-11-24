//! The live web app: Chrome through `yi-browser`, its PNG frames handed to a screen, input back.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use base64::Engine;
use yd_core::command::table::Run;
use yd_core::command::tools::web;
use yd_core::feed::{
    FeedEvent, FeedStatus, Pointer, ScreenInput, ScreenKey, SourceCmd, page_row_id,
};
use yd_core::inbox::{Closed, FeedTx};
use yd_core::row::Row;
use yi_browser::{Browser, Update};
use yi_types::cdp::{KeyInput, KeyKind, MouseButton, MouseInput, MouseKind};

use crate::cdp::Design;

pub use yd_core::feed::WEB as FEED;

/// How often an open page is read while no command comes: a frame at most this late.
const TICK: Duration = Duration::from_millis(16);
/// Device pixels per CSS pixel: frames come at the Mac's own density.
pub const SCALE: u32 = 2;
/// The page size before the surface says its own, in CSS pixels.
const FIRST_SIZE: (u32, u32) = (800, 600);
/// The widths `web.width` takes, in CSS pixels.
const WIDTHS: std::ops::RangeInclusive<u32> = 200..=4_096;
/// An address, a note or a failure from the page is cut to this many characters.
const MAX_TEXT: usize = 2_048;
/// Back steps kept per tab; a page that rewrites its own address would grow history forever.
const MAX_HISTORY: usize = 100;

/// What the source asks of a browser: Yi's own, or a test's stand-in.
pub trait Page {
    fn poll(&mut self) -> Vec<Update>;
    fn navigate(&mut self, url: &str);
    fn go(&mut self, step: i64);
    fn reload(&mut self);
    /// The viewport in CSS pixels at `scale` device pixels each.
    fn resize(&mut self, size: (u32, u32), scale: u32);
    fn text(&mut self, text: &str);
    fn mouse(&mut self, input: &MouseInput);
    fn key(&mut self, input: &KeyInput);
    /// A call outside Page, Input and Emulation, under the caller's number. False when the
    /// link carries none, as Yi's does not yet: its router sends only its own calls.
    fn call(&mut self, _id: u64, _method: &str, _params: serde_json::Value) -> bool {
        false
    }
    /// The answers to [`Page::call`] that came since the last look.
    fn answers(&mut self) -> Vec<Answer> {
        Vec::new()
    }
}

/// The answer to one [`Page::call`]: its number, and the result or what Chrome refused with.
pub type Answer = (u64, Result<serde_json::Value, String>);

impl Page for Browser {
    fn poll(&mut self) -> Vec<Update> {
        Browser::poll(self)
    }
    fn navigate(&mut self, url: &str) {
        self.router.navigate(url);
    }
    fn go(&mut self, step: i64) {
        self.router.go(step);
    }
    fn reload(&mut self) {
        self.router.reload();
    }
    fn resize(&mut self, size: (u32, u32), scale: u32) {
        self.router.resize(size, scale);
    }
    fn text(&mut self, text: &str) {
        self.router.insert_text(text);
    }
    fn mouse(&mut self, input: &MouseInput) {
        self.router.mouse(input);
    }
    fn key(&mut self, input: &KeyInput) {
        self.router.key(input);
    }
}

/// Why no browser started. Nothing is fetched or installed to make one start.
#[derive(Debug, thiserror::Error)]
pub enum Missing {
    #[error("yi was not found at {0}; the browser starts through `yi __fds`")]
    Yi(PathBuf),
    #[error("Chrome was not found; looked in {}", .0.join(", "))]
    Chrome(Vec<String>),
    #[error("Chrome did not start: {0}")]
    Start(#[source] std::io::Error),
    /// What the shim said on stderr when it would not start a program: Chrome's own start
    /// drops it, and the pane then only hears that the pipe closed.
    #[error("`yi __fds` would not start Chrome: {0}")]
    Shim(String),
}

/// How long the shim may take to start and end `/usr/bin/true`.
const PROBE: Duration = Duration::from_secs(2);
/// The most of the shim's stderr read, in bytes.
const PROBE_SAID: u64 = 8_192;

/// Incident: Metal opened its shader cache on descriptor 4 without close-on-exec, every child
/// inherited it and `yi __fds` refused, so no page ever showed. Held here, 3 and 4 stay free.
pub fn hold_pipe_fds() {
    use std::os::fd::AsRawFd;
    static HELD: std::sync::OnceLock<Vec<std::fs::File>> = std::sync::OnceLock::new();
    HELD.get_or_init(|| {
        let opened = (0..2).filter_map(|_| std::fs::File::open("/dev/null").ok());
        opened.filter(|file| file.as_raw_fd() <= 4).collect()
    });
}

/// Runs the shim on `/usr/bin/true` and keeps what it says when it fails.
fn probe(yi: &std::path::Path) -> Result<(), Missing> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    #[allow(clippy::disallowed_methods, reason = "the shim probe spawn")]
    let mut child = Command::new(yi)
        .args(["__fds", "/usr/bin/true"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(Missing::Start)?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < PROBE => std::thread::sleep(TICK),
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Missing::Shim("it did not end within 2 s".to_owned()));
            }
        }
    };
    if status.success() {
        return Ok(());
    }
    let mut said = Vec::new();
    if let Some(stderr) = child.stderr.take() {
        let _ = stderr.take(PROBE_SAID).read_to_end(&mut said);
    }
    let said = String::from_utf8_lossy(&said);
    let said = said.trim().trim_start_matches("error: __fds: ");
    Err(Missing::Shim(if said.is_empty() {
        format!("it exited {status}")
    } else {
        cut(said)
    }))
}

/// How to start Chrome: the installed `yi` as the pipe shim, a Chrome, a profile of its own.
#[derive(Debug, Clone, PartialEq)]
pub struct Chrome {
    pub yi: PathBuf,
    /// A Chrome to use instead of looking for one.
    pub chrome: Option<PathBuf>,
    /// Where each browser's profile folder goes; the browser removes its own when it ends.
    pub profiles: PathBuf,
}

impl Chrome {
    /// The installed `yi` and the first Chrome or Chromium found, profiles under the temp folder.
    pub fn installed() -> Self {
        let yi = crate::server::installed_yi();
        Self {
            yi: yi.unwrap_or_else(|missing| missing.fallback),
            chrome: std::env::var_os("HOME").and_then(|home| configured_chrome(Path::new(&home))),
            profiles: std::env::temp_dir(),
        }
    }

    pub fn launch(&self, url: &str, size: (u32, u32), scale: u32) -> Result<Browser, Missing> {
        if !self.yi.is_file() {
            return Err(Missing::Yi(self.yi.clone()));
        }
        let chrome = yi_browser::find(self.chrome.as_deref(), None).map_err(Missing::Chrome)?;
        probe(&self.yi)?;
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let name = format!("yi-desktop-browser-{}-{n}", std::process::id());
        let profile = self.profiles.join(name);
        Browser::launch(&self.yi, &chrome, None, &profile, url, size, scale).map_err(Missing::Start)
    }
}

/// The `browser.path` of Yi's own config under `home`, the Chrome Yi's console runs too.
pub fn configured_chrome(home: &Path) -> Option<PathBuf> {
    let config = std::fs::read(home.join(".yi/config.json")).ok()?;
    let config: serde_json::Value = serde_json::from_slice(&config).ok()?;
    config.pointer("/browser/path")?.as_str().map(PathBuf::from)
}

/// What the tab bar names a page by: its host, or a file's name.
pub fn title(url: &str) -> String {
    if let Some(path) = url.strip_prefix("file://") {
        return path.rsplit('/').next().unwrap_or(path).to_owned();
    }
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', '?', '#'])
        .next()
        .unwrap_or(rest)
        .to_owned()
}

fn cut(text: &str) -> String {
    text.chars().take(MAX_TEXT).collect()
}

/// One page and what the bar shows of it. History is counted here: Chrome tells only the
/// address it landed on, so back and forward are known from the steps this source asked for.
#[derive(Debug, Default)]
struct Tab {
    entries: Vec<String>,
    at: usize,
    step: Option<i64>,
    asked: Option<Instant>,
    ms: Option<u128>,
    failed: Option<String>,
    note: Option<String>,
    width: Option<u32>,
}

impl Tab {
    fn url(&self) -> &str {
        self.entries.get(self.at).map_or("", String::as_str)
    }

    fn landed(&mut self, url: String) {
        self.failed = None;
        self.ms = self.asked.take().map(|asked| asked.elapsed().as_millis());
        match self.step.take() {
            Some(step) => {
                let at = i64::try_from(self.at).unwrap_or(0).saturating_add(step);
                self.at = usize::try_from(at.max(0)).unwrap_or(0);
                match self.entries.get_mut(self.at) {
                    Some(entry) => *entry = url,
                    None => self.entries.push(url),
                }
            }
            None if self.url() == url => {}
            None => {
                self.entries
                    .truncate(self.at.saturating_add(1).min(self.entries.len()));
                self.entries.push(url);
                self.at = self.entries.len().saturating_sub(1);
            }
        }
        let over = self.entries.len().saturating_sub(MAX_HISTORY);
        self.entries.drain(..over);
        self.at = self.at.saturating_sub(over);
    }

    fn row(&self, instance: u32) -> Row {
        let url = self.url();
        let flag = |on: bool| if on { "yes" } else { "no" };
        let status = self.failed.as_deref().unwrap_or("");
        let ms = self.ms.map(|ms| ms.to_string()).unwrap_or_default();
        let width = self.width.map(|w| w.to_string()).unwrap_or_default();
        let forward = self.at.saturating_add(1) < self.entries.len();
        Row::new(page_row_id(FEED.instance(instance)).as_str())
            .text("url", url)
            .text("title", title(url))
            .text("status", status)
            .text("ms", ms)
            .text("width", width)
            .text("can_back", flag(self.at > 0))
            .text("can_forward", flag(forward))
            .text("loading", flag(self.asked.is_some()))
            .text("note", self.note.clone().unwrap_or_default())
    }
}

/// The registry's row: Chrome as installed, frames to no screen. The app starts a pane's page
/// with [`serve`] and a sink that hands each PNG to the pane's screen.
pub fn run(out: FeedTx, cmds: Receiver<SourceCmd>) -> Result<(), Closed> {
    let chrome = Chrome::installed();
    let launch = move |url: &str, size, scale| chrome.launch(url, size, scale);
    serve(launch, |_: Vec<u8>| Ok(()), out, cmds)
}

/// One page for one feed instance. Chrome starts on the first address and is killed and reaped
/// when `cmds` closes or says `Close`. `show` gets each frame's PNG and says why it drew none.
pub fn serve<P: Page>(
    mut launch: impl FnMut(&str, (u32, u32), u32) -> Result<P, Missing>,
    mut show: impl FnMut(Vec<u8>) -> Result<(), String>,
    out: FeedTx,
    cmds: Receiver<SourceCmd>,
) -> Result<(), Closed> {
    let instance = out.id().instance;
    let (mut page, mut tab, mut size) = (None::<P>, Tab::default(), FIRST_SIZE);
    let mut sent: Option<Row> = None;
    let mut pointer = Held::default();
    let mut design = Design::default();
    out.send(FeedEvent::Status(FeedStatus::Empty))?;
    loop {
        let cmd = match &page {
            Some(_) => match cmds.recv_timeout(TICK) {
                Ok(cmd) => Some(cmd),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            },
            None => match cmds.recv() {
                Ok(cmd) => Some(cmd),
                Err(_) => break,
            },
        };
        let viewport = |size: (u32, u32), width: Option<u32>| match width {
            Some(w) => {
                let h = u64::from(size.1).saturating_mul(u64::from(w)) / u64::from(size.0.max(1));
                (w, u32::try_from(h).unwrap_or(u32::MAX).max(1))
            }
            None => size,
        };
        match cmd {
            None => {}
            Some(SourceCmd::Close) => {
                // The pane is gone and its page with it: so are the element and its changes.
                for event in design.landed() {
                    out.send(event)?;
                }
                break;
            }
            Some(SourceCmd::Run(Run { command, text, .. })) if command == web::OPEN => {
                let Some(url) = text.as_deref().and_then(web::address) else {
                    tab.note = Some("refused: not an http, https or file address".to_owned());
                    sent = upsert(&out, &tab, instance, sent)?;
                    continue;
                };
                tab.note = None;
                tab.asked = Some(Instant::now());
                match page.as_mut() {
                    Some(page) => page.navigate(&url),
                    None => match launch(&url, viewport(size, tab.width), SCALE) {
                        Ok(started) => {
                            page = Some(started);
                            out.send(FeedEvent::Status(FeedStatus::Live))?;
                        }
                        Err(why) => {
                            tab.asked = None;
                            out.send(FeedEvent::Status(FeedStatus::Error(why.to_string())))?;
                            continue;
                        }
                    },
                }
            }
            Some(SourceCmd::Run(run)) if Design::owns(run.command) => {
                if let Some(open) = page.as_mut() {
                    pointer.css = viewport(size, tab.width);
                    for event in design.ask(open, &run, |x, y| pointer.page(x, y)) {
                        out.send(event)?;
                    }
                }
            }
            Some(SourceCmd::Run(Run { command, text, .. })) => {
                let Some(open) = page.as_mut() else { continue };
                if command == web::BACK || command == web::FORWARD {
                    let step = if command == web::BACK { -1 } else { 1 };
                    tab.step = Some(step);
                    tab.asked = Some(Instant::now());
                    open.go(step);
                } else if command == web::RELOAD {
                    tab.asked = Some(Instant::now());
                    open.reload();
                } else if command == web::WIDTH {
                    let wide = text.as_deref().and_then(|t| t.trim().parse::<u32>().ok());
                    tab.width = wide.filter(|w| WIDTHS.contains(w));
                    open.resize(viewport(size, tab.width), SCALE);
                }
            }
            Some(SourceCmd::Refresh) => {
                if let Some(open) = page.as_mut() {
                    tab.asked = Some(Instant::now());
                    open.reload();
                }
            }
            Some(SourceCmd::Resize { cols, rows }) => {
                size = (u32::from(cols).max(1), u32::from(rows).max(1));
                if let Some(open) = page.as_mut() {
                    open.resize(viewport(size, tab.width), SCALE);
                }
            }
            Some(SourceCmd::Screen(input)) => {
                if let Some(open) = page.as_mut() {
                    pointer.css = viewport(size, tab.width);
                    pointer.send(open, input);
                }
            }
            Some(
                SourceCmd::Input(_)
                | SourceCmd::Root(_)
                | SourceCmd::Read(_)
                | SourceCmd::Revert { .. }
                | SourceCmd::Seen(_)
                | SourceCmd::Labels(_)
                | SourceCmd::Readable(_),
            ) => {}
        }
        let Some(open) = page.as_mut() else {
            sent = upsert(&out, &tab, instance, sent)?;
            continue;
        };
        let mut frame = None;
        for update in open.poll() {
            match update {
                Update::Frame(data) => frame = Some(data),
                Update::Url(url) => {
                    tab.landed(cut(&url));
                    for event in design.landed() {
                        out.send(event)?;
                    }
                }
                Update::NavigateFailed(why) => {
                    (tab.asked, tab.step) = (None, None);
                    tab.failed = Some(cut(&why));
                }
                Update::Skipped(why) => tab.note = Some(cut(&why)),
                Update::Failed(why) => {
                    page = None;
                    out.send(FeedEvent::Status(FeedStatus::Error(cut(&why))))?;
                    break;
                }
            }
        }
        // A page that failed took its elements with it, as a page gone elsewhere does.
        let gone = if page.is_none() {
            design.landed()
        } else {
            Vec::new()
        };
        let answers = page.as_mut().map(Page::answers).unwrap_or_default();
        for event in gone
            .into_iter()
            .chain(design.heard(answers, tab.url(), pointer.ratio()))
        {
            out.send(event)?;
        }
        if let Some(data) = frame {
            let png = decoded(&data).map_err(|why| why.to_string());
            pointer.picture = png.as_deref().ok().and_then(sides).or(pointer.picture);
            if let Err(why) = png.and_then(&mut show) {
                tab.note = Some(cut(&format!("a frame was skipped: {why}")));
            }
        }
        sent = upsert(&out, &tab, instance, sent)?;
    }
    drop(page);
    Ok(())
}

/// Sends the page's row when it changed since the last one sent.
fn upsert(
    out: &FeedTx,
    tab: &Tab,
    instance: u32,
    sent: Option<Row>,
) -> Result<Option<Row>, Closed> {
    if tab.entries.is_empty() && tab.note.is_none() {
        return Ok(sent);
    }
    let row = tab.row(instance);
    if sent.as_ref() != Some(&row) {
        out.send(FeedEvent::Upsert(row.clone()))?;
    }
    Ok(Some(row))
}

/// One screencast frame, base64 PNG as Chrome sent it, to the PNG's bytes.
pub fn decoded(data: &str) -> Result<Vec<u8>, base64::DecodeError> {
    base64::engine::general_purpose::STANDARD.decode(data)
}

/// The width and height a PNG's header gives, when it has one.
fn sides(png: &[u8]) -> Option<(u32, u32)> {
    let side = |at: usize| {
        let four: [u8; 4] = png.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(u32::from_be_bytes(four)).filter(|side| *side > 0)
    };
    Some((side(16)?, side(20)?))
}

/// Where the pointer is on the page and whether its button is down, and the two sizes a
/// point is turned by: the last picture's pixels and the page's CSS pixels.
#[derive(Debug, Default)]
struct Held {
    at: (u32, u32),
    down: bool,
    picture: Option<(u32, u32)>,
    css: (u32, u32),
}

impl Held {
    /// A point of the picture as a point of the page: frames can come at the CSS size or at
    /// the device's, so the ratio is read from the last frame.
    fn page(&self, x: u32, y: u32) -> (u32, u32) {
        let Some((pw, ph)) = self.picture else {
            return (x, y);
        };
        let turn = |v: u32, css: u32, picture: u32| {
            let scaled = u64::from(v).saturating_mul(u64::from(css)) / u64::from(picture.max(1));
            u32::try_from(scaled).unwrap_or(u32::MAX)
        };
        (turn(x, self.css.0, pw), turn(y, self.css.1, ph))
    }

    /// How many picture pixels one CSS pixel is; one before the first frame.
    fn ratio(&self) -> f64 {
        let wide = self.picture.map_or(self.css.0, |(wide, _)| wide);
        f64::from(wide.max(1)) / f64::from(self.css.0.max(1))
    }

    fn mouse(&self, kind: MouseKind, left: bool, buttons: u8) -> MouseInput {
        MouseInput {
            kind,
            x: self.at.0,
            y: self.at.1,
            button: if left {
                MouseButton::Left
            } else {
                MouseButton::None
            },
            buttons,
            click_count: u8::from(left && kind != MouseKind::MouseMoved),
            delta_x: 0,
            delta_y: 0,
            modifiers: 0,
        }
    }

    /// Hands one input of the screen to the page: a press and release as a click, a wheel as
    /// a scroll where the pointer last was, a named key as its down and up.
    fn send(&mut self, page: &mut impl Page, input: ScreenInput) {
        match input {
            ScreenInput::Pointer { phase, x, y } => {
                self.at = self.page(x, y);
                let said = match phase {
                    Pointer::Press => {
                        self.down = true;
                        self.mouse(MouseKind::MousePressed, true, 1)
                    }
                    Pointer::Move => {
                        self.mouse(MouseKind::MouseMoved, self.down, u8::from(self.down))
                    }
                    Pointer::Release => {
                        self.down = false;
                        self.mouse(MouseKind::MouseReleased, true, 0)
                    }
                };
                page.mouse(&said);
            }
            // The screen says how far the content moves; Chrome is told how far to scroll.
            ScreenInput::Wheel { dx, dy } => page.mouse(&MouseInput {
                delta_x: dx.saturating_neg(),
                delta_y: dy.saturating_neg(),
                ..self.mouse(MouseKind::MouseWheel, false, 0)
            }),
            ScreenInput::Text(text) => page.text(&cut(&text)),
            ScreenInput::Key(key) => {
                let (name, text, code) = named(key);
                let down = KeyInput {
                    kind: KeyKind::KeyDown,
                    key: name.to_owned(),
                    text: text.map(str::to_owned),
                    windows_virtual_key_code: code,
                    modifiers: 0,
                };
                page.key(&down);
                page.key(&KeyInput {
                    kind: KeyKind::KeyUp,
                    text: None,
                    ..down
                });
            }
        }
    }
}

/// A named key as Chrome's input calls it: its name, the text it types, its key code.
fn named(key: ScreenKey) -> (&'static str, Option<&'static str>, u32) {
    match key {
        ScreenKey::Enter => ("Enter", Some("\r"), 13),
        ScreenKey::Backspace => ("Backspace", None, 8),
        ScreenKey::Tab => ("Tab", None, 9),
        ScreenKey::Escape => ("Escape", None, 27),
        ScreenKey::Left => ("ArrowLeft", None, 37),
        ScreenKey::Up => ("ArrowUp", None, 38),
        ScreenKey::Right => ("ArrowRight", None, 39),
        ScreenKey::Down => ("ArrowDown", None, 40),
        ScreenKey::Delete => ("Delete", None, 46),
    }
}

#[cfg(test)]
mod design;
#[cfg(test)]
mod tests;
