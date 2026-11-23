//! Language servers: started when a file opens, spoken to over stdio, one per language and
//! working folder, their problems and answers sent as the `lsp` feed's rows.

mod answers;
mod ask;
mod edit;
mod server;

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use yd_core::command::table::{Run, Target};
use yd_core::feed::{FeedEvent, FeedStatus, SourceCmd};
use yd_core::ids::RowId;
use yd_core::inbox::{Closed, FeedTx};
use yd_core::row::Row;

use answers::{Shown, Stats, Texts};
use ask::{Ask, Spot};
use server::{Doc, Out, Server, State, exited, kill, reap, start, stop, which};

use super::protocol::{self, Incoming, ReadError};
use crate::fs::inside;

pub use yd_core::command::tools::lsp::{
    APPLY_FIX, DEFINITION, FIXES, HINTS, HOVER, OPEN, REFERENCES, RESTART,
};
pub use yd_core::feed::LSP as FEED;

/// One row of the server table: which server a file's extension starts, and how.
#[derive(Debug)]
pub struct Language {
    /// The name its row shows and its id carries.
    pub server: &'static str,
    pub language: &'static str,
    /// The protocol's language id.
    pub id: &'static str,
    pub extensions: &'static [&'static str],
    /// Looked for on `PATH`, then in the folders a Dock launch lacks.
    pub program: &'static str,
    pub args: &'static [&'static str],
    /// Sent as `initializationOptions`.
    pub options: fn() -> Value,
}

pub const LANGUAGES: &[Language] = &[Language {
    server: "rust-analyzer",
    language: "Rust",
    id: "rust",
    extensions: &["rs"],
    program: "rust-analyzer",
    args: &[],
    options: no_project_code,
}];

/// Invariant: opening a file runs no code from the project: no build scripts, proc-macros
/// or cargo check. Owner decision to revisit: a per-folder trust setting.
fn no_project_code() -> Value {
    json!({"cargo": {"buildScripts": {"enable": false}}, "procMacro": {"enable": false}, "checkOnSave": false})
}

/// How long a question may wait for its answer, and `initialize` for its own.
const TIMEOUT: Duration = Duration::from_secs(10);
const START: Duration = Duration::from_secs(30);
/// How long a server has to exit after `shutdown` and `exit` before its group is killed.
const GRACE: Duration = Duration::from_secs(2);
/// Messages waiting to be written to one server; past it the server is ended.
const QUEUE: usize = 256;
const EVENTS: usize = 1024;
/// Folders a Dock launch's bare `PATH` lacks, where language servers are installed.
const MORE_BINS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin"];

pub fn run(out: FeedTx, cmds: Receiver<SourceCmd>) -> Result<(), Closed> {
    serve(out, cmds, LANGUAGES)
}

/// The source's body over a given server table; every server is stopped when it ends.
pub(crate) fn serve(
    out: FeedTx,
    cmds: Receiver<SourceCmd>,
    languages: &'static [Language],
) -> Result<(), Closed> {
    let (events, inbox) = sync_channel(EVENTS);
    let forward = events.clone();
    thread::spawn(move || {
        for cmd in cmds {
            if forward.send(Event::Cmd(cmd)).is_err() {
                return;
            }
        }
        let _ = forward.send(Event::Done);
    });
    let mut lsp = Lsp {
        out,
        events,
        languages,
        root: None,
        servers: BTreeMap::new(),
        found: BTreeMap::new(),
        serial: 0,
        shown: BTreeMap::new(),
        fixes: BTreeMap::new(),
        diagnostics: BTreeMap::new(),
    };
    let ended = lsp
        .out
        .send(FeedEvent::Reset(Vec::new()))
        .and_then(|()| lsp.serve(&inbox));
    lsp.stop_all();
    ended
}

enum Event {
    Cmd(SourceCmd),
    Done,
    Message(u64, Value),
    Ended(u64, End),
}

