//! The app's own queue of permission asks, kept across reconnects.

use serde_json::Value;
use yi_types::acp::{
    AcpPermissionOption, AcpPermissionOptionKind, AcpPermissionParams, AcpPermissionSubject,
    AcpToolContent,
};
use yi_types::event::AgentEvent;

use crate::address::uri::{Address, AddressError};
use crate::ids::{ApprovalId, Millis, ThreadId};
use crate::kind::Kind;
use crate::reduce::state::Wire;
use crate::reduce::threads::Run;
use crate::reduce::turns::{CallId, ToolState};
use crate::wire::frame::Kept;
use crate::wire::request::{self, Request};

/// Asks the queue holds; one more is answered `cancelled` at once.
pub const MAX_ASKS: usize = 64;

/// One permission ask. The server sends it once and never again, so it is kept here until
/// the server says how it went.
#[derive(Debug, Clone, PartialEq)]
pub struct Ask {
    pub id: ApprovalId,
    /// The request's id exactly as the server wrote it, for the answer.
    pub(crate) wire_id: Value,
    pub thread: ThreadId,
    /// When it arrived: what its waiting time counts from.
    pub at: Millis,
    pub title: String,
    pub description: Option<String>,
    pub options: Vec<AcpPermissionOption>,
    /// What it is about, when the server says: an edit's diff.
    pub subject: Option<AcpPermissionSubject>,
    /// The tool call it gates, once the server's event names it.
    pub tool: Option<CallId>,
    /// The option the user picked, kept until the server confirms it.
    pub answer: Option<String>,
}

/// What answering with an option does. The set is closed here; a kind a newer server names
/// is kept as it was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
    Other(String),
}

/// An option's id, as the server wrote it: what an answer names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OptionId<'a>(pub &'a str);

/// One answer an ask offers, under the server's own name for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer<'a> {
    pub id: OptionId<'a>,
    pub name: &'a str,
    pub kind: OptionKind,
}

/// One file an edit's ask would change: its path as the diff names it, and its line counts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Change {
    pub path: String,
    pub added: u64,
    pub removed: u64,
}

/// The files a unified diff touches, by its `+++` headers, with the lines each adds and drops.
fn changes(diff: &str) -> Vec<Change> {
    let mut out: Vec<Change> = Vec::new();
    for line in diff.lines().filter(|line| !line.starts_with("--- ")) {
        if let Some(path) = line.strip_prefix("+++ ") {
            let path = path.strip_prefix("b/").unwrap_or(path);
            out.push(Change {
                path: path.to_owned(),
                ..Change::default()
            });
        } else if let Some(file) = out.last_mut() {
            match line.chars().next() {
                Some('+') => file.added = file.added.saturating_add(1),
                Some('-') => file.removed = file.removed.saturating_add(1),
                _ => {}
            }
        }
    }
    out
}

/// What the user chose for an ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Allow this once.
    Allow,
    /// Allow it and what is like it from now on.
    Always,
    Deny,
}

impl Ask {
    /// `yi://approval/<id>`: what an answer is aimed at.
    pub fn address(&self) -> Result<Address, AddressError> {
        Address::new(Kind::Approval, self.id.as_str())
    }

    /// The files the ask's diff would change: from its patch, else from its description.
    pub fn changes(&self) -> Vec<Change> {
        let content = match &self.subject {
            Some(AcpPermissionSubject::ToolCall { tool_call }) => tool_call.content.as_deref(),
            Some(AcpPermissionSubject::Other(_)) | None => None,
        };
        let patch = content
            .unwrap_or_default()
            .iter()
            .find_map(|part| match part {
                AcpToolContent::Diff { patch, .. } => patch.as_ref().map(|p| p.text.as_str()),
                _ => None,
            });
        changes(patch.or(self.description.as_deref()).unwrap_or_default())
    }

    /// Every answer this ask offers, in the server's order.
    pub fn offers(&self) -> impl Iterator<Item = Offer<'_>> {
        self.options.iter().map(|option| Offer {
            id: OptionId(&option.option_id),
            name: &option.name,
            kind: match &option.kind {
                AcpPermissionOptionKind::AllowOnce => OptionKind::AllowOnce,
                AcpPermissionOptionKind::AllowAlways => OptionKind::AllowAlways,
                AcpPermissionOptionKind::RejectOnce => OptionKind::RejectOnce,
                AcpPermissionOptionKind::RejectAlways => OptionKind::RejectAlways,
                AcpPermissionOptionKind::Other(kind) => OptionKind::Other(kind.clone()),
            },
        })
    }

    /// The option that says `choice`. Always falls back to once: an ask may offer no rule.
    fn option(&self, choice: Choice) -> Option<&str> {
        let of = |kind: AcpPermissionOptionKind| {
            let found = self.options.iter().find(|option| option.kind == kind);
            found.map(|option| option.option_id.as_str())
        };
        match choice {
            Choice::Allow => of(AcpPermissionOptionKind::AllowOnce),
            Choice::Always => self
                .always()
                .or_else(|| of(AcpPermissionOptionKind::AllowOnce)),
            Choice::Deny => of(AcpPermissionOptionKind::RejectOnce),
        }
    }

    /// What cmd+Return allows: Yi's narrowest standing yes. Invariant: Yi lists them narrowest
    /// first and never puts the whole repository first, so this never grants it.
    pub fn always(&self) -> Option<&str> {
        let mut always = self.options.iter();
        let first = always.find(|option| option.kind == AcpPermissionOptionKind::AllowAlways);
        first.map(|option| option.option_id.as_str())
    }

    fn reply(&self) -> Request {
        Request::answer(&self.wire_id, request::outcome(self.answer.as_deref()))
    }
}

