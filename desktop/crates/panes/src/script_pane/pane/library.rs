//! The plugins found on disk: each one's manifest, script, store, last picture and last error.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, SystemTime};

use rune_luau::{Data, Script, ScriptOptions};
use std::borrow::Cow;
use yd_core::feed::Table;
use yd_core::ids::SettingKey;
use yd_core::ids::{FeedId, RowId};
use yd_core::model::Model;
use yd_core::msg::{Msg, step};
use yd_core::row::{Row, Rows, Value};
use yd_core::settings::schema::{Control, Setting, SettingValue, Store};
use yd_core::settings::store::SettingsMsg;
use yd_core::settings::store::value::Json;

use crate::script_pane::host::{self, Bridge, Piece, Shared, reach_of};

/// The Plugins list's rows: one a plugin, filled from the library.
pub const FEED: FeedId = FeedId::new("plugins");
/// The setting that keeps which hosts each plugin may reach: lines of `<plugin> <host:port>`.
pub const GRANTS: &str = "plugin-kit.grants";
/// The grants' row in Settings: the kit's switches write it, the app's settings file keeps it.
pub const GRANTS_ROW: Setting = Setting {
    key: SettingKey::fixed(GRANTS),
    group: Cow::Borrowed("Plugins"),
    name: Cow::Borrowed("Hosts plugin panes may reach"),
    description: Cow::Borrowed("One line a grant: the plugin's name, a space, then host:port."),
    control: Control::Text,
    store: Store::App,
    default: Some(SettingValue::Text(Cow::Borrowed(""))),
    value: None,
};
/// How often the folder is looked at, in seconds of the poll's clock.
pub const LOOK: f64 = 1.0;
/// The most of a plugin's file that is read.
const MAX_FILE: u64 = 256 << 10;
/// Web asks a plugin may have waiting, and addresses it may reach in one load: a script that
/// asks a new address each run would otherwise fetch for as long as it is open.
const MAX_WAITING: usize = 8;
const MAX_ADDRESSES: usize = 32;
/// Answers the fetch table keeps for every plugin together; past it the oldest go.
pub const MAX_FETCHED: usize = 64;

/// A plugin's `plugin.json`. Fields this app does not know are kept in `raw`.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub name: String,
    pub title: String,
    pub icon: String,
    /// `host:port` pairs it may ask to reach, lower case.
    pub reach: Vec<String>,
    /// Seconds between timer runs.
    pub every: Option<u32>,
    pub raw: Json,
}

impl Manifest {
    pub fn read(text: &str) -> Result<Manifest, String> {
        let raw: Json = text
            .parse()
            .map_err(|e| format!("plugin.json does not read: {e}"))?;
        let field = |name: &str| raw.get(name).and_then(Json::as_str).map(str::to_owned);
        let name = field("name").ok_or("plugin.json has no name")?;
        let mut reach = Vec::new();
        for entry in raw
            .get("reach")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            let entry = entry
                .as_str()
                .ok_or("reach lists a value that is not text")?;
            match reach_of(&["http://", entry, "/"].concat()) {
                Some(pair) if entry.contains(':') => reach.push(pair),
                _ => return Err(format!("reach names {entry:?}, not host:port")),
            }
        }
        let every = match raw.get("every") {
            None | Some(Json::Null) => None,
            Some(n) => match n.as_u64().and_then(|n| u32::try_from(n).ok()) {
                Some(n @ 1..=86_400) => Some(n),
                _ => return Err("every is not a whole number of seconds from 1 to 86400".into()),
            },
        };
        Ok(Manifest {
            title: field("title").unwrap_or_else(|| name.clone()),
            icon: field("icon").unwrap_or_else(|| "chart".to_owned()),
            name,
            reach,
            every,
            raw,
        })
    }
}

/// When a plugin's two files last changed, as the folder says.
type Stamp = (Option<SystemTime>, Option<SystemTime>);

