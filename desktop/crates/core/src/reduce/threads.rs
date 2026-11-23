//! The thread list: titles, states, pins and what is unseen.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use yi_types::acp::{AcpConfigOption, AcpStopReason};
use yi_types::entry::Entry;
use yi_types::event::AgentEvent;
use yi_types::subagent::ChildFlag;

use crate::address::uri::{Address, AddressError};
use crate::ids::{AgentId, ApprovalId, Millis, ThreadId};
use crate::kind::Kind;
use crate::reduce::agents::{self, Agent};
use crate::reduce::state::Wire;
use crate::reduce::todos::Work;
use crate::reduce::turns::{CallId, Fact, Message, ToolState, Transcript};
use crate::reduce::usage::{Tokens, Usage};
use crate::wire::frame::{Kept, Replay};
use crate::wire::replay::Sync;
use crate::wire::request::RequestId;

/// What the server says a thread is doing.
#[derive(Debug, Clone, PartialEq)]
pub enum Run {
    /// Nothing runs; `stop` is why the last run stopped, when this app saw it stop.
    Idle {
        stop: Option<AcpStopReason>,
    },
    Running,
    /// A permission ask blocks the run.
    NeedsYou,
    /// A state newer than this app, as written.
    Other(String),
}

impl Default for Run {
    fn default() -> Self {
        Self::Idle { stop: None }
    }
}

/// The stop reason Yi gives a run that failed.
const ERRORED: &str = "_yi/error";

/// A thread's state as the sidebar, the bell and the status items show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    NeedsYou,
    Working,
    /// It stopped on an error.
    Failed,
    /// It finished and the user has not looked.
    Done,
    Idle,
}

/// Where a thread's agent works.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workdir {
    pub cwd: String,
    /// The lane worktree it runs in, when it has one.
    pub lane: Option<String>,
}

/// A line the server or a command said about a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub at: Millis,
    pub text: String,
}

/// Notes one thread keeps; the oldest go first.
pub const MAX_NOTES: usize = 64;
/// Kinds of update newer than this app that one thread keeps the last value of.
pub const MAX_EXTRAS: usize = 64;
/// Threads one answer to `session/list` may name.
pub const MAX_THREADS: usize = 10_000;

/// One thread: Yi's session.
#[derive(Debug, Clone, PartialEq)]
pub struct Thread {
    pub id: ThreadId,
    /// The folder the server runs it in: its project.
    pub cwd: String,
    pub title: Option<String>,
    pub state: Run,
    /// Changes of state the server counted while no client watched.
    pub(crate) unseen_listed: u64,
    /// Changes of state this app saw while no view showed the thread.
    pub(crate) unseen_here: u64,
    /// When it was pinned; the list shows pins first, in this order.
    pub pinned: Option<Millis>,
    pub last_activity: Millis,
    /// How many views show it.
    pub views: u32,
    /// The request that made it, for the view that asked for a new thread.
    pub born: Option<RequestId>,
    pub forked_from: Option<ThreadId>,
    /// Words to put back in the composer: a prompt a rewind or a fork left out, or one the
    /// server never took.
    pub prefill: Option<String>,
    pub workdir: Option<Workdir>,
    /// The model, effort and mode options, each with its choices and its value.
    pub options: Vec<AcpConfigOption>,
    pub usage: Usage,
    pub transcript: Transcript,
    pub work: Work,
    pub agents: Vec<Agent>,
    pub notes: Vec<Note>,
    /// The last update of each kind newer than this app, as written.
    pub extras: BTreeMap<String, Value>,
    pub sync: Sync,
}

/// Why a thread wants the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Want {
    /// A queued permission ask.
    Approval(ApprovalId),
    /// The server says it needs the user, and this app holds no ask for it: opening it asks.
    NeedsYou,
    /// A child agent waits on an answer, or is stuck.
    Child(AgentId),
    Failed,
    Done,
}

/// One row of what needs the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub thread: ThreadId,
    pub want: Want,
    pub since: Millis,
    /// One line of what it wants or said, in the server's own words.
    pub detail: String,
}

/// What the bell, the dock badge and the status items read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Attention {
    /// What needs the user first, then what failed, then what finished; newest first in each.
    pub alerts: Vec<Alert>,
    /// Threads with an agent at work.
    pub working: Vec<ThreadId>,
}

impl Thread {
    pub fn new(id: ThreadId) -> Self {
        Self {
            id,
            cwd: String::new(),
            title: None,
            state: Run::default(),
            unseen_listed: 0,
            unseen_here: 0,
            pinned: None,
            last_activity: Millis::default(),
            views: 0,
            born: None,
            forked_from: None,
            prefill: None,
            workdir: None,
            options: Vec::new(),
            usage: Usage::default(),
            transcript: Transcript::default(),
            work: Work::default(),
            agents: Vec::new(),
            notes: Vec::new(),
            extras: BTreeMap::new(),
            sync: Sync::default(),
        }
    }

