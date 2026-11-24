//! What the app knows from the server.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use yi_types::settings::SettingRow;

use crate::ids::{AgentId, EntryId, Millis, ThreadId};
use crate::reduce::approvals::Ask;
use crate::reduce::threads::{Run, Thread};
use crate::wire::event::Down;
use crate::wire::frame::{Kept, Refusal};
use crate::wire::replay::Sync;
use crate::wire::request::{self, Request, RequestId};

/// Failures the model keeps; the oldest go first.
pub const MAX_FAILURES: usize = 32;

/// Everything folded from Yi's server: the threads, the asks, Yi's settings, and what this
/// app has asked and not yet heard back on.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Wire {
    /// Where the socket to the server stands.
    pub link: Link,
    /// Every thread the server lists or this app opened.
    pub threads: BTreeMap<ThreadId, Thread>,
    /// Permission asks waiting on the user, oldest first.
    pub asks: Vec<Ask>,
    /// Yi's own settings, as the server last sent them.
    pub settings: Vec<Kept<SettingRow>>,
    /// The last requests that got no result, oldest first.
    pub failures: Vec<Failure>,
    /// Lines from the server that were not frames.
    pub dropped: u64,
    /// The server has answered a thread list: an empty one now means it has none.
    pub listed: bool,
    pub(crate) pending: BTreeMap<RequestId, Pending>,
    /// Prompts whose answer the connection lost: the server may hold them, so they wait.
    pub(crate) held: Vec<Held>,
    /// Prompts typed while their thread was busy, sent one at a time once it is free.
    pub(crate) later: Vec<Later>,
    /// Threads stopped with prompts unsent: those wait until the person sends them.
    pub(crate) stopped: BTreeSet<ThreadId>,
    pub(crate) next: u64,
    /// The project folders the last poll listed on their own.
    pub(crate) folders: Vec<String>,
    /// The latest time any wire event carried.
    pub(crate) seen: Millis,
    /// The model's asks to show a thing, read off the wire and not yet answered.
    pub(crate) shows: Vec<(Value, yi_types::acp::AcpShowParams)>,
}

/// Where the socket to the server stands.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Link {
    /// The client has not reported yet.
    #[default]
    Starting,
    Up {
        /// The server's version.
        server: String,
        since: Millis,
        /// How long the server took to answer as the link came up, when it was timed.
        latency: Option<Millis>,
    },
    Down {
        why: Down,
        since: Millis,
    },
}

/// What a request asked for: how its answer is read.
#[derive(Debug, Clone, PartialEq)]
pub enum Asked {
    /// The threads of one folder, or with none the server's live ones.
    Threads(Option<String>),
    /// A new thread in `cwd`, and the prompt to send it once it exists.
    New {
        cwd: String,
        first: Vec<Value>,
    },
    Resume(ThreadId),
    Prompt {
        thread: ThreadId,
        text: String,
    },
    Options(ThreadId),
    Pin(ThreadId),
    Seen(ThreadId),
    Fork(ThreadId),
    Rewind(ThreadId),
    Slash(ThreadId),
    /// A child agent's conversation, replayed into its thread's stream.
    Child {
        thread: ThreadId,
        child: AgentId,
    },
    Settings,
    /// A call whose answer carries nothing this app keeps.
    Other {
        method: String,
        thread: Option<ThreadId>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Pending {
    pub(crate) asked: Asked,
    pub(crate) at: Millis,
    /// For a prompt: the stored messages that already read as it when it was sent.
    pub(crate) before: BTreeSet<EntryId>,
}

/// A prompt the server may have taken, until its branch shows a new message that reads as it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Held {
    pub(crate) thread: ThreadId,
    pub(crate) text: String,
    pub(crate) before: BTreeSet<EntryId>,
}

/// A prompt not yet sent: its words, and its blocks with what was attached as it was typed.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Later {
    pub(crate) thread: ThreadId,
    pub(crate) text: String,
    pub(crate) blocks: Vec<Value>,
}

/// A request that got no result, for whoever asked to show.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub id: RequestId,
    pub at: Millis,
    pub asked: Asked,
    pub why: Refusal,
}