/// One loaded plugin.
pub struct Plugin {
    /// Its folder's name: what the app calls it.
    pub name: String,
    pub dir: Option<PathBuf>,
    pub manifest: Result<Manifest, String>,
    pub source: String,
    script: Script,
    bridge: Shared,
    /// Moves on every reload that took.
    pub version: u64,
    /// How long the last load or reload took.
    pub took: Duration,
    /// The last good picture: what stays under an error.
    pub picture: Vec<Piece>,
    pub error: Option<String>,
    seen: Stamp,
    ran: Option<f64>,
    /// URLs asked for and not yet answered.
    pending: BTreeSet<String>,
    /// Every URL fetched since this plugin loaded, held to [`MAX_ADDRESSES`].
    reached: BTreeSet<String>,
    /// The URLs its last run read, the first [`MAX_ADDRESSES`]: answers it still draws from.
    reads: BTreeSet<String>,
}

impl Plugin {
    pub fn new(name: &str, manifest: &str, source: &str, store: &str) -> Plugin {
        let bridge: Shared = Rc::default();
        if let Ok(mut b) = bridge.try_borrow_mut() {
            b.store = host::store_of(store);
        }
        let options = ScriptOptions::new().setup(host::setup(bridge.clone()));
        let start = std::time::Instant::now();
        let script = Script::with_options(name, host::sources(source), options);
        let took = start.elapsed();
        Plugin {
            name: name.to_owned(),
            dir: None,
            manifest: Manifest::read(manifest),
            source: source.to_owned(),
            error: script.error().map(|e| said(&e)),
            script,
            bridge,
            version: 1,
            took,
            picture: Vec::new(),
            seen: (None, None),
            ran: None,
            pending: BTreeSet::new(),
            reached: BTreeSet::new(),
            reads: BTreeSet::new(),
        }
    }

    /// Takes new files: the script reloads keeping its state and store; a failed reload
    /// keeps the running one and says why.
    pub fn reload(&mut self, manifest: &str, source: &str) {
        self.manifest = Manifest::read(manifest);
        self.source = source.to_owned();
        self.ran = None;
        match self.script.reload(host::sources(source)) {
            Ok(took) => {
                self.version = self.version.saturating_add(1);
                self.took = took.compile.saturating_add(took.swap);
                self.error = None;
            }
            Err(e) => self.error = Some(said(&e)),
        }
    }

    pub fn every(&self) -> Option<u32> {
        self.manifest.as_ref().ok().and_then(|m| m.every)
    }

    pub fn title(&self) -> &str {
        self.manifest
            .as_ref()
            .map_or(self.name.as_str(), |m| m.title.as_str())
    }

    pub fn reach(&self) -> &[String] {
        self.manifest.as_ref().map_or(&[], |m| m.reach.as_slice())
    }

    /// Runs the view with what the app shows now. Returns the URLs to ask the fetch source for:
    /// those not answered and not asked yet, and on a `refresh` every one it read.
    pub fn run(&mut self, inputs: &Inputs<'_>, now: f64, refresh: bool) -> Vec<String> {
        self.ran = Some(now);
        if let Ok(mut b) = self.bridge.try_borrow_mut() {
            fill(&mut b, &self.name, self.reach(), inputs);
        }
        let ran = self.script.query("run", []);
        let asked = self
            .bridge
            .try_borrow_mut()
            .map(|mut b| std::mem::take(&mut b.asked))
            .unwrap_or_default();
        match ran
            .map_err(|e| said(&e))
            .and_then(|data| host::pieces(&data))
        {
            Ok(picture) => {
                self.picture = picture;
                self.error = None;
            }
            Err(why) => self.error = Some(why),
        }
        self.reads = asked.iter().take(MAX_ADDRESSES).cloned().collect();
        let answered = |url: &String| inputs.answer(url).is_some() && !refresh;
        let wanted: Vec<String> = asked
            .into_iter()
            .filter(|url| !answered(url) && !self.pending.contains(url))
            .collect();
        let mut new = Vec::new();
        for url in &wanted {
            let known = self.reached.contains(url);
            if self.pending.len() < MAX_WAITING && (known || self.reached.len() < MAX_ADDRESSES) {
                self.reached.insert(url.clone());
                self.pending.insert(url.clone());
                new.push(url.clone());
            }
        }
        if new.len() < wanted.len() && self.error.is_none() {
            self.error = Some(format!(
                "[…] kept {} of {} web asks (cap {MAX_WAITING} waiting, {MAX_ADDRESSES} addresses \
                 a load); the rest are asked as answers come, or after a reload",
                new.len(),
                wanted.len()
            ));
        }
        new
    }