    /// `yi://thread/<id>`: what a command aimed at this thread is given.
    pub fn address(&self) -> Result<Address, AddressError> {
        Address::new(Kind::Thread, self.id.as_str())
    }

    /// The git branch Yi gives the thread when it works in a checkout of its own.
    pub fn branch(&self) -> String {
        yi_types::lane::branch_for_session(self.id.as_str())
    }

    /// Changes of state nobody has looked at.
    pub fn unseen(&self) -> u64 {
        self.unseen_listed.saturating_add(self.unseen_here)
    }

    /// Dollars spent: the server's ledger plus the replies finished since it was read.
    pub fn cost(&self) -> f64 {
        let ledger = self.usage.ledger.as_ref();
        ledger.map_or(0.0, |ledger| ledger.cost_total) + self.transcript.since.cost
    }

    /// Answers came and none reported usage: a provider that sends none gives zeros, and
    /// zero tokens at no cost is not what was spent.
    pub fn unreported(&self) -> bool {
        let spent = self.transcript.messages.iter().filter_map(Message::spent);
        let mut spent = spent.peekable();
        let answered = spent.peek().is_some();
        let none = spent.all(|spent| spent.output == 0 && spent.cost <= 0.0);
        answered && none && self.usage.used.0 == 0 && self.cost() <= 0.0
    }

    /// The child agent that made the tool call `call`, when it was not the thread itself:
    /// who an ask is from, which the server does not say.
    pub fn asker(&self, call: &CallId) -> Option<&Agent> {
        let made = |agent: &&Agent| agent.transcript.tools.contains_key(call);
        self.agents.iter().find(made)
    }

    /// The option `id`: `model`, `thought_level` or `mode`.
    pub fn option(&self, id: &str) -> Option<&AcpConfigOption> {
        self.options.iter().find(|option| option.config_id == id)
    }

    /// Whether the last run ended in an error: Yi's only word for it is this stop reason.
    pub fn failed(&self) -> bool {
        matches!(&self.state, Run::Idle { stop: Some(AcpStopReason::Other(why)) } if why == ERRORED)
    }

    /// Whether a step waits on an approval nobody can give any more: see [`ToolState::Lost`].
    pub fn lost(&self) -> bool {
        let tools = self.transcript.tools.values();
        tools.into_iter().any(|call| call.state == ToolState::Lost)
    }

    pub(crate) fn note(&mut self, at: Millis, text: String) {
        if self.notes.len() >= MAX_NOTES {
            self.notes.remove(0);
        }
        self.notes.push(Note { at, text });
    }

    /// Keeps the last update of a kind this build has no fold for.
    pub(crate) fn keep(&mut self, kind: String, fields: Value) {
        if self.extras.len() < MAX_EXTRAS || self.extras.contains_key(&kind) {
            self.extras.insert(kind, fields);
        }
    }

    /// Invariant: folds the stream's next event but leaves the sidebar's order alone: Yi lists
    /// state changes only for a thread no view shows, so a watched one must not move on more.
    pub(crate) fn event(&mut self, child: Option<AgentId>, event: Kept<AgentEvent>, at: Millis) {
        let event = match event {
            Kept::Known(event) => event,
            Kept::Raw(raw) => return self.transcript.keep(raw),
        };
        if let AgentEvent::ChildUpdate { update } = event {
            return agents::report(&mut self.agents, update, child, at);
        }
        // Invariant: a child's ask comes on the thread's own stream, with no child named.
        let (call, allowed) = match (&event, &child) {
            (AgentEvent::PermissionRequested { tool_call_id, .. }, None) => (tool_call_id, None),
            (
                AgentEvent::PermissionResolved {
                    tool_call_id,
                    allowed,
                },
                None,
            ) => (tool_call_id, Some(*allowed)),
            _ => return self.say(child, event, at),
        };
        if !self.transcript.gate(call, allowed, at) {
            let mut children = self.agents.iter_mut();
            children.any(|agent| agent.transcript.gate(call, allowed, at));
        }
    }

    /// Folds an event into the transcript of whoever it is from.
    fn say(&mut self, child: Option<AgentId>, event: AgentEvent, at: Millis) {
        match child {
            None => self.transcript.event(event, at),
            Some(id) => {
                if let Some(agent) = agents::agent(&mut self.agents, &id, at) {
                    agent.last_seen = at;
                    agent.transcript.child_event(event, at);
                }
            }
        }
    }

