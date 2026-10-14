//! The browser pane: a one-row URL bar over a headless Chrome page whose screencast PNGs ride
//! the kitty graphics protocol undecoded, and the mapping of terminal input onto CDP input.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Position, Rect};
use ratatui::text::Line;
use yi_browser::{Browser, Update};
use yi_tui::colors::Theme;
use yi_types::cdp::{KeyInput, KeyKind, MouseButton, MouseInput, MouseKind as CdpMouse};

use crate::app::{App, MouseKind};
use crate::layout::PaneId;
use crate::model::PaneContent;

/// Invariant: avatars count up from 8000 and orbs sit at 7800 plus two per pane; frame ids
/// start far above both, so neither can reach one (#806).
const FRAME_ID_BASE: u32 = 0x00F0_0000;
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
const BAR: &str = "‹ › ↻ ";

pub enum Engine {
    Idle,
    /// The `[…]` row shown while `yi plugin add chrome` runs.
    Installing(String),
    Live(Box<Browser>),
    Failed(String),
}

/// Where `yi plugin add chrome` puts its headless shell, from the composition root: the plugin
/// table is not the console's to read.
#[derive(Debug, Clone)]
pub struct Pinned {
    pub program: PathBuf,
    pub version: String,
    pub dir: PathBuf,
}

/// The console's one source of Chrome: the configured path, the pin, and the one install of
/// it every browser pane waits on.
#[derive(Default)]
pub struct Chrome {
    configured: Option<PathBuf>,
    pinned: Option<Pinned>,
    installer: Option<PathBuf>,
    install: Install,
}

#[derive(Default)]
enum Install {
    #[default]
    Idle,
    Running(Receiver<Result<(), String>>),
    Done(Result<(), String>),
}

impl Chrome {
    pub fn new(configured: Option<PathBuf>, pinned: Option<Pinned>) -> Self {
        Self {
            configured,
            pinned,
            installer: std::env::current_exe().ok(),
            install: Install::Idle,
        }
    }

    /// No Chrome found: a configured path or a target with no pin is a row naming the fix;
    /// otherwise the pane waits on the install, started here unless one is running.
    fn missing(&mut self, looked: &[String]) -> Engine {
        let pinned = self.pinned.as_ref().filter(|_| self.configured.is_none());
        let Some(pinned) = pinned else {
            return Engine::Failed(format!(
                "no Chrome or Chromium found · looked in {} · install one, or set browser.path \
                 in ~/.yi/config.json",
                looked.join(", ")
            ));
        };
        if matches!(self.install, Install::Done(Ok(()))) {
            return Engine::Failed(format!(
                "`yi plugin add chrome` finished, yet {} is not there",
                pinned.program.display()
            ));
        }
        let row = format!(
            "[installing chrome-headless-shell {} (~100 MB) with `yi plugin add chrome` · {}]",
            pinned.version,
            pinned.dir.display()
        );
        if !matches!(self.install, Install::Running(_)) {
            self.install = Install::Running(spawn_install(self.installer.clone()));
        }
        Engine::Installing(row)
    }

    fn installed(&mut self) -> Option<Result<(), String>> {
        if let Install::Running(outcome) = &self.install {
            match outcome.try_recv() {
                Ok(done) => self.install = Install::Done(done),
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => {
                    self.install = Install::Done(Err("the install stopped without a word".into()));
                }
            }
        }
        match &self.install {
            Install::Done(done) => Some(done.clone()),
            Install::Idle | Install::Running(_) => Some(Ok(())),
        }
    }
}

/// Runs [`yi_browser::add_pinned`] off the frame loop.
fn spawn_install(yi: Option<PathBuf>) -> Receiver<Result<(), String>> {
    let (done, outcome) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let result = match yi {
            Some(yi) => yi_browser::add_pinned(&yi),
            None => Err("couldn't find the running yi to install Chrome with".into()),
        };
        let _ = done.send(result);
    });
    outcome
}

pub struct BrowserPane {
    url: String,
    editing: Option<String>,
    engine: Engine,
    frame: Option<(u64, String)>,
    notice: Option<String>,
    page: Rect,
    size: (u32, u32),
    clicks: Clicks,
}

impl BrowserPane {
    /// With no URL the bar opens for typing one.
    pub fn new(url: Option<String>) -> Self {
        Self {
            editing: url.is_none().then(String::new),
            url: url.unwrap_or_else(|| "about:blank".to_owned()),
            engine: Engine::Idle,
            frame: None,
            notice: None,
            page: Rect::default(),
            size: (0, 0),
            clicks: Clicks::default(),
        }
    }