    /// A button's press, then the view again.
    pub fn press(&mut self, id: &str, inputs: &Inputs<'_>, now: f64) -> Vec<String> {
        if let Err(e) = self.script.call("press", [Data::String(id.to_owned())]) {
            self.error = Some(said(&e));
            return Vec::new();
        }
        self.run(inputs, now, false)
    }

    /// The store's text to write, once after each change.
    fn unsaved(&self) -> Option<String> {
        let mut b = self.bridge.try_borrow_mut().ok()?;
        let dirty = std::mem::take(&mut b.dirty);
        dirty.then(|| host::store_text(&b.store))
    }

    pub fn store(&self) -> BTreeMap<String, Data> {
        self.bridge
            .try_borrow()
            .map(|b| b.store.clone())
            .unwrap_or_default()
    }

    /// Its row in the Plugins list.
    pub fn row(&self) -> Row {
        let letter = self
            .name
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string());
        let adds = vec![["adds pane · ", self.title()].concat()];
        let description = match &self.manifest {
            Ok(_) => "A pane drawn by its pane.luau, in the app.".to_owned(),
            Err(why) => why.clone(),
        };
        let path = self.dir.as_ref().map(|dir| dir.join("pane.luau"));
        Row::new(self.name.as_str())
            .text("letter", letter.unwrap_or_default())
            .text("name", self.name.as_str())
            .text("version", format!("version {}", self.version))
            .text("took", format!("loaded in {} ms", self.took.as_millis()))
            .cell("enabled", Value::Flag(true))
            .cell("adds", Value::Tags(adds))
            .text("description", description)
            .cell("reach", Value::Tags(self.reach().to_vec()))
            .text("error", self.error.clone().unwrap_or_default())
            .text(
                "path",
                path.map(|p| p.display().to_string()).unwrap_or_default(),
            )
    }
}

/// An error as the pane shows it: its first line, which names the file and line.
fn said(error: &rune_luau::ScriptError) -> String {
    error.summary().replace("[string \"", "").replace("\"]", "")
}

/// What a run reads from the app: the grants, the fetch source's answers and the feeds.
pub struct Inputs<'a> {
    pub grants: String,
    pub fetched: Option<&'a Table>,
    model: &'a Model,
    /// Made on the first run that needs them, so a pass that runs nothing builds none.
    feeds: std::cell::OnceCell<BTreeMap<&'static str, Data>>,
}

impl<'a> Inputs<'a> {
    pub fn of(model: &'a Model) -> Inputs<'a> {
        let grants = match model.settings.value(GRANTS) {
            Some(SettingValue::Text(text)) => text.to_string(),
            _ => String::new(),
        };
        Inputs {
            grants,
            fetched: model.tables.get(yd_core::feed::FETCH),
            model,
            feeds: std::cell::OnceCell::new(),
        }
    }

    fn feeds(&self) -> &BTreeMap<&'static str, Data> {
        self.feeds.get_or_init(|| feeds(self.model))
    }

    /// The answer row for `url`, if the fetch source sent one.
    pub fn answer(&self, url: &str) -> Option<&Row> {
        self.fetched?.find(&RowId::new(["get:", url].concat()))
    }

    pub fn granted(&self, plugin: &str, reach: &str) -> bool {
        let line = [plugin, " ", reach].concat();
        self.grants.lines().any(|held| held == line)
    }
}

