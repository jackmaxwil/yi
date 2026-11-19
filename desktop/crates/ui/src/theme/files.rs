//! Theme files on disk: read, reload when one changes, write.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::SystemTime;

use rune::{Spring, Tx};
use yd_core::findings::triage::replace;
use yd_core::ids::ThemeId;
use yd_core::msg::Msg;
use yd_core::tokens::{ThemeFile, ThemeValue, TokenError, Tokens};

use crate::disk::Disk;
use crate::theme::scale::{Fallback, check, load};
use crate::win::{Hub, Win, dispatch};

/// A theme file larger than this is not read: a theme is a few dozen values.
pub const MAX_BYTES: u64 = 256 * 1024;
/// At most this many files of the folder are read.
pub const MAX_FILES: usize = 256;
/// How often the folder is looked at, in seconds.
pub const POLL: f64 = 1.0;
/// A theme change blends every colour on this spring.
pub const BLEND: Spring = Spring::new(0.35, 0.0);
/// The theme the app starts in and falls back to.
pub const BASE: &str = "yi";

/// Built-in themes: the file, and why one cannot be picked.
const BUILT_IN: &[(&str, Option<&str>)] = &[
    (
        r#"{"id": "yi", "name": "Yi Dark", "note": "yours · the default", "dark": true}"#,
        None,
    ),
    (
        r##"{"id": "graphite", "name": "Graphite", "note": "grey, nearly solid panes", "dark": true,
        "values": {"window": "#1b1c1f", "text.1": "#d5d6da", "text.2": "#a7a9b0",
        "text.3": "#82858d", "accent": "#a9b0bd", "needs_you": "#c9cdd6", "ok": "#9fc7a8",
        "code.keyword": "#c3c6ce", "code.type": "#a9b0bd", "code.string": "#b9c4b0",
        "code.number": "#c9b9c4", "glass.pane": 0.62, "glass.pane_in_use": 0.74,
        "glass.window": 0.7}}"##,
        None,
    ),
    (
        r##"{"id": "ink", "name": "Ink", "note": "deep blue with violet", "dark": true,
        "values": {"window": "#0f1224", "text.1": "#d9dbef", "text.2": "#aeb2d0",
        "text.3": "#8387a8", "accent": "#a79cf0", "needs_you": "#7fb8f0",
        "code.keyword": "#c9a7f2", "code.type": "#8fb8f5", "code.string": "#9fd8c0",
        "code.number": "#f0a8c8", "glass.pane": 0.34, "glass.pane_in_use": 0.46,
        "glass.window": 0.3}}"##,
        None,
    ),
    (
        r##"{"id": "terminal", "name": "Terminal", "note": "code font everywhere", "dark": true,
        "values": {"window": "#0d120f", "text.1": "#cfe0d0", "text.2": "#a3b8a6",
        "text.3": "#7b8f7f", "accent": "#8fd3a6", "needs_you": "#d8dfa0",
        "code.keyword": "#9fd8c0", "code.type": "#cfe0d0", "code.string": "#b8d99a",
        "code.number": "#d8c0e0", "font.text": "Geist Mono", "glass.pane": 0.5,
        "glass.pane_in_use": 0.62, "glass.window": 0.5}}"##,
        None,
    ),
    (
        r##"{"id": "contrast", "name": "High contrast", "note": "for bright rooms", "dark": true,
        "values": {"window": "#000000", "text.1": "#ffffff", "text.2": "#e4e6ee",
        "text.3": "#bcc0cc", "accent": "#9cc2ff", "needs_you": "#7ff0e0", "glass.pane": 0.82,
        "glass.pane_in_use": 0.92, "glass.window": 0.85}}"##,
        None,
    ),
    (
        r##"{"id": "paper", "name": "Paper", "note": "light · listed, not drawn on this page",
        "dark": false, "values": {"window": "#f3f2ee", "text.1": "#1c1d20", "accent": "#2b67d9",
        "needs_you": "#0f8f86", "code.keyword": "#b0407a"}}"##,
        Some("a light theme: the app has no light base to draw it on yet"),
    ),
];

/// A file's modified time and length when it was last read or written.
pub type Seen = Option<(SystemTime, u64)>;