impl Asked {
    /// What a call of `method` with `params` asks for.
    pub(crate) fn of(method: &str, params: &Value) -> Self {
        let text = |at: &str| params.pointer(at).and_then(Value::as_str);
        let thread = text("/sessionId").map(ThreadId::new);
        match (method, thread) {
            (request::LIST, _) => Self::Threads(text("/cwd").map(str::to_owned)),
            (request::SETTINGS | request::SET_DEFAULT, _) => Self::Settings,
            (request::NEW, _) => Self::New {
                cwd: text("/cwd").unwrap_or_default().to_owned(),
                first: Vec::new(),
            },
            (request::PROMPT, Some(thread)) => Self::Prompt {
                thread,
                text: text("/prompt/0/text").unwrap_or_default().to_owned(),
            },
            (request::RESUME, Some(thread)) => Self::Resume(thread),
            (request::SET_OPTION, Some(thread)) => Self::Options(thread),
            (request::PIN, Some(thread)) => Self::Pin(thread),
            (request::SEEN, Some(thread)) => Self::Seen(thread),
            (request::FORK, Some(thread)) => Self::Fork(thread),
            (request::REWIND, Some(thread)) => Self::Rewind(thread),
            (request::SLASH, Some(thread)) => Self::Slash(thread),
            (request::CHILD_REPLAY, Some(thread)) => Self::Child {
                thread,
                child: AgentId::new(text("/childId").unwrap_or_default()),
            },
            (method, thread) => Self::Other {
                method: method.to_owned(),
                thread,
            },
        }
    }
}

impl Wire {
    /// The wire's own clock: never earlier than the latest time an event carried.
    pub(crate) fn clock(&mut self, at: Millis) -> Millis {
        self.seen = self.seen.max(at);
        self.seen
    }

    /// What a request changes before its answer comes.
    fn presume(&mut self, asked: &Asked) {
        match asked {
            Asked::Resume(thread) => {
                if let Some(thread) = self.threads.get_mut(thread) {
                    thread.sync = Sync::Resuming;
                }
            }
            Asked::Seen(thread) => {
                if let Some(thread) = self.threads.get_mut(thread) {
                    (thread.unseen_listed, thread.unseen_here) = (0, 0);
                }
            }
            Asked::Threads(_)
            | Asked::Settings
            | Asked::New { .. }
            | Asked::Prompt { .. }
            | Asked::Options(_)
            | Asked::Pin(_)
            | Asked::Fork(_)
            | Asked::Rewind(_)
            | Asked::Slash(_)
            | Asked::Child { .. }
            | Asked::Other { .. } => {}
        }
    }

    /// Whether any thread list is still unanswered.
    pub(crate) fn listing(&self) -> bool {
        let list = |pending: &Pending| matches!(pending.asked, Asked::Threads(_));
        self.pending.values().any(list)
    }

    /// Makes a call to the server and remembers what it asked, to read the answer by.
    pub(crate) fn call(
        &mut self,
        method: &str,
        params: Value,
        at: Millis,
        out: &mut Vec<Request>,
    ) -> RequestId {
        let asked = Asked::of(method, &params);
        self.presume(&asked);
        let before = self.prompted(&asked);
        let request = Request::call(self.next, method, params);
        let id = RequestId::number(self.next);
        self.next = self.next.saturating_add(1);
        self.pending
            .insert(id.clone(), Pending { asked, at, before });
        out.push(request);
        id
    }

    /// Takes a frame a recording's client sent as this app's own: a call is remembered so
    /// its recorded answer is read, and an answer to an ask marks the ask.
    pub(crate) fn sent(&mut self, frame: &Value, at: Millis) {
        let params = frame.get("params").unwrap_or(&Value::Null);
        match (frame.get("method").and_then(Value::as_str), frame.get("id")) {
            (Some(method), Some(id)) => {
                let asked = Asked::of(method, params);
                self.presume(&asked);
                let before = self.prompted(&asked);
                if let Some(number) = id.as_u64() {
                    self.next = self.next.max(number.saturating_add(1));
                }
                let pending = Pending { asked, at, before };
                self.pending.insert(RequestId::of(id), pending);
            }
            (None, Some(id)) => {
                let option = frame.pointer("/result/outcome/optionId");
                self.chose(id, option.and_then(Value::as_str));
            }
            (Some(_) | None, None) => {}
        }
    }