/// The grants text with `plugin`'s `reach` switched on or off.
pub fn regrant(grants: &str, plugin: &str, reach: &str, on: bool) -> String {
    let line = [plugin, " ", reach].concat();
    let kept = grants
        .lines()
        .filter(|held| *held != line && !held.is_empty());
    let mut lines: Vec<&str> = kept.collect();
    if on {
        lines.push(&line);
    }
    lines.join("\n")
}

/// Switches `plugin`'s reach of `pair` on or off in the model's settings.
pub fn grant(model: &mut Model, plugin: &str, pair: &str, on: bool) {
    let key = SettingKey::fixed(GRANTS);
    if !model.settings.rows.iter().any(|row| row.key == key) {
        step(model, Msg::Settings(SettingsMsg::Declare(vec![GRANTS_ROW])));
    }
    let text = regrant(&Inputs::of(model).grants, plugin, pair, on);
    let value = SettingValue::Text(Cow::Owned(text));
    step(model, Msg::Settings(SettingsMsg::Set(key, Some(value))));
}

fn fill(b: &mut Bridge, name: &str, reach: &[String], inputs: &Inputs<'_>) {
    b.reach = reach
        .iter()
        .filter(|pair| inputs.granted(name, pair))
        .cloned()
        .collect();
    b.answers.clear();
    let rows = inputs
        .fetched
        .into_iter()
        .flat_map(|t| (0..t.len()).filter_map(|at| t.get(at)));
    for row in rows {
        let Some(url) = row.id.as_str().strip_prefix("get:") else {
            continue;
        };
        if reach_of(url).is_some_and(|pair| b.reach.contains(&pair))
            && row.words("status").is_some_and(|s| s.starts_with('2'))
        {
            let body = row.words("body").unwrap_or_default();
            let value = body
                .parse::<Json>()
                .map_or_else(|_| Data::String(body.to_owned()), |j| host::from_json(&j));
            b.answers.insert(url.to_owned(), value);
        }
    }
    b.feeds = inputs.feeds().clone();
}

/// The feeds a script may read, newest [`host::FEED_ROWS`] of each.
fn feeds(model: &Model) -> BTreeMap<&'static str, Data> {
    let map = |pairs: Vec<(&str, String)>| {
        Data::Map(
            pairs
                .into_iter()
                .map(|(k, v)| (k.to_owned(), Data::String(v)))
                .collect(),
        )
    };
    let threads = model
        .wire
        .listed()
        .into_iter()
        .take(host::FEED_ROWS)
        .map(|t| {
            let status = format!("{:?}", model.wire.status(&t.id)).to_lowercase();
            map(vec![
                ("id", t.id.as_str().to_owned()),
                ("title", t.title.clone().unwrap_or_default()),
                ("status", status),
            ])
        });
    let logs = model.tables.get(FeedId::new("logs")).map(|t| {
        let from = t.len().saturating_sub(host::FEED_ROWS);
        let rows = (from..t.len()).filter_map(|at| t.get(at));
        rows.map(|row| {
            let cells = row
                .cells
                .keys()
                .filter_map(|k| Some((k.as_str(), row.words(k)?.to_owned())));
            map(std::iter::once(("id", row.id.as_str().to_owned()))
                .chain(cells)
                .collect())
        })
        .collect::<Vec<_>>()
    });
    let findings = model.wire.listed().into_iter().flat_map(|t| {
        yd_core::findings::view::findings(model, &t.id, yd_core::findings::view::Scope::Thread)
    });
    let findings = findings.take(host::FEED_ROWS).map(|f| {
        map(vec![
            ("title", f.title),
            ("stage", format!("{:?}", f.stage).to_lowercase()),
        ])
    });
    BTreeMap::from([
        ("threads", Data::List(threads.collect())),
        ("logs", Data::List(logs.unwrap_or_default())),
        ("findings", Data::List(findings.collect())),
    ])
}

/// Every plugin the app has, kept in the hub under no pane.
#[derive(Default)]
pub struct Library {
    /// `panes/` beside the settings file; none in a scene, which adds plugins itself.
    pub folder: Option<PathBuf>,
    /// Where Remove moves a plugin's folder: the user's Trash.
    pub trash: Option<PathBuf>,
    pub plugins: Vec<Plugin>,
    looked: Option<f64>,
    /// The grants as the last runs saw them.
    seen: Option<String>,
    /// The Plugins list's rows are behind the library.
    pub changed: bool,
}