impl Wire {
    /// Queues an ask from the server. One this app cannot read or has no room for is
    /// answered `cancelled` at once: an ask nobody can answer blocks its run for good.
    pub(crate) fn ask(
        &mut self,
        wire_id: Value,
        params: Kept<AcpPermissionParams>,
        at: Millis,
        out: &mut Vec<Request>,
    ) {
        let id = ApprovalId::new(match &wire_id {
            Value::String(id) => id.clone(),
            other => other.to_string(),
        });
        if self.asks.iter().any(|ask| ask.id == id) {
            return;
        }
        let params = match params {
            Kept::Known(params) if self.asks.len() < MAX_ASKS => params,
            Kept::Known(_) | Kept::Raw(_) => {
                return out.push(Request::answer(&wire_id, request::outcome(None)));
            }
        };
        let thread = ThreadId::new(params.session_id);
        let found = self.found(&thread);
        self.asks.push(Ask {
            id,
            wire_id,
            tool: self.gated(&thread).or(found),
            thread,
            at,
            title: params.title,
            description: params.description,
            options: params.options,
            subject: params.subject,
            answer: None,
        });
    }

    /// The call an ask gates when its event came first: the thread's or a child's, longest
    /// at its gate with no ask naming it. Incident: an ask that came second named no call.
    fn gated(&self, thread: &ThreadId) -> Option<CallId> {
        let thread = self.threads.get(thread)?;
        let named = |call: &CallId| self.asks.iter().any(|ask| ask.tool.as_ref() == Some(call));
        let children = thread.agents.iter().map(|agent| &agent.transcript);
        let calls = std::iter::once(&thread.transcript)
            .chain(children)
            .flat_map(|transcript| transcript.tools.values());
        let waits = calls.filter_map(|call| {
            let gate = call.gate.filter(|gate| gate.answered.is_none())?;
            (!named(&call.id)).then_some((gate.asked, &call.id))
        });
        waits
            .min_by_key(|(asked, _)| *asked)
            .map(|(_, id)| id.clone())
    }

    /// Incident: Yi does not ask a new connection again, so a replay that leaves the thread
    /// waiting with no ask held here leaves its unfinished steps waiting on nobody: they are lost.
    pub(crate) fn lose(&mut self, id: &ThreadId) {
        if self.asks.iter().any(|ask| &ask.thread == id) {
            return;
        }
        let Some(thread) = self.threads.get_mut(id) else {
            return;
        };
        if thread.state != Run::NeedsYou {
            return;
        }
        let open = |call: &&mut crate::reduce::turns::ToolCall| {
            matches!(
                call.state,
                ToolState::Queued | ToolState::Running | ToolState::Asking
            )
        };
        for call in thread.transcript.tools.values_mut().filter(open) {
            call.state = ToolState::Lost;
        }
    }

    /// A kept ask read back after a restart names no call: the replay's oldest open step of the
    /// tool its title names is it.
    pub(crate) fn relink(&mut self, id: &ThreadId) {
        let unnamed = |ask: &Ask| &ask.thread == id && ask.tool.is_none();
        let (Some(at), Some(thread)) =
            (self.asks.iter().position(unnamed), self.threads.get_mut(id))
        else {
            return;
        };
        let title = self
            .asks
            .get(at)
            .map(|ask| ask.title.clone())
            .unwrap_or_default();
        let open = thread.transcript.tools.values_mut();
        let open = open.filter(|call| matches!(call.state, ToolState::Queued | ToolState::Running));
        // Yi titles an ask by its tool ("edit requires permission"), not a todo queued beside it.
        let Some(call) = open.min_by_key(|call| (!title.starts_with(&call.name), call.started))
        else {
            return;
        };
        call.state = ToolState::Asking;
        let named = Some(call.id.clone());
        if let Some(ask) = self.asks.get_mut(at) {
            ask.tool = named;
        }
    }

    /// The server asked after all: the thread's lost steps wait on that ask again, and the first
    /// is the one it gates when no event has named another.
    fn found(&mut self, id: &ThreadId) -> Option<CallId> {
        let thread = self.threads.get_mut(id)?;
        let lost = thread.transcript.tools.values_mut();
        let lost = lost.filter(|call| call.state == ToolState::Lost);
        let mut first = None;
        for call in lost {
            call.state = ToolState::Asking;
            first.get_or_insert_with(|| call.id.clone());
        }
        first
    }