/// Where a theme came from.
#[derive(Debug, Clone, PartialEq)]
pub enum Origin {
    BuiltIn,
    /// A copy with no folder to keep it in: it lasts this run.
    Unsaved,
    /// A file in the themes folder, with the modified time and length last read.
    File {
        path: PathBuf,
        seen: Seen,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub file: ThemeFile,
    pub origin: Origin,
    /// Why it cannot be picked, for a theme that is listed but not drawn.
    pub unavailable: Option<&'static str>,
    /// Edited since it was last written to its file.
    dirty: bool,
}

impl Theme {
    pub fn id(&self) -> &ThemeId {
        &self.file.id
    }

    /// The base with this theme's values set.
    pub fn tokens(&self) -> Tokens {
        Tokens::themed(&self.file).0
    }

    /// Window, bright text, working, needs you and keyword: the five a gallery row shows.
    pub fn swatches(&self) -> [yd_core::tokens::Rgba; 5] {
        let t = self.tokens();
        [t.window, t.fg, t.accent, t.need, t.keyword]
    }

    fn path(&self) -> Option<&Path> {
        match &self.origin {
            Origin::File { path, .. } => Some(path),
            Origin::BuiltIn | Origin::Unsaved => None,
        }
    }
}

/// Why a file in the themes folder was not taken.
#[derive(Debug, Clone, PartialEq)]
pub enum FileError {
    Read(std::io::ErrorKind),
    TooLarge { bytes: u64, max: u64 },
    NotATheme,
    IdTaken(ThemeId),
    Write(String),
}

/// What was refused: a whole file, or one value in it, which keeps the value it had.
#[derive(Debug, Clone, PartialEq)]
pub enum Refused {
    File(FileError),
    Value { name: String, why: TokenError },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub path: PathBuf,
    pub what: Refused,
}

impl Refusal {
    /// One line for the user: the file, what was refused and why.
    pub fn words(&self) -> String {
        let file = self
            .path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        match &self.what {
            Refused::File(FileError::Read(kind)) => format!("{file}: cannot be read ({kind})"),
            Refused::File(FileError::TooLarge { bytes, max }) => {
                format!("{file}: {bytes} bytes, over the {max} a theme may take")
            }
            Refused::File(FileError::NotATheme) => format!("{file}: not a theme file"),
            Refused::File(FileError::IdTaken(id)) => format!("{file}: the id {id} is taken"),
            Refused::File(FileError::Write(why)) => format!("{file}: not saved: {why}"),
            Refused::Value { name, why } => format!("{file}: {name} kept as it was: {why}"),
        }
    }
}

/// Every theme the app knows, the one in use, and what was refused.
#[derive(Debug, Clone, PartialEq)]
pub struct Library {
    pub themes: Vec<Theme>,
    pub active: ThemeId,
    pub refusals: Vec<Refusal>,
    folder: Option<PathBuf>,
    looked: Option<f64>,
    /// Whether writes wait in `out` for a worker, as the running app's do, or are done at once.
    defer: bool,
    /// Files to write, each with its whole text.
    out: Vec<(PathBuf, String)>,
    /// Files with a write under way. Invariant: one is not read again until its write is
    /// heard of, or the reload would put an older text back over a newer edit.
    writing: Vec<PathBuf>,
}

impl Default for Library {
    fn default() -> Self {
        let themes = BUILT_IN
            .iter()
            .filter_map(|(text, unavailable)| {
                Some(Theme {
                    file: ThemeFile::parse(text).ok()?,
                    origin: Origin::BuiltIn,
                    unavailable: *unavailable,
                    dirty: false,
                })
            })
            .collect();
        Self {
            themes,
            active: ThemeId::new(BASE),
            refusals: Vec::new(),
            folder: None,
            looked: None,
            defer: false,
            out: Vec::new(),
            writing: Vec::new(),
        }
    }
}

impl Library {
    /// The built-in themes and the files in `folder`.
    pub fn at(folder: PathBuf) -> Self {
        let mut library = Self {
            folder: Some(folder),
            ..Self::default()
        };
        library.reload();
        library
    }

    pub fn theme(&self, id: &ThemeId) -> Option<&Theme> {
        self.themes.iter().find(|theme| theme.id() == id)
    }

    fn theme_mut(&mut self, id: &ThemeId) -> Option<&mut Theme> {
        self.themes.iter_mut().find(|theme| theme.id() == id)
    }