/// What one pass over the library asks of the app afterwards.
#[derive(Default)]
pub struct Asks {
    pub fetch: Vec<String>,
    /// Store files to write: path and text.
    pub writes: Vec<(PathBuf, String)>,
    /// The Plugins list's rows, when they moved.
    pub rows: Option<Vec<Row>>,
    /// Answers to drop from the fetch table, which is past [`MAX_FETCHED`].
    pub forget: Vec<RowId>,
    pub next: Option<f64>,
    /// Something drawn changed.
    pub drawn: bool,
}

impl Library {
    pub fn find(&self, name: &str) -> Option<&Plugin> {
        self.plugins.iter().find(|p| p.name == name)
    }

    pub fn find_mut(&mut self, name: &str) -> Option<&mut Plugin> {
        self.plugins.iter_mut().find(|p| p.name == name)
    }

    /// Adds or replaces a plugin made from text, as a scene or a test does.
    pub fn add(&mut self, plugin: Plugin) {
        self.plugins.retain(|p| p.name != plugin.name);
        self.plugins.push(plugin);
        self.plugins.sort_by(|a, b| a.name.cmp(&b.name));
        self.changed = true;
    }

    /// Reads a plugin's files again now and reloads it; one made from text reloads as it is.
    pub fn reload_now(&mut self, name: &str) -> bool {
        let Some(plugin) = self.find_mut(name) else {
            return false;
        };
        let (manifest, source) = match &plugin.dir {
            Some(dir) => (read(&dir.join("plugin.json")), read(&dir.join("pane.luau"))),
            None => (
                plugin
                    .manifest
                    .as_ref()
                    .map(|m| m.raw.to_string())
                    .unwrap_or_default(),
                plugin.source.clone(),
            ),
        };
        plugin.reload(&manifest, &source);
        self.changed = true;
        true
    }

    /// Moves a plugin's folder into the Trash, under a name not taken there, and forgets it.
    /// Nothing is deleted.
    pub fn remove(&mut self, name: &str) -> Result<PathBuf, String> {
        let dir = self
            .find(name)
            .and_then(|p| p.dir.clone())
            .ok_or("this plugin has no folder")?;
        let trash = self.trash.clone().ok_or("no Trash folder is known")?;
        let free = (0..100u32)
            .map(|n| {
                if n == 0 {
                    trash.join(name)
                } else {
                    trash.join(format!("{name} {n}"))
                }
            })
            .find(|at| !at.exists())
            .ok_or("the Trash has a hundred plugins of this name")?;
        std::fs::rename(&dir, &free).map_err(|e| format!("could not move it to the Trash: {e}"))?;
        self.plugins.retain(|p| p.name != name);
        self.changed = true;
        Ok(free)
    }

    /// One pass: the folder looked at when due, timers of the `shown` plugins, a rerun of any
    /// plugin when grants moved, and of a shown plugin when one of its own asks was answered.
    pub fn advance(&mut self, model: &Model, shown: &BTreeSet<String>, now: f64) -> Asks {
        let mut asks = Asks::default();
        let mut fresh: BTreeSet<String> = BTreeSet::new();
        if self.folder.is_some() && self.looked.is_none_or(|at| now - at >= LOOK) {
            self.looked = Some(now);
            fresh = self.look();
        }
        let inputs = Inputs::of(model);
        let moved = self.seen.as_ref() != Some(&inputs.grants);
        self.seen = Some(inputs.grants.clone());
        let mut next = self.folder.as_ref().and(self.looked).map(|at| at + LOOK);
        for plugin in &mut self.plugins {
            let visible = shown.contains(&plugin.name);
            let heard = visible
                && plugin
                    .pending
                    .iter()
                    .any(|url| inputs.answer(url).is_some());
            if heard {
                plugin.pending.retain(|url| inputs.answer(url).is_none());
            }
            let every = plugin.every().filter(|_| visible).map(f64::from);
            let due = every.is_some_and(|every| plugin.ran.is_none_or(|at| now - at >= every));
            let first = plugin.ran.is_none() || fresh.contains(&plugin.name);
            if first || due || moved || heard {
                asks.fetch.extend(plugin.run(&inputs, now, due && !first));
                asks.drawn = true;
            }
            if let (Some(every), Some(at)) = (every, plugin.ran) {
                let due = at + every;
                next = Some(next.map_or(due, |n: f64| n.min(due)));
            }
            if let (Some(text), Some(dir)) = (plugin.unsaved(), &plugin.dir) {
                asks.writes.push((dir.join("store.json"), text));
            }
        }
        if std::mem::take(&mut self.changed) || !fresh.is_empty() || asks.drawn {
            asks.rows = Some(self.plugins.iter().map(Plugin::row).collect());
        }
        asks.next = next;
        asks.forget = self.spare(inputs.fetched);
        asks
    }