    pub fn view(
        &mut self,
        inner: Rect,
        kitty: bool,
        theme: &Theme,
    ) -> (String, Vec<Line<'static>>) {
        self.page = Rect {
            y: inner.y.saturating_add(1),
            height: inner.height.saturating_sub(1),
            ..inner
        };
        let bar = match &self.editing {
            Some(text) => Line::styled(format!("{BAR}{text}▏"), theme.accent_style()),
            None => Line::styled(format!("{BAR}{}", self.url), theme.dim_style()),
        };
        let status = match (&self.engine, kitty, &self.frame) {
            (Engine::Failed(why), ..) => Some(format!("[browser: {why}]")),
            (Engine::Installing(row), ..) => Some(row.clone()),
            (_, false, _) => Some(
                "[page not drawn: this terminal does not speak the kitty graphics protocol · \
                 open the console in Ghostty or kitty]"
                    .to_owned(),
            ),
            (_, true, _) if self.notice.is_some() => self.notice.clone(),
            (_, true, None) => Some("loading…".to_owned()),
            (_, true, Some(_)) => None,
        };
        let mut lines = vec![bar];
        let width = usize::from(inner.width);
        for row in status
            .iter()
            .flat_map(|text| crate::render::wrap(text, width))
        {
            lines.push(Line::styled(row, theme.dim_style()));
        }
        let host = self.url.split("://").last().unwrap_or_default();
        (
            format!("◎ {}", host.chars().take(24).collect::<String>()),
            lines,
        )
    }

    /// Starts Chrome once the page has a size, follows it, and drains its updates; true when
    /// the pane needs a redraw.
    pub fn pump(&mut self, kitty: bool, cell: (usize, usize), chrome: &mut Chrome) -> bool {
        if !kitty || self.page.width == 0 || self.page.height == 0 {
            return false;
        }
        let size = css_size(self.page, cell);
        match &mut self.engine {
            Engine::Failed(_) => false,
            Engine::Installing(_) => match chrome.installed() {
                None => false,
                Some(Ok(())) => {
                    self.engine = Engine::Idle;
                    true
                }
                Some(Err(why)) => {
                    self.engine = Engine::Failed(why);
                    true
                }
            },
            Engine::Idle => {
                self.size = size;
                self.engine = launch(chrome, &self.url, size);
                true
            }
            Engine::Live(browser) => {
                if size != self.size {
                    self.size = size;
                    browser.router.resize(size);
                }
                let updates = browser.poll();
                let dirty = !updates.is_empty();
                self.absorb(updates);
                dirty
            }
        }
    }

    fn absorb(&mut self, updates: Vec<Update>) {
        let mut frames = Vec::new();
        for update in updates {
            match update {
                Update::Frame(png) => frames.push(png),
                Update::Url(url) => {
                    self.url = url;
                    self.notice = None;
                }
                Update::NavigateFailed(why) => {
                    self.notice = Some(format!("[couldn't open the page: {why}]"));
                }
                Update::Failed(why) => self.engine = Engine::Failed(why),
            }
        }
        // Only the newest frame is shown, so only it is scanned; an older one stands in only
        // when the newest is refused.
        if let Some(png) = frames
            .into_iter()
            .rev()
            .find(|png| yi_types::image::is_strict_base64(png))
        {
            let seq = self
                .frame
                .as_ref()
                .map_or(0, |(seq, _)| seq.wrapping_add(1));
            self.frame = Some((seq, png));
        }
    }

    fn live(&mut self) -> Option<&mut Browser> {
        match &mut self.engine {
            Engine::Live(browser) => Some(browser),
            Engine::Idle | Engine::Installing(_) | Engine::Failed(_) => None,
        }
    }

    /// Keys the page or the bar takes; ⌘, ⌥, ctrl+b and ctrl+c stay the console's.
    pub fn key(&mut self, key: &KeyEvent) -> bool {
        let reserved = KeyModifiers::ALT | KeyModifiers::SUPER;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.modifiers.intersects(reserved)
            || (ctrl && matches!(key.code, KeyCode::Char('b' | 'c')))
        {
            return false;
        }
        if ctrl && key.code == KeyCode::Char('l') {
            self.editing = Some(self.url.clone());
            return true;
        }
        let Some(text) = &mut self.editing else {
            for input in key_inputs(key) {
                if let Some(browser) = self.live() {
                    browser.router.key(&input);
                }
            }
            return true;
        };
        match key.code {
            KeyCode::Char(c) => text.push(c),
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Esc => self.editing = None,
            KeyCode::Enter => {
                let url = typed_url(text);
                self.editing = None;
                self.url.clone_from(&url);
                if let Some(browser) = self.live() {
                    browser.router.navigate(&url);
                }
            }
            _ => {}
        }
        true
    }