    /// The tokens of the theme in use; the base when it is gone.
    pub fn tokens(&self) -> Tokens {
        self.theme(&self.active)
            .map(Theme::tokens)
            .unwrap_or_default()
    }

    /// Makes `id` the theme in use. A theme that is unavailable or unknown is not taken.
    pub fn pick(&mut self, id: &ThemeId) -> bool {
        let fits = self.theme(id).is_some_and(|t| t.unavailable.is_none());
        if fits {
            self.active = id.clone();
        }
        fits
    }

    /// Takes the text of the file at `path`. A file that is not a theme keeps what was read
    /// from it before; a value that does not fit is refused by name and keeps its old value.
    pub fn take(&mut self, path: &Path, text: &str, seen: Option<(SystemTime, u64)>) {
        self.refusals.retain(|refusal| refusal.path != path);
        let refuse = |library: &mut Library, what| {
            library.refusals.push(Refusal {
                path: path.to_path_buf(),
                what,
            });
        };
        let Ok(mut file) = ThemeFile::parse(text) else {
            return refuse(self, Refused::File(FileError::NotATheme));
        };
        let taken = self
            .themes
            .iter()
            .any(|theme| theme.id() == &file.id && theme.path() != Some(path));
        if taken {
            return refuse(self, Refused::File(FileError::IdTaken(file.id)));
        }
        let old = self.themes.iter().position(|t| t.path() == Some(path));
        for fallback in load(text).fell_back {
            let Fallback::Refused { name, why } = fallback else {
                continue;
            };
            let kept = old
                .and_then(|at| self.themes.get(at))
                .and_then(|theme| theme.file.values.get(&name))
                .cloned();
            match kept {
                Some(value) => file.values.insert(name.clone(), value),
                None => file.values.remove(&name),
            };
            refuse(self, Refused::Value { name, why });
        }
        let theme = Theme {
            file,
            origin: Origin::File {
                path: path.to_path_buf(),
                seen,
            },
            unavailable: None,
            dirty: false,
        };
        match old.and_then(|at| self.themes.get_mut(at)) {
            Some(slot) => *slot = theme,
            None => self.themes.push(theme),
        }
    }