    /// The id the next request will carry. A view reads it before it runs a command, then
    /// finds the thread that command made, or why it failed, by it.
    pub fn ticket(&self) -> RequestId {
        RequestId::number(self.next)
    }

    /// The thread the request `ticket` made.
    pub fn born(&self, ticket: &RequestId) -> Option<&Thread> {
        self.threads
            .values()
            .find(|thread| thread.born.as_ref() == Some(ticket))
    }

    /// Why the request `ticket` got no result, if it failed.
    pub fn failed(&self, ticket: &RequestId) -> Option<&Failure> {
        self.failures.iter().rev().find(|fail| &fail.id == ticket)
    }

    /// Prompts sent to `thread` that the server has not taken yet, oldest first.
    pub fn queued(&self, thread: &ThreadId) -> impl Iterator<Item = &str> + use<'_> {
        let (to, unsent, thread) = (thread.clone(), thread.clone(), thread.clone());
        let held = self.held.iter().filter(move |held| held.thread == to);
        let pending = self
            .pending
            .values()
            .filter_map(move |pending| match &pending.asked {
                Asked::Prompt { thread: to, text } if *to == thread => Some(text.as_str()),
                _ => None,
            });
        let later = self
            .later
            .iter()
            .filter(move |later| later.thread == unsent);
        held.map(|held| held.text.as_str())
            .chain(pending)
            .chain(later.map(|later| later.text.as_str()))
    }

    /// A thread that runs, or has a prompt not yet taken or not yet sent, takes no prompt now.
    /// Incident: Yi held a prompt sent mid-turn as a follow-up, lost it to a Stop, never answered.
    pub(crate) fn busy(&self, thread: &ThreadId) -> bool {
        self.runs(thread) || self.queued(thread).next().is_some()
    }

    fn runs(&self, thread: &ThreadId) -> bool {
        let busy = |t: &Thread| matches!(t.state, Run::Running | Run::NeedsYou | Run::Other(_));
        self.threads.get(thread).is_some_and(busy)
    }

    /// The thread's unsent prompts wait for the person to send them, since it was stopped.
    pub fn holds(&self, thread: &ThreadId) -> bool {
        self.stopped.contains(thread) && self.later.iter().any(|l| &l.thread == thread)
    }

    /// Sends the oldest unsent prompt of each thread that runs nothing, owes no answer and
    /// was not stopped with them waiting.
    pub(crate) fn release(&mut self, at: Millis, out: &mut Vec<Request>) {
        let mut threads: Vec<ThreadId> = self.later.iter().map(|l| l.thread.clone()).collect();
        threads.retain(|thread| !self.stopped.contains(thread));
        threads.dedup();
        for thread in threads {
            let unsent = self.later.iter().filter(|l| l.thread == thread).count();
            let sent = self.queued(&thread).count().saturating_sub(unsent);
            let first = self.later.iter().position(|l| l.thread == thread);
            if let (false, 0, Some(first)) = (self.runs(&thread), sent, first) {
                let later = self.later.remove(first);
                let params = request::prompt(&later.thread, &later.blocks);
                self.call(request::PROMPT, params, at, out);
            }
        }
    }

    /// The stored messages of a prompt's thread that read as it.
    fn prompted(&self, asked: &Asked) -> BTreeSet<EntryId> {
        let Asked::Prompt { thread, text } = asked else {
            return BTreeSet::new();
        };
        let thread = self.threads.get(thread);
        let said = thread.map(|thread| thread.transcript.prompted(text).cloned().collect());
        said.unwrap_or_default()
    }

    /// Lets go of the lost prompts that the branch of `id` now shows the server took.
    pub(crate) fn taken(&mut self, id: &ThreadId) {
        let Some(thread) = self.threads.get(id) else {
            return;
        };
        let mut claimed = BTreeSet::new();
        self.held.retain(|held| {
            let fresh =
                |entry: &&EntryId| !held.before.contains(*entry) && !claimed.contains(*entry);
            let took = (&held.thread == id)
                .then(|| thread.transcript.prompted(&held.text).find(fresh))
                .flatten();
            took.map(|entry| claimed.insert(entry.clone())).is_none()
        });
        // Invariant: one stored message answers for one lost prompt, however many read alike.
        for held in self.held.iter_mut().filter(|held| &held.thread == id) {
            held.before.extend(claimed.iter().cloned());
        }
    }
}