    /// Answers an ask with the user's choice. The ask stays, marked, until the server
    /// confirms: an answer lost with the connection is sent again when it is back.
    pub(crate) fn answer(&mut self, id: &ApprovalId, choice: Choice, out: &mut Vec<Request>) {
        let Some(ask) = self.asks.iter_mut().find(|ask| &ask.id == id) else {
            return;
        };
        match (ask.option(choice).map(str::to_owned), choice) {
            (Some(option), _) => {
                ask.answer = Some(option);
                out.push(ask.reply());
            }
            // An ask that offers no way to say no is refused by cancelling it.
            (None, Choice::Deny) => self.cancel(|ask| &ask.id == id, out),
            (None, Choice::Allow | Choice::Always) => {}
        }
    }

    /// Answers an ask with one of the options it offers, named by id. An id the ask does not
    /// offer changes nothing: the server would refuse it and the ask would wait for good.
    pub(crate) fn answer_with(&mut self, id: &ApprovalId, option: &str, out: &mut Vec<Request>) {
        let Some(ask) = self.asks.iter_mut().find(|ask| &ask.id == id) else {
            return;
        };
        if ask.offers().any(|offer| offer.id.0 == option) {
            ask.answer = Some(option.to_owned());
            out.push(ask.reply());
        }
    }

    /// Answers `cancelled` for every ask `gone` picks, and forgets them.
    pub(crate) fn cancel(&mut self, gone: impl Fn(&Ask) -> bool, out: &mut Vec<Request>) {
        self.asks.retain(|ask| {
            let cancel = gone(ask);
            if cancel {
                out.push(Request::answer(&ask.wire_id, request::outcome(None)));
            }
            !cancel
        });
    }

    /// Every ask as the request the server sent, one a line: what a restart reads back.
    /// Incident: a killed app lost its asks, and Yi answers only the id it asked under.
    pub fn kept(&self) -> String {
        let frame = |ask: &Ask| {
            let params = AcpPermissionParams {
                session_id: ask.thread.as_str().to_owned(),
                title: ask.title.clone(),
                description: ask.description.clone(),
                options: ask.options.clone(),
                subject: ask.subject.clone(),
            };
            let frame = serde_json::json!({"jsonrpc": "2.0", "id": ask.wire_id,
                "method": request::PERMISSION, "params": params});
            format!("{frame}\n")
        };
        self.asks.iter().map(frame).collect()
    }

    /// The asks the user has yet to answer, oldest first.
    pub fn waiting(&self) -> impl Iterator<Item = &Ask> {
        self.asks.iter().filter(|ask| ask.answer.is_none())
    }

    /// The answers the user chose and the server has not confirmed, to send again.
    pub(crate) fn answers(&self) -> impl Iterator<Item = Request> {
        let chosen = self.asks.iter().filter(|ask| ask.answer.is_some());
        chosen.map(Ask::reply)
    }

    /// Reads the two events that say which tool call an ask gates and how it was answered.
    pub(crate) fn permission(&mut self, thread: &ThreadId, event: &AgentEvent) {
        match event {
            AgentEvent::PermissionRequested {
                tool_call_id,
                title,
                ..
            } => {
                let mut open = self.asks.iter_mut().filter(|ask| &ask.thread == thread);
                let ask = open.find(|ask| ask.tool.is_none() && &ask.title == title);
                if let Some(ask) = ask {
                    ask.tool = Some(CallId::new(tool_call_id));
                }
            }
            AgentEvent::PermissionResolved { tool_call_id, .. } => {
                let before = self.asks.len();
                self.answered(thread, &[CallId::new(tool_call_id)]);
                // An ask that came after its event names no call: the one answered is it.
                let unnamed =
                    |ask: &Ask| &ask.thread == thread && ask.tool.is_none() && ask.answer.is_some();
                if let (true, Some(at)) = (
                    self.asks.len() == before,
                    self.asks.iter().position(unnamed),
                ) {
                    self.asks.remove(at);
                }
            }
            _ => {}
        }
    }

    /// Forgets the asks of `thread` that gate these tool calls: the server has its answer.
    pub(crate) fn answered(&mut self, thread: &ThreadId, tools: &[CallId]) {
        self.asks.retain(|ask| {
            let named = ask.tool.as_ref().is_some_and(|tool| tools.contains(tool));
            &ask.thread != thread || !named
        });
    }

    /// What a recording's client answered an ask with, taken as this app's own answer.
    pub(crate) fn chose(&mut self, wire_id: &Value, option: Option<&str>) {
        match option {
            Some(option) => {
                let ask = self.asks.iter_mut().find(|ask| &ask.wire_id == wire_id);
                if let Some(ask) = ask {
                    ask.answer = Some(option.to_owned());
                }
            }
            None => self.asks.retain(|ask| &ask.wire_id != wire_id),
        }
    }
}
