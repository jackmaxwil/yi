//! The host pieces a script may draw with and the feeds it may read.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use rune_luau::mlua::{self, Lua, Table};
use rune_luau::{Data, Host, Sources};
use yd_core::row::Tone;
use yd_core::settings::store::value::Json;

/// What a plugin is told it may build with: the words `plugin.build` sends and "Copy for Yi"
/// copies. A change here is a change to what Yi is told.
pub const KIT: &str = "\
A Yi Desktop plugin pane is a folder panes/<name>/ holding two files.

plugin.json: {\"name\": \"<name>\", \"title\": \"<pane title>\", \"icon\": \"chart\", \
\"reach\": [\"host:port\"], \"every\": <seconds, optional>}. reach lists every host and port \
the pane may fetch from; the user turns each on.

pane.luau (Luau, sandboxed: no files, processes, sockets or clock):
  local pane = require(\"@yi/pane\")
  return {
    view = function(ctx)
      local q = ctx.http.get(\"http://host:port/path\") -- nil until the answer arrives
      return { pane.stat(\"Waiting\", q and q.waiting or \"-\", { tone = \"needs_you\" }) }
    end,
    press = function(id, ctx) end, -- optional: a button was pressed
  }

view returns a list of pieces, drawn by the app in its own theme:
  pane.stat(label, value, { tone = \"needs_you\" | \"ok\" | \"bad\" | \"warn\" | \"accent\" })
  pane.spark(numbers, { label = \"...\" })
  pane.table(rows, { columns = {\"name\", ...}, title = \"...\", note = \"...\" }) \
-- a cell may be { text = \"...\", tone = \"needs_you\" }
  pane.list(items, { title = \"...\" })
  pane.tree(nodes) -- each node { label = \"...\", children = { ... } }
  pane.button(title, { id = \"...\" })
  pane.code(text)
  pane.log(lines)

ctx, the only way out of the sandbox:
  ctx.http.get(url): a granted http or https address; JSON comes back as a table, else text
  ctx.store.get(key), ctx.store.set(key, value): the pane's own store, 256 KiB, kept
  ctx.feed(\"threads\" | \"findings\" | \"logs\"): the app's rows, read only, newest 200

The app reloads the pane when either file is saved; it keeps its store.";

/// The pieces module, `require(\"@yi/pane\")`: each piece is a plain table the app draws.
const PIECES: &str = r#"
local M = {}
local function o(opts) return opts or {} end
function M.stat(label, value, opts) return { piece = "stat", label = label, value = value, tone = o(opts).tone } end
function M.spark(series, opts) return { piece = "spark", series = series, label = o(opts).label } end
function M.table(rows, opts) opts = o(opts) return { piece = "table", rows = rows, columns = opts.columns, title = opts.title, note = opts.note } end
function M.list(items, opts) return { piece = "list", items = items, title = o(opts).title } end
function M.tree(nodes, opts) return { piece = "tree", nodes = nodes, title = o(opts).title } end
function M.button(title, opts) return { piece = "button", title = title, id = o(opts).id or title } end
function M.code(text, opts) return { piece = "code", text = text } end
function M.log(lines, opts) return { piece = "log", lines = lines } end
return table.freeze(M)
"#;

/// The entry the app runs: it hands the plugin its one context object.
const ENTRY: &str = r#"
local plugin = require("@self/pane")
local host = require("@yi/host")
local ctx = table.freeze({
    http = table.freeze({ get = host.get }),
    store = table.freeze({ get = host.load, set = host.save }),
    feed = host.feed,
})
return {
    run = function() return plugin.view(ctx) end,
    press = function(id) if plugin.press then plugin.press(id, ctx) end end,
    saveHotState = function() if plugin.saveHotState then return plugin.saveHotState() end return nil end,
    loadHotState = function(s) if plugin.loadHotState then plugin.loadHotState(s) end end,
}
"#;

/// The most a pane's store may hold, written as JSON.
pub const STORE_CAP: usize = 256 << 10;
/// The most rows of one feed a script is handed: the newest.
pub const FEED_ROWS: usize = 200;

/// The script's files as the VM sees them: the app's entry and the plugin's `pane.luau`.
pub fn sources(pane: &str) -> Sources {
    Sources::new()
        .with("init.luau", ENTRY)
        .with("pane.luau", pane)
}

/// What the host functions of one plugin read and leave behind, filled before each run.
#[derive(Debug, Default)]
pub struct Bridge {
    /// `host:port` pairs this plugin may reach now: its manifest's and switched on.
    pub reach: BTreeSet<String>,
    /// Answers that have arrived, by URL.
    pub answers: BTreeMap<String, Data>,
    pub feeds: BTreeMap<&'static str, Data>,
    /// Granted URLs the script asked for in this run.
    pub asked: BTreeSet<String>,
    pub store: BTreeMap<String, Data>,
    /// The store changed and is not yet written.
    pub dirty: bool,
}

pub type Shared = Rc<RefCell<Bridge>>;

/// Binds `@yi/pane` and `@yi/host` into each new VM and takes the clock away.
pub fn setup(bridge: Shared) -> impl Fn(&Host<'_>) -> mlua::Result<()> + 'static {
    move |host| {
        let lua = host.lua();
        host.register_module(
            "@yi/pane",
            lua.load(PIECES).set_name("=yi/pane").eval::<Table>()?,
        )?;
        host.global("os", mlua::Nil)?;
        let module = lua.create_table()?;
        let b = bridge.clone();
        module.set(
            "get",
            lua.create_function(move |_, url: String| Ok(get(&b, url)))?,
        )?;
        let b = bridge.clone();
        let load = move |_: &Lua, key: String| {
            let b = b
                .try_borrow()
                .map_err(|_| mlua::Error::runtime("store busy"))?;
            Ok(b.store.get(&key).cloned().unwrap_or_default())
        };
        module.set("load", lua.create_function(load)?)?;
        let b = bridge.clone();
        let save = move |_: &Lua, (key, value): (String, Data)| save(&b, key, value);
        module.set("save", lua.create_function(save)?)?;
        let b = bridge.clone();
        let feed = move |_: &Lua, name: String| {
            let b = b
                .try_borrow()
                .map_err(|_| mlua::Error::runtime("feeds busy"))?;
            Ok(b.feeds.get(name.as_str()).cloned().unwrap_or_default())
        };
        module.set("feed", lua.create_function(feed)?)?;
        host.register_module("@yi/host", module)
    }
}

/// `ctx.http.get`: the answer if it came, else nil; asked only when the host is granted.
fn get(bridge: &Shared, url: String) -> Data {
    let Ok(mut b) = bridge.try_borrow_mut() else {
        return Data::Nil;
    };
    if !reach_of(&url).is_some_and(|reach| b.reach.contains(&reach)) {
        return Data::Nil;
    }
    let answer = b.answers.get(&url).cloned();
    b.asked.insert(url);
    answer.unwrap_or_default()
}

fn save(bridge: &Shared, key: String, value: Data) -> mlua::Result<()> {
    let mut b = bridge
        .try_borrow_mut()
        .map_err(|_| mlua::Error::runtime("store busy"))?;
    let before = b.store.insert(key.clone(), value);
    if store_text(&b.store).len() > STORE_CAP {
        match before {
            Some(old) => b.store.insert(key, old),
            None => b.store.remove(&key),
        };
        return Err(mlua::Error::runtime(format!(
            "the store is full: it holds at most {STORE_CAP} bytes"
        )));
    }
    b.dirty = true;
    Ok(())
}

/// The store as the JSON written to `store.json`.
pub fn store_text(store: &BTreeMap<String, Data>) -> String {
    let all = store.iter().map(|(k, v)| (k.clone(), to_json(v)));
    Json::Object(all.collect()).to_string()
}

/// The store `store.json` holds; anything that does not read is an empty store.
pub fn store_of(text: &str) -> BTreeMap<String, Data> {
    match from_json(&text.parse::<Json>().unwrap_or_default()) {
        Data::Map(map) => map,
        _ => BTreeMap::new(),
    }
}

/// `host:port` of an http or https URL, the port filled from the scheme; none for anything
/// else, and for an address with a user in it.
pub fn reach_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let port = match scheme.to_ascii_lowercase().as_str() {
        "http" => "80",
        "https" => "443",
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, given)) if given.parse::<u16>().is_ok() => (host, given),
        Some(_) => return None,
        None => (authority, port),
    };
    Some(format!("{}:{port}", host.to_ascii_lowercase()))
}

pub fn from_json(json: &Json) -> Data {
    if let Some(on) = json.as_bool() {
        Data::Bool(on)
    } else if let Some(n) = json.as_i64() {
        Data::Integer(n)
    } else if let Some(n) = json.as_f64() {
        Data::Number(n)
    } else if let Some(text) = json.as_str() {
        Data::String(text.to_owned())
    } else if let Some(items) = json.as_array() {
        Data::List(items.iter().map(from_json).collect())
    } else if let Some(map) = json.as_object() {
        Data::Map(map.iter().map(|(k, v)| (k.clone(), from_json(v))).collect())
    } else {
        Data::Nil
    }
}

fn to_json(data: &Data) -> Json {
    match data {
        Data::Nil => Json::Null,
        Data::Bool(on) => Json::from(*on),
        Data::Integer(n) => Json::from(*n),
        Data::Number(n) => Json::from(*n),
        Data::String(text) => Json::from(text.as_str()),
        Data::List(items) => Json::Array(items.iter().map(to_json).collect()),
        Data::Map(map) => Json::Object(map.iter().map(|(k, v)| (k.clone(), to_json(v))).collect()),
    }
}

/// One cell of a table piece, or one line of a list.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub text: String,
    pub tone: Option<Tone>,
}

/// One drawn piece of a description.
#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    Stat {
        label: String,
        value: String,
        tone: Option<Tone>,
    },
    Spark {
        series: Vec<f32>,
        label: Option<String>,
    },
    Table {
        title: Option<String>,
        note: Option<String>,
        columns: Vec<String>,
        rows: Vec<Vec<Cell>>,
    },
    List {
        title: Option<String>,
        items: Vec<Cell>,
    },
    /// Each line with its depth from the top.
    Tree {
        title: Option<String>,
        lines: Vec<(usize, String)>,
    },
    Button {
        title: String,
        id: String,
    },
    Code(String),
    Log(Vec<String>),
}

/// A view's result as pieces, or why it is not a description.
pub fn pieces(data: &Data) -> Result<Vec<Piece>, String> {
    let list = match data {
        Data::List(list) => list.as_slice(),
        Data::Map(map) if map.is_empty() => &[],
        other => {
            return Err(format!(
                "view must return a list of pieces, not {}",
                kind(other)
            ));
        }
    };
    list.iter()
        .enumerate()
        .map(|(at, item)| {
            piece(item).ok_or_else(|| {
                format!(
                    "piece {} is not one of stat, spark, table, list, tree, button, code, log",
                    at.saturating_add(1)
                )
            })
        })
        .collect()
}

fn piece(data: &Data) -> Option<Piece> {
    let opt = |field: &str| words(data.get(field));
    Some(match data.get("piece").as_str()? {
        "stat" => Piece::Stat {
            label: opt("label").unwrap_or_default(),
            value: opt("value").unwrap_or_default(),
            tone: tone(data.get("tone")),
        },
        "spark" => Piece::Spark {
            series: items(data.get("series"))
                .iter()
                .filter_map(|n| n.as_f64())
                .map(|n| n as f32)
                .collect(),
            label: opt("label"),
        },
        "table" => {
            let rows = items(data.get("rows"));
            let mut columns: Vec<String> = items(data.get("columns"))
                .iter()
                .filter_map(words)
                .collect();
            if columns.is_empty() {
                columns = match rows.first() {
                    Some(Data::Map(first)) => first.keys().cloned().collect(),
                    _ => Vec::new(),
                };
            }
            let rows = rows
                .iter()
                .map(|row| columns.iter().map(|c| cell(row.get(c))).collect())
                .collect();
            Piece::Table {
                title: opt("title"),
                note: opt("note"),
                columns,
                rows,
            }
        }
        "list" => Piece::List {
            title: opt("title"),
            items: items(data.get("items")).iter().map(cell).collect(),
        },
        "tree" => {
            let mut lines = Vec::new();
            let mut stack: Vec<(usize, &Data)> = items(data.get("nodes"))
                .iter()
                .rev()
                .map(|n| (0, n))
                .collect();
            while let Some((depth, node)) = stack.pop() {
                lines.push((depth, words(node.get("label")).unwrap_or_default()));
                let below = items(node.get("children"))
                    .iter()
                    .rev()
                    .map(|n| (depth.saturating_add(1), n));
                stack.extend(below);
            }
            Piece::Tree {
                title: opt("title"),
                lines,
            }
        }
        "button" => Piece::Button {
            title: opt("title").unwrap_or_default(),
            id: opt("id").unwrap_or_default(),
        },
        "code" => Piece::Code(opt("text").unwrap_or_default()),
        "log" => Piece::Log(items(data.get("lines")).iter().filter_map(words).collect()),
        _ => return None,
    })
}

fn items(data: &Data) -> &[Data] {
    match data {
        Data::List(list) => list,
        _ => &[],
    }
}

/// A value as the words it shows: text as it is, whole numbers without a point.
fn words(data: &Data) -> Option<String> {
    match data {
        Data::String(text) => Some(text.clone()),
        Data::Integer(n) => Some(n.to_string()),
        Data::Number(n) => Some(n.to_string()),
        Data::Bool(on) => Some(on.to_string()),
        Data::Nil | Data::List(_) | Data::Map(_) => None,
    }
}

fn cell(data: &Data) -> Cell {
    match data {
        Data::Map(_) => Cell {
            text: words(data.get("text")).unwrap_or_default(),
            tone: tone(data.get("tone")),
        },
        other => Cell {
            text: words(other).unwrap_or_default(),
            tone: None,
        },
    }
}

/// A theme role by the name a script gives it; an unknown name is no tone.
fn tone(data: &Data) -> Option<Tone> {
    Some(match data.as_str()? {
        "needs_you" | "need" => Tone::Need,
        "ok" => Tone::Ok,
        "bad" => Tone::Bad,
        "warn" => Tone::Warn,
        "accent" => Tone::Accent,
        "model" => Tone::Model,
        "context" => Tone::Context,
        "dim" => Tone::Dim,
        "faint" => Tone::Faint,
        _ => return None,
    })
}

fn kind(data: &Data) -> &'static str {
    match data {
        Data::Nil => "nil",
        Data::Bool(_) => "a boolean",
        Data::Integer(_) | Data::Number(_) => "a number",
        Data::String(_) => "a string",
        Data::List(_) | Data::Map(_) => "a table",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_is_reached_by_its_host_and_port_and_only_on_the_web() {
        assert_eq!(
            reach_of("http://CI-Ubuntu:9100/queue").as_deref(),
            Some("ci-ubuntu:9100")
        );
        assert_eq!(
            reach_of("https://example.org/x").as_deref(),
            Some("example.org:443")
        );
        assert_eq!(reach_of("http://user@ci-ubuntu:9100/"), None);
        assert_eq!(reach_of("file:///etc/passwd"), None);
        assert_eq!(reach_of("http://ci-ubuntu:http/"), None);
    }

    #[test]
    fn a_result_that_is_not_a_list_of_pieces_says_what_it_was() {
        let said = pieces(&Data::String("hi".into()));
        assert_eq!(
            said,
            Err("view must return a list of pieces, not a string".to_owned())
        );
    }
}