    /// Reads every theme file in the folder that changed since it was last read, and drops
    /// the themes whose file is gone. Edits not yet written are written first.
    pub fn reload(&mut self) {
        self.flush();
        let Some(folder) = self.folder.clone() else {
            return;
        };
        let mut listed: Vec<PathBuf> = std::fs::read_dir(&folder)
            .into_iter()
            .flatten()
            .filter_map(|entry| Some(entry.ok()?.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect();
        listed.sort();
        listed.truncate(MAX_FILES);
        for path in &listed {
            self.read(path);
        }
        let writing = &self.writing;
        self.themes.retain(|theme| match &theme.origin {
            Origin::File { path, .. } => listed.contains(path) || writing.contains(path),
            Origin::BuiltIn | Origin::Unsaved => true,
        });
        self.refusals
            .retain(|refusal| listed.contains(&refusal.path));
        if self.theme(&self.active).is_none() {
            self.active = ThemeId::new(BASE);
        }
    }

    fn read(&mut self, path: &Path) {
        if self.writing.iter().any(|written| written == path) {
            return;
        }
        let refuse = |library: &mut Library, why| {
            library.refusals.retain(|refusal| refusal.path != path);
            library.refusals.push(Refusal {
                path: path.to_path_buf(),
                what: Refused::File(why),
            });
        };
        let meta = match std::fs::metadata(path) {
            Ok(meta) => meta,
            Err(cause) => return refuse(self, FileError::Read(cause.kind())),
        };
        let seen = meta.modified().ok().map(|at| (at, meta.len()));
        let known = self.themes.iter().any(|theme| {
            theme.origin
                == Origin::File {
                    path: path.to_path_buf(),
                    seen,
                }
        });
        if known {
            return;
        }
        if meta.len() > MAX_BYTES {
            return refuse(
                self,
                FileError::TooLarge {
                    bytes: meta.len(),
                    max: MAX_BYTES,
                },
            );
        }
        match std::fs::read_to_string(path) {
            Ok(text) => self.take(path, &text, seen),
            Err(cause) => refuse(self, FileError::Read(cause.kind())),
        }
    }

    /// A copy of `id` to edit, named after it; written to the folder when there is one.
    pub fn copy(&mut self, id: &ThemeId) -> Option<ThemeId> {
        let source = self.theme(id)?.file.clone();
        let stem: String = source
            .id
            .as_str()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .take(48)
            .collect();
        let fresh = (1..=999)
            .map(|n| match n {
                1 => ThemeId::new(format!("{stem}-copy")),
                n => ThemeId::new(format!("{stem}-copy-{n}")),
            })
            .find(|fresh| self.theme(fresh).is_none())?;
        let origin = match &self.folder {
            Some(folder) => Origin::File {
                path: folder.join(format!("{fresh}.json")),
                seen: None,
            },
            None => Origin::Unsaved,
        };
        self.themes.push(Theme {
            file: ThemeFile {
                id: fresh.clone(),
                name: format!("{} copy", source.name),
                ..source
            },
            origin,
            unavailable: None,
            dirty: true,
        });
        self.flush();
        Some(fresh)
    }

    /// Sets one value of `id`. A built-in theme is not changed: the value goes to a new copy of
    /// it, which is put in use. Gives the theme that changed.
    pub fn set(
        &mut self,
        id: &ThemeId,
        name: &str,
        value: ThemeValue,
    ) -> Result<ThemeId, TokenError> {
        Tokens::default().set(name, &value)?;
        check(name, &value)?;
        let built_in = self.theme(id).map(|theme| theme.origin == Origin::BuiltIn);
        let id = match built_in {
            Some(true) => self.copy(id).unwrap_or_else(|| id.clone()),
            Some(false) | None => id.clone(),
        };
        if let Some(theme) = self.theme_mut(&id)
            && theme.origin != Origin::BuiltIn
        {
            theme.file.set(name, value);
            theme.dirty = true;
        }
        if self.active != id && self.theme(&id).is_some() {
            self.active = id.clone();
        }
        Ok(id)
    }

    /// Hands every edited theme's text to be written to its file: at once, or by the
    /// app's worker when writes are deferred.
    fn flush(&mut self) {
        for theme in self.themes.iter_mut().filter(|theme| theme.dirty) {
            theme.dirty = false;
            let Origin::File { path, .. } = &theme.origin else {
                continue;
            };
            match theme.file.to_text() {
                Ok(text) => {
                    self.writing.push(path.clone());
                    self.out.push((path.clone(), text));
                }
                Err(why) => self.refusals.push(Refusal {
                    path: path.clone(),
                    what: Refused::File(FileError::Write(why.to_string())),
                }),
            }
        }
        if !self.defer {
            self.save();
        }
    }

    /// Writes what waits to be written, here and now.
    pub fn save(&mut self) {
        for (path, text) in std::mem::take(&mut self.out) {
            let done = write(&path, &text);
            self.wrote(&path, done);
        }
    }

    /// Hears how one write of `path` went: the file as written is the file last seen, so
    /// it is not read back; a write that failed is said with the refusals.
    pub fn wrote(&mut self, path: &Path, done: Result<Seen, String>) {
        if let Some(at) = self.writing.iter().position(|written| written == path) {
            self.writing.swap_remove(at);
        }
        match done {
            Ok(written) => {
                let theme = self.themes.iter_mut().find(|t| t.path() == Some(path));
                if let Some(Origin::File { seen, .. }) = theme.map(|theme| &mut theme.origin) {
                    *seen = written;
                }
            }
            Err(why) => self.refusals.push(Refusal {
                path: path.to_path_buf(),
                what: Refused::File(FileError::Write(why)),
            }),
        }
    }
}

/// Writes `text` as the file at `path`, whole or not at all: to a copy beside it, synced,
/// then renamed over it. Gives the time and length the file then has.
pub fn write(path: &Path, text: &str) -> Result<Seen, String> {
    let io = |cause: std::io::Error| cause.to_string();
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(io)?;
    }
    let copy = path.with_extension("json.tmp");
    replace(path, &copy, text.as_bytes()).map_err(io)?;
    let meta = std::fs::metadata(path).ok();
    Ok(meta.and_then(|meta| Some((meta.modified().ok()?, meta.len()))))
}

/// How the worker's theme writes went, for the next tick to hear.
struct Written {
    done: Sender<(PathBuf, Result<Seen, String>)>,
    heard: Receiver<(PathBuf, Result<Seen, String>)>,
}

impl Default for Written {
    fn default() -> Self {
        let (done, heard) = channel();
        Self { done, heard }
    }
}

/// Hands the texts that wait to the app's worker; each write's outcome comes back for
/// [`hear`]. With no worker the write is done here.
fn send(hub: &mut Hub) {
    let out = library(hub).map(|library| std::mem::take(&mut library.out));
    let out = out.unwrap_or_default();
    if out.is_empty() {
        return;
    }
    let Some(done) = hub.panes.get_mut::<Written>(None).map(|w| w.done.clone()) else {
        return;
    };
    let none = Disk::default();
    let disk = hub.panes.get::<Disk>(None).unwrap_or(&none);
    for (path, text) in out {
        let done = done.clone();
        disk.run(move || {
            let written = write(&path, &text);
            // The hub is gone only as the app quits: nobody is left to hear.
            let _ = done.send((path, written));
        });
    }
}

/// Tells the library how the writes that finished went.
fn hear(hub: &mut Hub) {
    let heard: Vec<_> = match hub.panes.get_mut::<Written>(None) {
        Some(written) => written.heard.try_iter().collect(),
        None => return,
    };
    if let Some(library) = library(hub) {
        for (path, done) in heard {
            library.wrote(&path, done);
        }
    }
}

/// The tokens of the theme in use go to the model by its one message.
pub fn settle(hub: &mut Hub) {
    if let Some(tokens) = library(hub).map(|library| library.tokens()) {
        yd_core::msg::step(&mut hub.model, Msg::Theme(Box::new(tokens)));
    }
}

/// Puts the theme in use on the model in every window, blending every colour when a `tx` is
/// given (a drag gives none: it follows the pointer).
pub fn apply(win: &mut Win, tx: Option<&mut Tx>) {
    let tokens = match win.hub.try_borrow_mut() {
        Ok(mut hub) => {
            let tokens = library(&mut hub).map(|library| library.tokens());
            tokens.filter(|tokens| hub.model.tokens != *tokens)
        }
        Err(_) => None,
    };
    let Some(tokens) = tokens else { return };
    dispatch(win, Msg::Theme(Box::new(tokens)));
    let Some(tx) = tx else { return };
    tx.color_motion(BLEND);
    let Ok(hub) = win.hub.try_borrow() else {
        return;
    };
    let others = hub.wins.iter().filter(|(id, _)| *id != win.window);
    others.for_each(|(_, app)| app.update(|_, tx| tx.color_motion(BLEND)));
}

/// The app's themes, kept in the hub under no pane.
pub fn library(hub: &mut Hub) -> Option<&mut Library> {
    hub.panes.get_mut::<Library>(None)
}

/// Reads the themes folder at `folder`: the start of the native app calls it. From here
/// on a theme's file is written by the app's worker, not where the edit is made.
pub fn open(hub: &mut Hub, folder: PathBuf) {
    if let Some(library) = library(hub) {
        let active = library.active.clone();
        *library = Library::at(folder);
        library.defer = true;
        library.pick(&active);
    }
    settle(hub);
}

/// Writes every edit still unwritten, here and now: what quitting does once the worker
/// has finished.
pub fn close(hub: &mut Hub) {
    hear(hub);
    if let Some(library) = library(hub) {
        library.flush();
        library.save();
    }
}

/// Looks at the folder every [`POLL`] seconds of `now` and puts a changed theme in use,
/// asking for the wake the next look needs. True when the window has something to draw.
pub fn tick(win: &mut Win, now: f64, tx: &mut Tx) -> bool {
    let changed = match win.hub.try_borrow_mut() {
        Ok(mut hub) => {
            hear(&mut hub);
            let changed = match library(&mut hub) {
                Some(library) if library.folder.is_some() => {
                    let due = library.looked.is_none_or(|at| now - at >= POLL);
                    if due {
                        library.looked = Some(now);
                    }
                    let next = library.looked.map_or(now, |at| at + POLL);
                    tx.wake_at(next);
                    let before = due.then(|| library.clone());
                    if due {
                        library.reload();
                    }
                    before.is_some_and(|before| *library != before)
                }
                Some(_) | None => false,
            };
            send(&mut hub);
            changed
        }
        Err(_) => false,
    };
    if changed {
        apply(win, Some(tx));
    }
    changed
}

#[cfg(test)]
mod tests;