    /// Takes in entries the server just stored; gives the tool calls whose asks they answer.
    pub(crate) fn commit(&mut self, entries: Vec<Kept<Entry>>) -> Vec<CallId> {
        let facts: Vec<Fact> = entries
            .into_iter()
            .filter_map(|entry| self.transcript.absorb(entry, true))
            .collect();
        self.facts(facts)
    }

    /// Takes in one replay frame; gives the tool calls whose asks its entries answer.
    pub(crate) fn replay(&mut self, replay: Replay) -> Vec<CallId> {
        let whole = replay.from == 0;
        let facts = match &replay.child {
            Some(child) => {
                let at = self.last_activity;
                let Some(agent) = agents::agent(&mut self.agents, child, at) else {
                    return Vec::new();
                };
                fold(&mut agent.transcript, replay.entries, whole)
            }
            None => {
                self.sync = Sync::Live(None);
                self.title = replay.name.or(self.title.take());
                self.work.todos = replay.todos.or(self.work.todos.take());
                self.work.goal = replay.goal;
                self.usage.window = replay.window.map_or(self.usage.window, Tokens);
                if replay.stats.is_some() {
                    self.usage.ledger = replay.stats;
                }
                fold(&mut self.transcript, replay.entries, whole)
            }
        };
        for (part, raw) in replay.unread {
            self.keep(format!("_yi/replay.{part}"), raw);
        }
        self.facts(facts)
    }

    fn facts(&mut self, facts: Vec<Fact>) -> Vec<CallId> {
        let mut answered = Vec::new();
        for fact in facts {
            match fact {
                Fact::Permission(tool) => answered.push(tool),
                Fact::Child(trail, at) => agents::trail(&mut self.agents, trail, at),
            }
        }
        answered
    }
}

/// A replay frame into a transcript: from the branch's start it replaces what was there.
fn fold(transcript: &mut Transcript, entries: Vec<Kept<Entry>>, whole: bool) -> Vec<Fact> {
    if whole {
        return transcript.rebuild(entries);
    }
    entries
        .into_iter()
        .filter_map(|entry| transcript.absorb(entry, false))
        .collect()
}

fn text(row: &Value, key: &str) -> Option<String> {
    row.get(key).and_then(Value::as_str).map(str::to_owned)
}

impl Wire {
    /// A thread's state for a dot: what needs the user wins, then work, then how it ended.
    pub fn status(&self, id: &ThreadId) -> Status {
        let Some(thread) = self.threads.get(id) else {
            return Status::Idle;
        };
        let asked = self.waiting().any(|ask| &ask.thread == id);
        let flagged = thread.agents.iter().any(|agent| agent.flagged.is_some());
        if asked || flagged {
            return Status::NeedsYou;
        }
        match &thread.state {
            Run::NeedsYou if thread.lost() => Status::Idle,
            Run::NeedsYou => Status::NeedsYou,
            Run::Running => Status::Working,
            Run::Idle { .. } if thread.failed() => Status::Failed,
            Run::Idle { .. } if thread.unseen() > 0 => Status::Done,
            Run::Idle { .. } | Run::Other(_) => Status::Idle,
        }
    }

    /// What the bell, the dock badge and the status items show, derived from the threads.
    pub fn attention(&self) -> Attention {
        let mut attention = Attention::default();
        for ask in self.waiting() {
            let detail = ask.description.as_ref().unwrap_or(&ask.title).clone();
            attention.alerts.push(Alert {
                thread: ask.thread.clone(),
                want: Want::Approval(ask.id.clone()),
                since: ask.at,
                detail,
            });
        }
        for (id, thread) in &self.threads {
            let alert = |want, since, detail| Alert {
                thread: id.clone(),
                want,
                since,
                detail,
            };
            for agent in &thread.agents {
                let (Some(since), Some(flag)) = (agent.flagged, &agent.update.flag) else {
                    continue;
                };
                let detail = match flag {
                    ChildFlag::NeedsYou { note } | ChildFlag::Stuck { note } => note.clone(),
                    ChildFlag::Other => String::new(),
                };
                let want = Want::Child(agent.id.clone());
                attention.alerts.push(alert(want, since, detail));
            }
            // An ask held here, answered or not, already says what the thread waits on.
            let held = self.asks.iter().any(|ask| &ask.thread == id);
            match &thread.state {
                Run::NeedsYou if !held && !thread.lost() => {
                    let alert = alert(Want::NeedsYou, thread.last_activity, String::new());
                    attention.alerts.push(alert);
                }
                Run::Running => attention.working.push(id.clone()),
                Run::Idle { .. } if thread.unseen() > 0 => {
                    let want = if thread.failed() {
                        Want::Failed
                    } else {
                        Want::Done
                    };
                    let words = thread.transcript.last_words().unwrap_or_default();
                    attention
                        .alerts
                        .push(alert(want, thread.last_activity, words));
                }
                Run::NeedsYou | Run::Idle { .. } | Run::Other(_) => {}
            }
        }
        let rank = |want: &Want| match want {
            Want::Approval(_) | Want::NeedsYou => 0,
            Want::Child(_) => 1,
            Want::Failed => 2,
            Want::Done => 3,
        };
        attention
            .alerts
            .sort_by(|a, b| (rank(&a.want), b.since).cmp(&(rank(&b.want), a.since)));
        attention
    }