/// Why a server's pipes stopped.
enum End {
    Read(ReadError),
    Write(io::Error),
}

struct Lsp {
    out: FeedTx,
    events: SyncSender<Event>,
    languages: &'static [Language],
    root: Option<PathBuf>,
    servers: BTreeMap<&'static str, Server>,
    /// Each server's program as found on `PATH`, looked for once.
    found: BTreeMap<&'static str, Option<PathBuf>>,
    serial: u64,
    /// The rows last sent for each kind of answer about each file.
    shown: BTreeMap<String, Vec<RowId>>,
    /// Each fix row's edit and the diff it showed.
    fixes: BTreeMap<String, (Value, String)>,
    /// The diagnostics last published for each file, as sent.
    diagnostics: BTreeMap<String, Vec<Value>>,
}

impl Lsp {
    fn serve(&mut self, inbox: &Receiver<Event>) -> Result<(), Closed> {
        loop {
            let wait = self
                .next_deadline()
                .map_or(TIMEOUT, |at| at.saturating_duration_since(Instant::now()));
            match inbox.recv_timeout(wait) {
                Ok(Event::Cmd(cmd)) => self.command(cmd)?,
                Ok(Event::Message(serial, message)) => self.message(serial, message)?,
                Ok(Event::Ended(serial, end)) => self.ended(serial, end)?,
                Ok(Event::Done) | Err(RecvTimeoutError::Disconnected) => return Ok(()),
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.expire()?;
        }
    }

    fn command(&mut self, cmd: SourceCmd) -> Result<(), Closed> {
        match cmd {
            SourceCmd::Root(folder) => {
                let folder = folder.canonicalize().unwrap_or(folder);
                if self.root.as_ref() == Some(&folder) {
                    return Ok(());
                }
                self.stop_all();
                self.root = Some(folder);
                self.out.send(FeedEvent::Reset(Vec::new()))
            }
            SourceCmd::Close => {
                self.stop_all();
                self.out.send(FeedEvent::Reset(Vec::new()))
            }
            SourceCmd::Run(run) => self.run(&run),
            SourceCmd::Input(_)
            | SourceCmd::Resize { .. }
            | SourceCmd::Refresh
            | SourceCmd::Read(_)
            | SourceCmd::Revert { .. }
            | SourceCmd::Screen(_)
            | SourceCmd::Seen(_)
            | SourceCmd::Labels(_)
            | SourceCmd::Readable(_) => Ok(()),
        }
    }

    fn refuse(&self, why: impl Into<String>) -> Result<(), Closed> {
        self.out
            .send(FeedEvent::Status(FeedStatus::Error(why.into())))
    }

    /// A hover's failure goes in its card, so resting on a word never errors the whole feed.
    fn tell(&mut self, ask: &Ask, why: String) -> Result<(), Closed> {
        let Ask::Hover(s) = ask else {
            return self.refuse(why);
        };
        let card = Row::new(format!("hover:{}", s.id())).text("text", why);
        self.replace(format!("hover:{}", s.path), vec![card])
    }

    fn run(&mut self, run: &Run) -> Result<(), Closed> {
        let Some(root) = self.root.clone() else {
            return self.refuse("language servers start once the thread's working folder is known");
        };
        if run.command == RESTART {
            return self.restart(run, &root);
        }
        if run.command == APPLY_FIX {
            return self.apply_fix(run, &root);
        }
        let Some(spot) = Spot::of(run) else {
            return self.refuse(format!("{} needs a file or lines address", run.command.0));
        };
        let ask = match run.command {
            c if c == OPEN => None,
            c if c == HOVER => Some(Ask::Hover(spot.clone())),
            c if c == DEFINITION => Some(Ask::Definition(spot.clone())),
            c if c == REFERENCES => Some(Ask::References(spot.clone())),
            c if c == HINTS => Some(Ask::Hints(spot.path.clone())),
            c if c == FIXES => Some(Ask::Fixes(spot.clone())),
            _ => return Ok(()),
        };
        // Nobody waits on a file being opened or on its hints: only a question is told.
        let waits = ask.as_ref().filter(|a| !matches!(a, Ask::Hints(_)));
        let Some(name) = self.server_for(&spot.path, &root, waits)? else {
            return Ok(());
        };
        let Some(abs) = self.open(name, &spot.path, &root)? else {
            return Ok(());
        };
        match ask {
            Some(ask) => self.ask(name, ask, &abs),
            None => Ok(()),
        }
    }

    /// The running server for `path`'s language, started if it is not; none when its program
    /// is not installed, it failed, or no row serves it, which a question is told in words.
    fn server_for(
        &mut self,
        path: &str,
        root: &Path,
        asked: Option<&Ask>,
    ) -> Result<Option<&'static str>, Closed> {
        let ext = Path::new(path).extension().and_then(|e| e.to_str());
        let lang = self
            .languages
            .iter()
            .find(|l| ext.is_some_and(|e| l.extensions.contains(&e)));
        let Some(lang) = lang else {
            let kind = ext.map_or_else(|| path.to_owned(), |ext| format!(".{ext} files"));
            let told = asked.map(|ask| self.tell(ask, format!("no language server for {kind}")));
            return told.unwrap_or(Ok(())).map(|()| None);
        };
        match self.servers.get(lang.server) {
            Some(server) if matches!(server.state, State::Failed(_)) => Ok(None),
            Some(_) => Ok(Some(lang.server)),
            None => self.launch(lang, root),
        }
    }