    pub fn paste(&mut self, text: &str) {
        match &mut self.editing {
            Some(bar) => bar.push_str(text),
            None => {
                if let Some(browser) = self.live() {
                    browser.router.insert_text(text);
                }
            }
        }
    }

    /// A mouse event at a cell of this pane: the bar's three marks and its text, or the page.
    pub fn mouse(&mut self, kind: MouseKind, x: u16, y: u16, cell: (usize, usize)) -> bool {
        let on_bar =
            y.checked_add(1) == Some(self.page.y) && (self.page.x..self.page.right()).contains(&x);
        if on_bar && kind == MouseKind::Down {
            let column = x.saturating_sub(self.page.x);
            match (column / 2, self.live()) {
                (0, Some(browser)) => browser.router.go(-1),
                (1, Some(browser)) => browser.router.go(1),
                (2, Some(browser)) => browser.router.reload(),
                _ => self.editing = Some(self.url.clone()),
            }
            return true;
        }
        let Some(point) = css_point(self.page, cell, x, y) else {
            return false;
        };
        let input = match kind {
            MouseKind::Down => {
                self.editing = None;
                let count = self.clicks.press((x, y), Instant::now());
                button(CdpMouse::MousePressed, point, count, 1)
            }
            MouseKind::Up => button(CdpMouse::MouseReleased, point, self.clicks.count, 0),
            MouseKind::Drag => button(CdpMouse::MouseMoved, point, 0, 1),
            MouseKind::Moved => MouseInput {
                button: MouseButton::None,
                ..button(CdpMouse::MouseMoved, point, 0, 0)
            },
            MouseKind::ScrollUp | MouseKind::ScrollDown => wheel(kind, point, cell),
            MouseKind::RightDown => return false,
        };
        if let Some(browser) = self.live() {
            browser.router.mouse(&input);
        }
        true
    }
}

fn launch(chrome: &mut Chrome, url: &str, size: (u32, u32)) -> Engine {
    let pinned = chrome
        .pinned
        .as_ref()
        .map(|pinned| pinned.program.as_path());
    let chrome = match yi_browser::find(chrome.configured.as_deref(), pinned) {
        Ok(found) => found,
        Err(looked) => return chrome.missing(&looked),
    };
    let launched = std::env::current_exe()
        .and_then(|yi| Browser::launch(&yi, &chrome, &yi_browser::profile_dir(), url, size));
    match launched {
        Ok(browser) => Engine::Live(Box::new(browser)),
        Err(error) => Engine::Failed(format!("{}: {error}", chrome.display())),
    }
}

/// A bare host gets `http://`, the scheme a local dev server speaks.
fn typed_url(text: &str) -> String {
    let text = text.trim();
    if text.contains("://") || text.starts_with("about:") {
        text.to_owned()
    } else {
        format!("http://{text}")
    }
}

/// The last `http://localhost:*` or `http://127.0.0.1:*` URL in a transcript, as printed; the
/// host is read from the authority, so userinfo cannot move the page off this machine.
pub fn last_local_url<'a>(sources: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut last = None;
    for source in sources {
        for word in source.split(|c: char| c.is_whitespace() || "<>()[]`'\"".contains(c)) {
            let word = word.trim_end_matches(['.', ',', ';']);
            let probe = word.trim_end_matches('\\');
            let local =
                yi_types::url::http_loopback_host_and_port(probe).is_some_and(|(_, port)| {
                    port.is_some_and(|port| {
                        !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())
                    })
                });
            if local {
                last = Some(word.to_owned());
            }
        }
    }
    last
}

pub fn css_size(page: Rect, (width, height): (usize, usize)) -> (u32, u32) {
    let px = |cells: u16, cell: usize| {
        u32::try_from(usize::from(cells).saturating_mul(cell)).unwrap_or(u32::MAX)
    };
    (px(page.width, width), px(page.height, height))
}

