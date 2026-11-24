//! One server event into the model, and one wire-routed command into a request.

pub mod commands;
#[cfg(test)]
pub(crate) mod tests;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::command::table::Run as Command;
use crate::ids::{AgentId, Millis, ThreadId};
use crate::model::Model;
use crate::msg::Effect;
use crate::reduce::agents;
use crate::reduce::pointed;
use crate::reduce::state::{Asked, Failure, Held, Link, MAX_FAILURES, Pending, Wire};
use crate::reduce::threads::{MAX_THREADS, Run, Thread, Workdir};
use crate::reduce::usage::Tokens;
use crate::wire::event::{Down, WireEvent};
use crate::wire::frame::{Frame, Kept, Refusal, Update};
use crate::wire::replay::{Sync, Taken};
use crate::wire::request::{self, Request, RequestId};
use crate::wire::shapes::AcpConfigOption;

/// How long a request may wait for its answer. A prompt is not held to it: the server
/// answers one only when the thread takes it, which may be turns away.
pub const DEADLINE: Millis = Millis(30_000);
/// The server's code for a worker that is being swapped: the request may be made again.
const BUSY: i64 = -32000;

/// Folds one event from the wire client into the model.
pub fn step(model: &mut Model, event: WireEvent) -> Vec<Effect> {
    let mut out = Vec::new();
    let now = model.now;
    let wire = &mut model.wire;
    match event {
        WireEvent::Line(line) => {
            let at = wire.clock(now);
            match Frame::decode(line.as_bytes()) {
                Ok(frame) => wire.frame(frame, at, &mut out),
                Err(_) => wire.dropped = wire.dropped.saturating_add(1),
            }
        }
        WireEvent::Frame { at, frame } => {
            let at = wire.clock(at);
            wire.frame(*frame, at, &mut out);
        }
        WireEvent::Up {
            at,
            server,
            latency,
        } => {
            let at = wire.clock(at);
            wire.up(server, at, latency, &mut out);
        }
        WireEvent::Down { at, why } => {
            let at = wire.clock(at);
            wire.down(why, at);
        }
        WireEvent::Unsent { at, id } => {
            let at = wire.clock(at);
            wire.settle(&id, Err(Refusal::Unsent), at, &mut out);
        }
        WireEvent::Poll { at } => {
            let at = wire.clock(at);
            wire.folders = model.settings.projects();
            wire.poll(at, &mut out);
        }
        WireEvent::Bad { at, .. } => {
            wire.clock(at);
            wire.dropped = wire.dropped.saturating_add(1);
        }
    }
    // Incident: nothing ticks the running app's clock, so a 3 minute turn read "0m". The time
    // the wire's events carry moves it.
    model.now = model.now.max(model.wire.seen);
    // Invariant: an act is dated no earlier than it happened, by the next time the wire
    // tells: the person is never found idle sooner than they are.
    if std::mem::take(&mut model.acted) {
        model.touched = model.now;
    }
    let mut effects: Vec<Effect> = out.into_iter().map(Effect::Wire).collect();
    for (id, params) in std::mem::take(&mut model.wire.shows) {
        effects.extend(pointed::answer(model, &id, &params));
    }
    effects
}

/// Carries out a command routed to the server: the requests it makes.
pub fn request(model: &mut Model, run: &Command) -> Vec<Effect> {
    let mut out = Vec::new();
    let at = model.wire.clock(model.now);
    model.wire.run(run, &model.attached.things, at, &mut out);
    out.into_iter().map(Effect::Wire).collect()
}

/// The options an answer carries. A server still building the session answers with none,
/// and sends them in an update: an empty list says nothing.
fn options(result: &Value) -> Option<Vec<AcpConfigOption>> {
    let options: Vec<AcpConfigOption> = Vec::deserialize(result.get("configOptions")?).ok()?;
    (!options.is_empty()).then_some(options)
}