    fn launch(
        &mut self,
        lang: &'static Language,
        root: &Path,
    ) -> Result<Option<&'static str>, Closed> {
        let program = self
            .found
            .entry(lang.server)
            .or_insert_with(|| which(lang.program))
            .clone();
        let Some(program) = program else {
            self.out.send(FeedEvent::Upsert(answers::server_row(
                lang,
                root,
                &Shown::Missing,
            )))?;
            return Ok(None);
        };
        self.serial = self.serial.saturating_add(1);
        match start(lang, &program, root, self.serial, &self.events) {
            Ok(server) => {
                self.servers.insert(lang.server, server);
                self.publish(lang.server)?;
                Ok(Some(lang.server))
            }
            Err(why) => {
                let why = format!("could not start {}: {why}", program.display());
                let row = answers::server_row(lang, root, &Shown::Failed(&why, &Stats::default()));
                self.out.send(FeedEvent::Upsert(row))?;
                Ok(None)
            }
        }
    }

    /// Sends the file's text to its server: opened the first time, changed when it differs.
    fn open(
        &mut self,
        name: &'static str,
        path: &str,
        root: &Path,
    ) -> Result<Option<PathBuf>, Closed> {
        let Some(abs) = inside(root, path) else {
            return self
                .refuse(format!("{path} is not a file in the working folder"))
                .map(|()| None);
        };
        let Some(text) = answers::read_text(&abs) else {
            return self
                .refuse(format!("{path} is not UTF-8 text under 2 MiB"))
                .map(|()| None);
        };
        let Some(server) = self.servers.get_mut(name) else {
            return Ok(None);
        };
        let uri = protocol::uri_of(&abs);
        let sent = match server.docs.get_mut(path) {
            Some(doc) if doc.text == text => Ok(()),
            Some(doc) => {
                doc.version = doc.version.saturating_add(1);
                doc.text.clone_from(&text);
                let version = doc.version;
                let change = json!({"textDocument": {"uri": uri, "version": version}, "contentChanges": [{"text": text}]});
                server.note("textDocument/didChange", change)
            }
            None => {
                let item = json!({"textDocument": {"uri": uri, "languageId": server.lang.id, "version": 1, "text": text}});
                server
                    .docs
                    .insert(path.to_owned(), Doc { version: 1, text });
                server.note("textDocument/didOpen", item)
            }
        };
        match sent {
            Ok(()) => Ok(Some(abs)),
            Err(why) => self.fail(name, why).map(|()| None),
        }
    }

    fn ask(&mut self, name: &'static str, ask: Ask, abs: &Path) -> Result<(), Closed> {
        let Some(server) = self.servers.get_mut(name) else {
            return Ok(());
        };
        let uri = protocol::uri_of(abs);
        let text = server
            .docs
            .get(ask.path().unwrap_or_default())
            .map(|d| d.text.as_str());
        let at = |spot: &Spot| {
            let line = spot.line.saturating_sub(1);
            let words = text.and_then(|t| t.lines().nth(line)).unwrap_or_default();
            json!({"line": line, "character": protocol::utf16_of(words, spot.col.saturating_sub(1))})
        };
        let doc = json!({"uri": uri});
        let params = match &ask {
            Ask::Initialize => return Ok(()),
            Ask::Hover(s) | Ask::Definition(s) => json!({"textDocument": doc, "position": at(s)}),
            Ask::References(s) => {
                json!({"textDocument": doc, "position": at(s), "context": {"includeDeclaration": true}})
            }
            Ask::Hints(_) => {
                let lines = text.map_or(0, |t| t.lines().count());
                json!({"textDocument": doc, "range": {"start": {"line": 0, "character": 0}, "end": {"line": lines, "character": 0}}})
            }
            Ask::Fixes(s) => {
                let on_line = |d: &&Value| {
                    d.pointer("/range/start/line").and_then(Value::as_u64)
                        == u64::try_from(s.line.saturating_sub(1)).ok()
                };
                let known = self.diagnostics.get(&s.path).into_iter().flatten();
                let diagnostics: Vec<&Value> = known.filter(on_line).collect();
                json!({"textDocument": doc, "range": {"start": at(s), "end": at(s)}, "context": {"diagnostics": diagnostics}})
            }
        };
        match server.call(ask, params) {
            Ok(()) => Ok(()),
            Err(why) => self.fail(name, why),
        }
    }

    fn message(&mut self, serial: u64, message: Value) -> Result<(), Closed> {
        let Some(name) = self.name_of(serial) else {
            return Ok(());
        };
        match protocol::classify(message) {
            Incoming::Answer { id, result } => self.answer(name, id, result),
            Incoming::Ask { id, method, params } => {
                let result = match method.as_str() {
                    "workspace/configuration" => {
                        let items = params
                            .get("items")
                            .and_then(Value::as_array)
                            .map_or(0, Vec::len);
                        Value::Array(vec![Value::Null; items.min(QUEUE)])
                    }
                    _ => Value::Null,
                };
                let sent = self
                    .servers
                    .get_mut(name)
                    .map(|s| s.send(&protocol::answer(id, result)));
                match sent {
                    Some(Err(why)) => self.fail(name, why),
                    Some(Ok(())) | None => Ok(()),
                }
            }
            Incoming::Note { method, params } if method == "textDocument/publishDiagnostics" => {
                self.diagnosed(name, &params)
            }
            Incoming::Note { method, params } if method == "experimental/serverStatus" => {
                match params.get("quiescent").and_then(Value::as_bool) {
                    Some(true) => self.retell(name),
                    _ => Ok(()),
                }
            }
            Incoming::Note { .. } | Incoming::Other => Ok(()),
        }
    }

    fn name_of(&self, serial: u64) -> Option<&'static str> {
        self.servers
            .iter()
            .find(|(_, s)| s.serial == serial)
            .map(|(name, _)| *name)
    }

    fn answer(
        &mut self,
        name: &'static str,
        id: u64,
        result: Result<Value, String>,
    ) -> Result<(), Closed> {
        let (Some(root), Some(server)) = (self.root.clone(), self.servers.get_mut(name)) else {
            return Ok(());
        };
        let Some(pending) = server.pending.remove(&id) else {
            return Ok(());
        };
        let result = match (&pending.ask, result) {
            (Ask::Initialize, Ok(result)) => return self.started(name, &result),
            (Ask::Initialize, Err(why)) => {
                return self.fail(name, format!("initialize failed: {why}"));
            }
            (_, result) => result,
        };
        server.stats.answered = server.stats.answered.saturating_add(1);
        server.stats.slowest = server.stats.slowest.max(pending.sent.elapsed());
        let Ok(result) = result else {
            return self.publish(name);
        };
        let mut texts = Texts::new(&root, &server.docs);
        let (scope, rows) = match &pending.ask {
            Ask::Hover(s) => (format!("hover:{}", s.path), answers::hover(s, &result)),
            Ask::Definition(s) => (
                format!("def:{}", s.path),
                answers::definition(s, &result, &mut texts),
            ),
            Ask::References(s) => (
                "ref".to_owned(),
                answers::references(s, &result, &mut texts),
            ),
            Ask::Hints(path) => {
                let abs = root.join(path);
                (
                    format!("hint:{path}"),
                    answers::hints(path, &abs, &result, &mut texts),
                )
            }
            Ask::Fixes(s) => {
                let found = answers::fixes(s, &result, &root, &mut texts);
                let rows = found.iter().map(|(row, _, _)| row.clone()).collect();
                self.fixes
                    .retain(|id, _| !id.starts_with(&format!("fix:{}:", s.path)));
                for (row, change, diff) in found {
                    self.fixes
                        .insert(row.id.as_str().to_owned(), (change, diff));
                }
                (format!("fix:{}", s.path), rows)
            }
            Ask::Initialize => return Ok(()),
        };
        if matches!(pending.ask, Ask::References(_) | Ask::Hints(_)) {
            server.stats.cut = texts.cut;
        }
        self.replace(scope, rows)?;
        self.publish(name)
    }

    /// The server answered `initialize`: say so, then send what waited for it.
    fn started(&mut self, name: &'static str, result: &Value) -> Result<(), Closed> {
        let Some(server) = self.servers.get_mut(name) else {
            return Ok(());
        };
        server.stats.can = answers::can_do(result);
        server.state = State::Ready;
        let mut sent = server.send(&protocol::notification("initialized", json!({})));
        for out in std::mem::take(&mut server.held) {
            sent = sent.and_then(|()| match out {
                Out::Note(method, params) => server.send(&protocol::notification(method, params)),
                Out::Call(ask, params) => server.call(ask, params),
            });
        }
        match sent {
            Ok(()) => self.publish(name),
            Err(why) => self.fail(name, why),
        }
    }

    fn diagnosed(&mut self, name: &'static str, params: &Value) -> Result<(), Closed> {
        let (Some(root), Some(server)) = (self.root.clone(), self.servers.get_mut(name)) else {
            return Ok(());
        };
        let Some(uri) = params.get("uri").and_then(Value::as_str) else {
            return Ok(());
        };
        let Some(abs) = protocol::path_of(uri) else {
            return Ok(());
        };
        let list = params
            .get("diagnostics")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut texts = Texts::new(&root, &server.docs);
        let rows = answers::problems(server.lang.server, uri, &list, &mut texts);
        server.stats.cut = texts.cut;
        let path = answers::rel_of(&root, &abs);
        self.diagnostics.insert(path.clone(), list);
        self.replace(format!("problem:{path}"), rows)?;
        self.publish(name)
    }

    /// Incident: rust-analyzer reports none of the errors of a file it was told of before its
    /// workspace loaded, so each file is closed and opened again the first time it is idle.
    fn retell(&mut self, name: &'static str) -> Result<(), Closed> {
        let (Some(root), Some(server)) = (self.root.clone(), self.servers.get_mut(name)) else {
            return Ok(());
        };
        if std::mem::replace(&mut server.idle, true) {
            return Ok(());
        }
        let lang = server.lang.id;
        let mut told = Vec::new();
        for (path, doc) in &mut server.docs {
            let Some(abs) = inside(&root, path) else {
                continue;
            };
            let uri = protocol::uri_of(&abs);
            doc.version = doc.version.saturating_add(1);
            let item =
                json!({"uri": uri, "languageId": lang, "version": doc.version, "text": doc.text});
            told.push((
                json!({"textDocument": {"uri": uri}}),
                json!({"textDocument": item}),
            ));
        }
        // A question in flight is answered for the closed file, if at all: it is asked again.
        let waiting = server.pending.values().map(|p| p.ask.clone());
        let again: Vec<Ask> = waiting.filter(|a| !matches!(a, Ask::Initialize)).collect();
        let mut sent = Ok(());
        for (close, open) in told {
            sent = sent
                .and_then(|()| server.note("textDocument/didClose", close))
                .and_then(|()| server.note("textDocument/didOpen", open));
        }
        if let Err(why) = sent {
            return self.fail(name, why);
        }
        for ask in again {
            if let Some(abs) = ask.path().and_then(|path| inside(&root, path)) {
                self.ask(name, ask, &abs)?;
            }
        }
        Ok(())
    }

    /// Sends `rows` as the answer for `scope`, removing the rows its last answer sent.
    fn replace(&mut self, scope: String, rows: Vec<Row>) -> Result<(), Closed> {
        let ids: Vec<RowId> = rows.iter().map(|r| r.id.clone()).collect();
        for old in self.shown.remove(&scope).unwrap_or_default() {
            if !ids.contains(&old) {
                self.out.send(FeedEvent::Remove(old))?;
            }
        }
        for row in rows {
            self.out.send(FeedEvent::Upsert(row))?;
        }
        self.shown.insert(scope, ids);
        Ok(())
    }

    fn publish(&self, name: &'static str) -> Result<(), Closed> {
        let (Some(root), Some(server)) = (&self.root, self.servers.get(name)) else {
            return Ok(());
        };
        let row = answers::server_row(server.lang, root, &server.shown());
        self.out.send(FeedEvent::Upsert(row))
    }

    /// Ends a server with an error its row shows; its group is killed and reaped.
    fn fail(&mut self, name: &'static str, why: String) -> Result<(), Closed> {
        let Some(server) = self.servers.get_mut(name) else {
            return Ok(());
        };
        if !matches!(server.state, State::Failed(_)) {
            kill(&mut server.child);
            server.pending.clear();
            server.held.clear();
            server.state = State::Failed(why);
        }
        self.publish(name)
    }

    fn ended(&mut self, serial: u64, end: End) -> Result<(), Closed> {
        let Some(name) = self.name_of(serial) else {
            return Ok(());
        };
        let Some(server) = self.servers.get_mut(name) else {
            return Ok(());
        };
        if matches!(server.state, State::Failed(_)) {
            return Ok(());
        }
        let waited = server.pending.len();
        let status = match &end {
            End::Read(ReadError::Eof) | End::Write(_) => reap(&mut server.child, GRACE),
            End::Read(_) => None,
        };
        let mut why = match (end, status) {
            (End::Read(ReadError::Eof) | End::Write(_), Some(status)) => exited(status),
            (End::Read(ReadError::Eof), None) => "closed its output".to_owned(),
            (End::Write(error), None) => format!("could not be written to: {error}"),
            (End::Read(error), _) => error.to_string(),
        };
        // Invariant: only an exited server's stderr is sure to close, so only it is waited on.
        let said = status.and_then(|_| server.said.recv_timeout(GRACE).ok());
        if let Some(said) = said.filter(|said| !said.is_empty()) {
            why.push_str(&format!(": {said}"));
        }
        if waited > 0 {
            why.push_str(&format!(
                " while {waited} request{} waited",
                if waited == 1 { "" } else { "s" }
            ));
        }
        self.fail(name, why)
    }

    fn next_deadline(&self) -> Option<Instant> {
        let pending = self.servers.values().flat_map(|s| s.pending.values());
        pending.filter_map(server::Pending::due).min()
    }

    /// Drops questions past their limit; an unanswered `initialize` ends its server.
    fn expire(&mut self) -> Result<(), Closed> {
        let now = Instant::now();
        let names: Vec<&'static str> = self.servers.keys().copied().collect();
        for name in names {
            let Some(server) = self.servers.get_mut(name) else {
                continue;
            };
            let late: Vec<(u64, Ask)> = server
                .pending
                .iter()
                .filter(|(_, p)| p.due().is_some_and(|at| at <= now))
                .map(|(id, p)| (*id, p.ask.clone()))
                .collect();
            if late.is_empty() {
                continue;
            }
            if late.iter().any(|(_, ask)| matches!(ask, Ask::Initialize)) {
                self.fail(
                    name,
                    format!("it did not answer initialize within {} s", START.as_secs()),
                )?;
                continue;
            }
            let mut sent = Ok(());
            for (id, _) in &late {
                server.pending.remove(id);
                server.stats.late = server.stats.late.saturating_add(1);
                sent = sent.and_then(|()| {
                    server.send(&protocol::notification(
                        "$/cancelRequest",
                        json!({"id": id}),
                    ))
                });
            }
            match sent {
                Ok(()) => {
                    self.publish(name)?;
                    // Invariant: after the row, whose upsert would put the feed back to live.
                    for (_, ask) in &late {
                        self.tell(ask, format!("no answer within {} s", TIMEOUT.as_secs()))?;
                    }
                }
                Err(why) => self.fail(name, why)?,
            }
        }
        Ok(())
    }

    /// Restarts the server a `server:` address names, or every server, reopening its files.
    fn restart(&mut self, run: &Run, root: &Path) -> Result<(), Closed> {
        let named = match &run.target {
            Target::Thing(address) => address.id().strip_prefix("server:"),
            Target::App | Target::Pane(_) => None,
        };
        let langs: Vec<&'static Language> = self
            .languages
            .iter()
            .filter(|l| named.map_or(self.servers.contains_key(l.server), |n| n == l.server))
            .collect();
        for lang in langs {
            let docs: Vec<String> = self
                .servers
                .get(lang.server)
                .map(|s| s.docs.keys().cloned().collect())
                .unwrap_or_default();
            if let Some(server) = self.servers.remove(lang.server) {
                stop(vec![server]);
            }
            self.found.remove(lang.server);
            if self.launch(lang, root)?.is_some() {
                for doc in docs {
                    self.open(lang.server, &doc, root)?;
                }
            }
        }
        Ok(())
    }

    fn apply_fix(&mut self, run: &Run, root: &Path) -> Result<(), Closed> {
        let id = match &run.target {
            Target::Thing(address) => Some(address.id().to_owned()),
            Target::App | Target::Pane(_) => run.text.clone(),
        };
        let Some((change, shown)) = id.as_ref().and_then(|id| self.fixes.get(id)).cloned() else {
            return self.refuse("that fix is no longer offered: ask for fixes again");
        };
        match answers::write_fix(root, &change, &shown) {
            Ok(paths) => {
                for abs in paths {
                    let path = answers::rel_of(root, &abs);
                    let names: Vec<&'static str> = self
                        .servers
                        .iter()
                        .filter(|(_, s)| s.docs.contains_key(&path))
                        .map(|(n, _)| *n)
                        .collect();
                    for name in names {
                        self.open(name, &path, root)?;
                    }
                    self.replace(format!("fix:{path}"), Vec::new())?;
                    self.fixes
                        .retain(|id, _| !id.starts_with(&format!("fix:{path}:")));
                }
                Ok(())
            }
            Err(why) => self.refuse(why.to_string()),
        }
    }

    fn stop_all(&mut self) {
        stop(std::mem::take(&mut self.servers).into_values().collect());
        self.shown.clear();
        self.fixes.clear();
        self.diagnostics.clear();
    }
}

#[cfg(test)]
mod tests;