/// The centre of cell (x, y) in CSS pixels from the page's corner; `None` off the page.
pub fn css_point(
    page: Rect,
    (width, height): (usize, usize),
    x: u16,
    y: u16,
) -> Option<(u32, u32)> {
    if !page.contains(Position::new(x, y)) {
        return None;
    }
    let at = |offset: u16, cell: usize| {
        let px = usize::from(offset)
            .saturating_mul(cell)
            .saturating_add(cell / 2);
        u32::try_from(px).unwrap_or(u32::MAX)
    };
    Some((
        at(x.saturating_sub(page.x), width),
        at(y.saturating_sub(page.y), height),
    ))
}

fn button(kind: CdpMouse, (x, y): (u32, u32), click_count: u8, buttons: u8) -> MouseInput {
    MouseInput {
        kind,
        x,
        y,
        button: MouseButton::Left,
        buttons,
        click_count,
        delta_x: 0,
        delta_y: 0,
        modifiers: 0,
    }
}

/// One notch scrolls three rows of the terminal's own cell height, as a chat pane does.
pub fn wheel(kind: MouseKind, point: (u32, u32), (_, height): (usize, usize)) -> MouseInput {
    let rows = i32::try_from(height.saturating_mul(3)).unwrap_or(i32::MAX);
    MouseInput {
        button: MouseButton::None,
        delta_y: if kind == MouseKind::ScrollUp {
            -rows
        } else {
            rows
        },
        ..button(CdpMouse::MouseWheel, point, 0, 0)
    }
}

#[derive(Debug, Default)]
pub struct Clicks {
    last: Option<((u16, u16), Instant)>,
    count: u8,
}

impl Clicks {
    /// A press on the same cell within 400 ms counts on, up to a triple click.
    pub fn press(&mut self, at: (u16, u16), now: Instant) -> u8 {
        let again = self.last.is_some_and(|(cell, when)| {
            cell == at && now.saturating_duration_since(when) <= DOUBLE_CLICK
        });
        self.count = if again {
            self.count.saturating_add(1).min(3)
        } else {
            1
        };
        self.last = Some((at, now));
        self.count
    }
}

/// A key as Chrome's keyDown then keyUp; ctrl and shift ride along as the CDP bitmask.
pub fn key_inputs(key: &KeyEvent) -> Vec<KeyInput> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let modifiers = u8::from(ctrl) * 2 + u8::from(key.modifiers.contains(KeyModifiers::SHIFT)) * 8;
    let named = |name: &str, vk: u32| (name.to_owned(), None, vk);
    let (name, text, vk) = match key.code {
        KeyCode::Char(c) => {
            let upper = c.to_ascii_uppercase();
            let typed = c.is_ascii_alphanumeric() || c == ' ';
            let vk = if typed { u32::from(upper) } else { 0 };
            (c.to_string(), (!ctrl).then(|| c.to_string()), vk)
        }
        KeyCode::Enter => ("Enter".to_owned(), Some("\r".to_owned()), 13),
        KeyCode::Backspace => named("Backspace", 8),
        KeyCode::Tab => named("Tab", 9),
        KeyCode::Esc => named("Escape", 27),
        KeyCode::PageUp => named("PageUp", 33),
        KeyCode::PageDown => named("PageDown", 34),
        KeyCode::End => named("End", 35),
        KeyCode::Home => named("Home", 36),
        KeyCode::Left => named("ArrowLeft", 37),
        KeyCode::Up => named("ArrowUp", 38),
        KeyCode::Right => named("ArrowRight", 39),
        KeyCode::Down => named("ArrowDown", 40),
        KeyCode::Delete => named("Delete", 46),
        _ => return Vec::new(),
    };
    let down = KeyInput {
        kind: KeyKind::KeyDown,
        key: name,
        text,
        windows_virtual_key_code: vk,
        modifiers,
    };
    let up = KeyInput {
        kind: KeyKind::KeyUp,
        text: None,
        ..down.clone()
    };
    vec![down, up]
}

fn frame_ids(pane: PaneId) -> [u32; 2] {
    let base = FRAME_ID_BASE.saturating_add(pane.raw().saturating_mul(2));
    [base, base.saturating_add(1)]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    rect: Rect,
    seq: u64,
    id: u32,
}