fn text(result: &Value, at: &str) -> Option<String> {
    result
        .pointer(at)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

impl Wire {
    fn frame(&mut self, frame: Frame, at: Millis, out: &mut Vec<Request>) {
        match frame {
            Frame::Update { thread, update } => self.update(thread, update, at, out),
            Frame::Ask { id, params } => self.ask(id, *params, at, out),
            Frame::Reply { id, result } => self.settle(&id, result, at, out),
            Frame::Show { id, params } => self.shows.push((id, *params)),
            Frame::Request { id, method } => out.push(Request::refuse(&id, &method)),
            Frame::Other(_) => {}
        }
        self.release(at, out);
    }

    /// The thread `id`, made if the server speaks of one this app has not heard of.
    fn thread(&mut self, id: &ThreadId) -> &mut Thread {
        self.threads
            .entry(id.clone())
            .or_insert_with(|| Thread::new(id.clone()))
    }

    fn update(&mut self, id: ThreadId, update: Update, at: Millis, out: &mut Vec<Request>) {
        // Invariant: the server names threads at will, so the ones it can add are counted.
        if self.threads.len() >= MAX_THREADS && !self.threads.contains_key(&id) {
            return;
        }
        match update {
            Update::Event { seq, child, event } => {
                if let Kept::Known(event) = event.as_ref() {
                    self.permission(&id, event);
                }
                let thread = self.thread(&id);
                match thread.sync.take(seq) {
                    Taken::Next => thread.event(child, *event, at),
                    Taken::Hole => self.resume(&id, at, out),
                    Taken::Apart => {}
                }
            }
            Update::Gap | Update::Replaced => {
                if self.thread(&id).sync.live() {
                    self.resume(&id, at, out);
                }
            }
            Update::Replay(replay) => {
                let own = replay.child.is_none();
                let answered = self.thread(&id).replay(*replay);
                self.answered(&id, &answered);
                self.taken(&id);
                if own {
                    self.relink(&id);
                    self.lose(&id);
                }
            }
            Update::Committed { entries, notice } => {
                let thread = self.thread(&id);
                if let Some(notice) = notice {
                    thread.note(at, notice);
                }
                // Before its branch is replayed a thread has no transcript to add to.
                if thread.sync.live() {
                    let answered = thread.commit(entries);
                    self.answered(&id, &answered);
                    self.taken(&id);
                }
            }
            Update::State(run) => self.state(&id, run, at),
            other => self.thread(&id).set(other, at),
        }
    }

    /// The server's word on what a thread is doing.
    fn state(&mut self, id: &ThreadId, run: Run, at: Millis) {
        let thread = self.thread(id);
        thread.last_activity = thread.last_activity.max(at);
        // The server counts a change nobody watched; this app counts one no view showed.
        if thread.views == 0 {
            thread.unseen_here = thread.unseen_here.saturating_add(1);
        }
        let over = matches!(run, Run::Idle { .. });
        thread.state = run;
        if over {
            // A run that ended waits on nothing: its asks are moot.
            self.asks.retain(|ask| &ask.thread != id);
        }
    }

    /// Asks for a thread's whole branch again. Its events pass by until the replay starts.
    pub(crate) fn resume(&mut self, id: &ThreadId, at: Millis, out: &mut Vec<Request>) {
        let thread = self.thread(id);
        if thread.sync == Sync::Resuming {
            return;
        }
        let params = request::resume(id, &thread.cwd);
        self.call(request::RESUME, params, at, out);
    }

    fn up(&mut self, server: String, at: Millis, latency: Option<Millis>, out: &mut Vec<Request>) {
        self.link = Link::Up {
            server,
            since: at,
            latency,
        };
        let shown = self.threads.values().filter(|thread| thread.views > 0);
        let shown: Vec<ThreadId> = shown.map(|thread| thread.id.clone()).collect();
        for id in shown {
            self.resume(&id, at, out);
        }
        out.extend(self.answers());
    }

    /// The connection is gone, and with it every watch and every answer still owed.
    fn down(&mut self, why: Down, at: Millis) {
        self.link = Link::Down { why, since: at };
        for thread in self.threads.values_mut() {
            thread.sync = Sync::Apart;
        }
        let owed: Vec<RequestId> = self.pending.keys().cloned().collect();
        for id in owed {
            self.settle(&id, Err(Refusal::Lost), at, &mut Vec::new());
        }
    }

    fn poll(&mut self, at: Millis, out: &mut Vec<Request>) {
        let late = |asked_at: Millis| at.0.saturating_sub(asked_at.0) > DEADLINE.0;
        let silent = self.pending.iter().filter(|(_, pending)| {
            late(pending.at) && !matches!(pending.asked, Asked::Prompt { .. })
        });
        let silent: Vec<RequestId> = silent.map(|(id, _)| id.clone()).collect();
        for id in silent {
            self.settle(&id, Err(Refusal::TimedOut), at, out);
        }
        if !matches!(self.link, Link::Up { .. }) {
            return;
        }
        if !self.listing() {
            // Without a cwd Yi lists only what its daemon holds live; a folder's own list
            // is what it has stored, so a thread outlives a restart of the server.
            for folder in self.folders.clone() {
                self.call(request::LIST, json!({"cwd": folder}), at, out);
            }
            self.call(request::LIST, json!({}), at, out);
        }
        let shown = |thread: &&Thread| thread.views > 0 && thread.sync == Sync::Broken;
        let broken = self.threads.values().filter(shown);
        let broken: Vec<ThreadId> = broken.map(|thread| thread.id.clone()).collect();
        for id in broken {
            self.resume(&id, at, out);
        }
    }

    /// Reads the answer to a request by what the request asked.
    pub(crate) fn settle(
        &mut self,
        id: &RequestId,
        result: Result<Value, Refusal>,
        at: Millis,
        out: &mut Vec<Request>,
    ) {
        let Some(pending) = self.pending.remove(id) else {
            return;
        };
        let result = match result {
            Ok(result) => result,
            Err(why) => return self.fail(id, pending, why, at),
        };
        let asked = pending.asked;
        match asked {
            Asked::Threads(folder) => {
                self.listed |= !self.listing();
                self.merge(&result, folder.as_deref());
            }
            Asked::Settings => {
                if let Some(Value::Array(rows)) = result.get("rows") {
                    self.settings = rows.iter().cloned().map(Kept::read).collect();
                }
            }
            Asked::New { cwd, first } => {
                let Some(made) = text(&result, "/sessionId").map(ThreadId::new) else {
                    return;
                };
                let thread = self.thread(&made);
                thread.cwd = cwd;
                thread.born = Some(id.clone());
                thread.sync = Sync::Live(None);
                thread.last_activity = thread.last_activity.max(at);
                if let Some(options) = options(&result) {
                    thread.options = options;
                }
                if !first.is_empty() {
                    self.call(request::PROMPT, request::prompt(&made, &first), at, out);
                }
            }
            Asked::Fork(from) => {
                let Some(made) = text(&result, "/sessionId").map(ThreadId::new) else {
                    return;
                };
                let source = self.threads.get(&from);
                let cwd = source.map(|thread| thread.cwd.clone()).unwrap_or_default();
                let title = source.and_then(|thread| thread.title.clone());
                let thread = self.thread(&made);
                thread.cwd = cwd;
                thread.born = Some(id.clone());
                thread.title = text(&result, "/_meta/yi/name").or(title);
                thread.prefill = text(&result, "/_meta/yi/prefill");
                thread.forked_from = Some(from);
                thread.last_activity = thread.last_activity.max(at);
            }
            Asked::Resume(resumed) => {
                let thread = self.thread(&resumed);
                thread.title = text(&result, "/_meta/yi/name").or(thread.title.take());
                if let Some(options) = options(&result) {
                    thread.options = options;
                }
                // The replay comes before this answer; one that sent no frame is whole.
                if thread.sync == Sync::Resuming {
                    thread.sync = Sync::Live(None);
                }
            }
            Asked::Options(set) => {
                if let Some(options) = options(&result) {
                    self.thread(&set).options = options;
                }
            }
            Asked::Pin(pinned) => {
                let since = result.get("pinnedMs").and_then(Value::as_u64);
                self.thread(&pinned).pinned = since.map(Millis);
            }
            Asked::Rewind(rewound) => {
                let unsent = text(&result, "/unsent").filter(|unsent| !unsent.is_empty());
                self.thread(&rewound).prefill = unsent;
            }
            Asked::Slash(told) => {
                if let Some(said) = text(&result, "/text") {
                    self.thread(&told).note(at, said);
                }
            }
            Asked::Child { thread, child } => self.gone(&thread, &child, false, at),
            Asked::Prompt { .. } | Asked::Seen(_) | Asked::Other { .. } => {}
        }
    }

    /// Whether the server still holds a thread's child, by how it answered for its replay.
    fn gone(&mut self, thread: &ThreadId, child: &AgentId, gone: bool, at: Millis) {
        let agents = &mut self.thread(thread).agents;
        if let Some(agent) = crate::reduce::agents::agent(agents, child, at) {
            agent.gone = gone;
        }
    }

    /// Keeps a request that got no result, and puts back what it would have changed.
    fn fail(&mut self, id: &RequestId, pending: Pending, why: Refusal, at: Millis) {
        let asked = pending.asked;
        match (&asked, &why) {
            // Yi takes a prompt as it reads it and answers later: a lost answer is no refusal.
            (Asked::Prompt { thread, text }, Refusal::Lost | Refusal::TimedOut) => {
                let (thread, text, before) = (thread.clone(), text.clone(), pending.before);
                return self.held.push(Held {
                    thread,
                    text,
                    before,
                });
            }
            (Asked::Prompt { thread, text }, Refusal::Unsent | Refusal::Server { .. }) => {
                // Several prompts may be lost at once; none of the user's words are dropped.
                let prefill = &mut self.thread(thread).prefill;
                *prefill = Some(match prefill.take() {
                    Some(kept) => format!("{kept}\n{text}"),
                    None => text.clone(),
                });
            }
            (Asked::Resume(thread), _) => {
                // A refusal stands; anything else, the busy worker's code included, may pass.
                let stands = matches!(&why, Refusal::Server { code, .. } if *code != BUSY);
                let thread = self.thread(thread);
                if thread.sync == Sync::Resuming {
                    thread.sync = if stands { Sync::Apart } else { Sync::Broken };
                }
            }
            (Asked::Child { thread, child }, Refusal::Server { .. }) => {
                self.gone(thread, child, true, at);
            }
            // A folder the server refuses to list does not hold back the rest of the round.
            (Asked::Threads(_), Refusal::Server { .. }) => self.listed |= !self.listing(),
            (
                Asked::Threads(_)
                | Asked::Settings
                | Asked::New { .. }
                | Asked::Options(_)
                | Asked::Pin(_)
                | Asked::Seen(_)
                | Asked::Fork(_)
                | Asked::Rewind(_)
                | Asked::Slash(_)
                | Asked::Child { .. }
                | Asked::Other { .. },
                _,
            ) => {}
        }
        if self.failures.len() >= MAX_FAILURES {
            self.failures.remove(0);
        }
        self.failures.push(Failure {
            id: id.clone(),
            at,
            asked,
            why,
        });
    }
}

impl Thread {
    /// The updates that set one fact about a thread.
    fn set(&mut self, update: Update, at: Millis) {
        match update {
            Update::Context { used, size } => {
                self.usage.used = Tokens(used);
                self.usage.window = Tokens(size);
            }
            Update::Config { options, window } => {
                self.options = options;
                self.usage.window = window.map_or(self.usage.window, Tokens);
            }
            Update::Name(name) => self.title = Some(name),
            Update::Workdir { cwd, lane } => self.workdir = Some(Workdir { cwd, lane }),
            Update::Todo(todos) => self.work.todos = todos,
            Update::Plan(plan) => self.work.plan = plan,
            Update::Goal(goal) => self.work.goal = goal,
            Update::Claims(claims) => self.work.claims = claims,
            Update::Child(update) => agents::report(&mut self.agents, *update, None, at),
            Update::Notice(text) => self.note(at, text),
            Update::Other { kind, fields } => self.keep(kind, fields),
            Update::Echo
            | Update::Event { .. }
            | Update::Gap
            | Update::Replaced
            | Update::Replay(_)
            | Update::Committed { .. }
            | Update::State(_) => {}
        }
    }
}