    /// The threads in the order a list shows them: pins first, in the order they were
    /// pinned, then the most recently active.
    pub fn listed(&self) -> Vec<&Thread> {
        let mut threads: Vec<&Thread> = self.threads.values().collect();
        threads.sort_by(|a, b| match (a.pinned, b.pinned) {
            (Some(a), Some(b)) => a.cmp(&b),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => b.last_activity.cmp(&a.last_activity),
        });
        threads
    }

    /// Merges the server's list of sessions into the threads. A thread the list no longer
    /// names goes, unless this app follows it, shows it, or holds an ask for it.
    pub(crate) fn merge(&mut self, result: &Value, folder: Option<&str>) {
        let Some(rows) = result.get("sessions").and_then(Value::as_array) else {
            return;
        };
        let (mut named, mut over, mut waits) = (BTreeSet::new(), BTreeSet::new(), Vec::new());
        for row in rows.iter().take(MAX_THREADS) {
            let Some(id) = text(row, "sessionId").map(ThreadId::new) else {
                continue;
            };
            let thread = self
                .threads
                .entry(id.clone())
                .or_insert_with(|| Thread::new(id.clone()));
            named.insert(id);
            if let Some(cwd) = text(row, "cwd") {
                thread.cwd = cwd;
            }
            thread.title = text(row, "title").or(thread.title.take());
            // A child made by another client is known only from the list: nest it too.
            let parent = row.pointer("/_meta/yi/parentSessionId");
            let parent = parent.or_else(|| row.get("parentSessionId"));
            let parent = parent.and_then(Value::as_str).map(ThreadId::new);
            let parent = parent.filter(|parent| parent != &thread.id);
            thread.forked_from = parent.or(thread.forked_from.take());
            let Some(meta) = row.pointer("/_meta/yi") else {
                continue;
            };
            let number = |key| meta.get(key).and_then(Value::as_u64);
            thread.unseen_listed = number("unseen").unwrap_or(0);
            thread.pinned = number("pinnedMs").map(Millis);
            let last = number("lastEventMs").map_or(thread.last_activity, Millis);
            thread.last_activity = thread.last_activity.max(last);
            // Incident: a project's list names no state, and its silence read as idle.
            let said = meta
                .get("lastState")
                .and_then(Value::as_str)
                .map(|state| match state {
                    "running" => Run::Running,
                    "requires_action" => Run::NeedsYou,
                    "idle" => Run::Idle { stop: None },
                    other => Run::Other(other.to_owned()),
                });
            // A thread followed live knows its state better than a list, once it heard one.
            let heard = thread.sync.live() && thread.state != Run::Idle { stop: None };
            if let Some(said) = said.filter(|_| !heard) {
                thread.state = said;
                let id = thread.id.clone();
                match &thread.state {
                    Run::Idle { .. } => drop(over.insert(id)),
                    Run::NeedsYou if thread.sync.live() => waits.push(id),
                    Run::Running | Run::NeedsYou | Run::Other(_) => {}
                }
            }
        }
        // A run the server says is over waits on nothing: a kept ask of it is moot.
        self.asks.retain(|ask| !over.contains(&ask.thread));
        waits.iter().for_each(|id| self.lose(id));
        let asked: BTreeSet<&ThreadId> = self.asks.iter().map(|ask| &ask.thread).collect();
        // Invariant: a list speaks only for its own folder; without one, for the folders
        // that are not listed on their own.
        let folders = &self.folders;
        let spoken = |cwd: &str| folder.map_or(!folders.iter().any(|f| f == cwd), |f| f == cwd);
        self.threads.retain(|id, thread| {
            let held = thread.views > 0 || matches!(thread.sync, Sync::Live(_) | Sync::Resuming);
            named.contains(id) || held || asked.contains(id) || !spoken(&thread.cwd)
        });
    }
}