/// Every browser page on the tab, uncovered and with a frame, placed over its rect; a new
/// frame goes to the pane's other id before the old one is deleted, so nothing blinks.
pub fn place(app: &mut App, out: &mut impl Write) {
    let mut want: BTreeMap<PaneId, (Rect, u64, &str)> = BTreeMap::new();
    if let Some(hits) = &app.hits {
        for (id, _) in &hits.panes {
            let Some(PaneContent::Browser(pane)) = app.state.panes.get(id).map(|p| &p.content)
            else {
                continue;
            };
            let covered = hits.covered(pane.page);
            if let (Some((seq, png)), false) = (&pane.frame, covered || pane.page.is_empty()) {
                want.insert(*id, (pane.page, *seq, png.as_str()));
            }
        }
    }
    let placed = &mut app.state.browser_placed;
    placed.retain(|pane, at| {
        let keep = want.contains_key(pane);
        if !keep {
            let _ = yi_tui::orb::kitty::delete_id(out, at.id);
        }
        keep
    });
    for (pane, (rect, seq, png)) in want {
        let old = placed.get(&pane).copied();
        if old.is_some_and(|old| old.rect == rect && old.seq == seq) {
            continue;
        }
        let [even, odd] = frame_ids(pane);
        let id = if old.is_some_and(|old| old.id == even) {
            odd
        } else {
            even
        };
        let painted = crate::kitty::transmit_base64(out, id, png).is_ok()
            && yi_tui::orb::kitty::place(out, id, rect.x, rect.y, rect.width, rect.height).is_ok();
        if !painted {
            continue;
        }
        if let Some(old) = old {
            let _ = yi_tui::orb::kitty::delete_id(out, old.id);
        }
        placed.insert(pane, Placed { rect, seq, id });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame reaches kitty as an f=100 transmit on the pane's id and one placement over the
    /// page; the next frame swaps to the other id and deletes the first.
    #[test]
    fn a_new_frame_swaps_ids_and_a_closed_pane_deletes_its_image()
    -> Result<(), Box<dyn std::error::Error>> {
        let theme = Theme::new(yi_tui::colors::ColorTier::TrueColor, true);
        let mut app = App::new("/repo".to_owned(), theme);
        let pane = app.state.focused_pane_id().ok_or("a pane")?;
        let mut browser = BrowserPane::new(Some("http://localhost:1/".to_owned()));
        browser.page = Rect::new(2, 3, 20, 8);
        browser.frame = Some((0, "iVBORw0KGgo=".to_owned()));
        if let Some(slot) = app.state.panes.get_mut(&pane) {
            slot.content = PaneContent::Browser(Box::new(browser));
        }
        app.hits = Some(crate::render::Hits {
            panes: vec![(pane, Rect::new(0, 0, 40, 20))],
            ..crate::render::Hits::default()
        });
        let [even, odd] = frame_ids(pane);
        let mut out = Vec::new();
        place(&mut app, &mut out);
        let first = String::from_utf8_lossy(&out).into_owned();
        assert!(first.contains(&format!("f=100,a=t,i={even},")), "{first:?}");
        assert!(
            first.contains(&format!("a=p,i={even},p=1,q=2,C=1,c=20,r=8")),
            "{first:?}"
        );
        out.clear();
        place(&mut app, &mut out);
        assert!(out.is_empty(), "an unchanged frame writes nothing");
        if let Some(PaneContent::Browser(browser)) =
            app.state.panes.get_mut(&pane).map(|p| &mut p.content)
        {
            browser.frame = Some((1, "iVBORw0KGgo=".to_owned()));
        }
        place(&mut app, &mut out);
        let second = String::from_utf8_lossy(&out).into_owned();
        assert!(second.contains(&format!("a=t,i={odd},")), "{second:?}");
        assert!(
            second.ends_with(&format!("a=d,d=I,i={even},q=2\u{1b}\\")),
            "{second:?}"
        );
        out.clear();
        app.hits = Some(crate::render::Hits::default());
        place(&mut app, &mut out);
        assert_eq!(
            String::from_utf8_lossy(&out),
            format!("\u{1b}_Ga=d,d=I,i={odd},q=2\u{1b}\\")
        );
        Ok(())
    }

    #[test]
    fn a_navigation_failure_is_a_notice_over_a_live_engine()
    -> Result<(), Box<dyn std::error::Error>> {
        let theme = Theme::new(yi_tui::colors::ColorTier::TrueColor, true);
        let mut pane = BrowserPane::new(Some("http://localhost:1/".to_owned()));
        pane.engine = Engine::Idle;
        pane.absorb(vec![Update::NavigateFailed("DNS: not found".to_owned())]);
        assert!(
            !matches!(pane.engine, Engine::Failed(_)),
            "a navigation failure never fails the engine"
        );
        let (_, lines) = pane.view(Rect::new(0, 0, 60, 6), true, &theme);
        let drawn: String = lines.iter().map(|line| line.to_string()).collect();
        assert!(
            drawn.contains("[couldn't open the page: DNS: not found]"),
            "{drawn:?}"
        );
        pane.absorb(vec![Update::Url("http://ok/".to_owned())]);
        let (_, lines) = pane.view(Rect::new(0, 0, 60, 6), true, &theme);
        let drawn: String = lines.iter().map(|line| line.to_string()).collect();
        assert!(!drawn.contains("couldn't open the page"), "{drawn:?}");
        Ok(())
    }

    /// Incident (#817): a payload outside base64 can end a kitty escape and start another.
    /// A drain's newest frame is the one shown; when it is refused, the frame before it stands.
    #[test]
    fn a_refused_newest_frame_leaves_the_one_before_it_on_screen()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut pane = BrowserPane::new(Some("http://localhost:1/".to_owned()));
        let pixel = "iVBORw0KGgo=";
        let refused = "AAAA\u{1b}\\\u{1b}]52;c;ZWNobyBwd25lZA==\u{7}";
        pane.absorb(vec![
            Update::Frame("AAAA".to_owned()),
            Update::Frame(pixel.to_owned()),
            Update::Frame(refused.to_owned()),
        ]);
        assert_eq!(
            pane.frame.as_ref().map(|(_, png)| png.as_str()),
            Some(pixel)
        );
        Ok(())
    }

    /// No Chrome on a machine with a pin: every browser pane shows the install row while one
    /// `yi plugin add chrome` runs, then the install's own error line.
    #[test]
    fn a_missing_chrome_runs_one_install_and_shows_its_row_then_its_error()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("yi-chrome-install-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let stub = dir.join("yi");
        std::fs::write(
            &stub,
            "#!/bin/sh\necho \"$@\" >> \"$0.calls\"\nwhile [ ! -e \"$0.go\" ]; do sleep 0.01; done\n\
             echo 'downloading chrome 155.0.8059.39' >&2\n\
             echo 'error: couldn'\\''t download chrome 155.0.8059.39: refused' >&2\nexit 1\n",
        )?;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755))?;
        let mut chrome = Chrome {
            configured: None,
            pinned: Some(Pinned {
                program: dir.join("absent/chrome-headless-shell"),
                version: "155.0.8059.39".to_owned(),
                dir: dir.join("plugins/chrome/155.0.8059.39"),
            }),
            installer: Some(stub.clone()),
            install: Install::Idle,
        };
        let theme = Theme::new(yi_tui::colors::ColorTier::TrueColor, true);
        let looked = ["/Applications/Google Chrome.app".to_owned()];
        let mut panes = [BrowserPane::new(None), BrowserPane::new(None)];
        for pane in &mut panes {
            pane.engine = chrome.missing(&looked);
        }
        let row = format!(
            "[installing chrome-headless-shell 155.0.8059.39 (~100 MB) with `yi plugin add \
             chrome` · {}]",
            dir.join("plugins/chrome/155.0.8059.39").display()
        );
        let shown = |pane: &mut BrowserPane| -> String {
            let (_, lines) = pane.view(Rect::new(0, 0, 400, 6), true, &theme);
            lines.get(1).map(ToString::to_string).unwrap_or_default()
        };
        for pane in &mut panes {
            assert_eq!(shown(pane), row);
            assert!(
                !pane.pump(true, (8, 16), &mut chrome),
                "nothing changes mid-install"
            );
        }
        std::fs::write(dir.join("yi.go"), "")?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while panes
            .iter()
            .any(|pane| matches!(pane.engine, Engine::Installing(_)))
        {
            if Instant::now() > deadline {
                return Err("the install never finished".into());
            }
            for pane in &mut panes {
                pane.pump(true, (8, 16), &mut chrome);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        for pane in &mut panes {
            assert_eq!(
                shown(pane),
                "[browser: error: couldn't download chrome 155.0.8059.39: refused]"
            );
        }
        assert_eq!(
            std::fs::read_to_string(dir.join("yi.calls"))?,
            "plugin add chrome\n",
            "two panes, one install"
        );
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }
}