    /// The answers to let go of so the fetch table holds at most [`MAX_FETCHED`]: the oldest
    /// no plugin waits on first, and one a plugin's last run read only when that is not enough.
    fn spare(&self, fetched: Option<&Table>) -> Vec<RowId> {
        let Some(table) = fetched else {
            return Vec::new();
        };
        let over = table.len().saturating_sub(MAX_FETCHED);
        let waited = |row: &&Row| {
            let url = row.id.as_str().strip_prefix("get:").unwrap_or_default();
            let waits = |p: &Plugin| p.reads.contains(url) || p.pending.contains(url);
            self.plugins.iter().any(waits)
        };
        let rows = || (0..table.len()).filter_map(|at| table.get(at));
        let idle = rows().filter(|row| !waited(row));
        let spare = idle.chain(rows().filter(waited)).take(over);
        spare.map(|row| row.id.clone()).collect()
    }

    /// Reads the folder: a new plugin loads, a changed one reloads, a gone one leaves. The
    /// names of the plugins that loaded or reloaded.
    fn look(&mut self) -> BTreeSet<String> {
        let mut fresh = BTreeSet::new();
        let Some(folder) = self.folder.clone() else {
            return fresh;
        };
        let mut found: Vec<(String, PathBuf)> = std::fs::read_dir(&folder)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.path().join("plugin.json").is_file())
            .filter_map(|entry| Some((entry.file_name().into_string().ok()?, entry.path())))
            .collect();
        found.sort();
        self.plugins
            .retain(|p| p.dir.is_none() || found.iter().any(|(name, _)| *name == p.name));
        for (name, dir) in found {
            let stamp = (
                modified(&dir.join("pane.luau")),
                modified(&dir.join("plugin.json")),
            );
            if self.find(&name).is_some_and(|plugin| plugin.seen == stamp) {
                continue;
            }
            let (manifest, source) = (read(&dir.join("plugin.json")), read(&dir.join("pane.luau")));
            match self.find_mut(&name) {
                Some(plugin) => {
                    plugin.seen = stamp;
                    plugin.reload(&manifest, &source);
                }
                None => {
                    let mut plugin =
                        Plugin::new(&name, &manifest, &source, &read(&dir.join("store.json")));
                    plugin.dir = Some(dir);
                    plugin.seen = stamp;
                    self.add(plugin);
                }
            }
            fresh.insert(name);
        }
        fresh
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// A plugin's file as text, at most [`MAX_FILE`] bytes of it; a missing one is empty.
fn read(path: &Path) -> String {
    use std::io::Read;
    let mut text = String::new();
    if let Ok(file) = std::fs::File::open(path) {
        let _ = file.take(MAX_FILE).read_to_string(&mut text);
    }
    text
}

/// Writes `text` to `path` whole or not at all: a copy beside it, synced, then renamed.
pub fn write(path: &Path, text: &str) -> std::io::Result<()> {
    let copy = path.with_extension("json.new");
    yd_core::findings::triage::replace(path, &copy, text.as_bytes())
}
