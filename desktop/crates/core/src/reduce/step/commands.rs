//! The commands that become requests to Yi's server, and what each is aimed at.

use serde_json::{Value, json};

use crate::address::attach;
use crate::address::thing::Thing;
use crate::address::uri::{Address, AddressError};
use crate::command::table::{Command, Route, Run, Target};
use crate::ids::{AgentId, ApprovalId, CommandId, EntryId, Millis, ThreadId};
use crate::kind::Kind;
use crate::reduce::approvals::{Ask, Choice};
use crate::reduce::state::{Asked, Later, Link, Wire};
use crate::reduce::threads::Thread;
use crate::wire::replay::Sync;
use crate::wire::request::{self, Request, RequestId};

/// A new thread in the folder a `place` address names; the text, if any, is its first prompt.
pub const NEW: CommandId = CommandId("thread.new");
/// A view starts showing a thread: it is resumed once, however many views show it.
pub const OPEN: CommandId = CommandId("thread.open");
/// A view stops showing a thread; when the last one does, its asks are answered `cancelled`.
pub const CLOSE: CommandId = CommandId("thread.close");
/// The text and the attached things, sent to a thread.
pub const PROMPT: CommandId = CommandId("thread.prompt");
/// Stops the turn; prompts typed during it wait until [`SEND_QUEUED`].
pub const CANCEL: CommandId = CommandId("thread.cancel");
/// Sends the prompts a Stop held, one at a time as before.
pub const SEND_QUEUED: CommandId = CommandId("thread.send-queued");
/// The text, put to the agent while it runs.
pub const STEER: CommandId = CommandId("thread.steer");
/// The text is a slash command's line, such as `undo`.
pub const SLASH: CommandId = CommandId("thread.slash");
/// The text is `<option>=<value>`: `model`, `thought_level` or `mode`.
pub const SET_OPTION: CommandId = CommandId("thread.set-option");
/// Pins a thread, or takes its pin off.
pub const PIN: CommandId = CommandId("thread.pin");
pub const SEEN: CommandId = CommandId("thread.seen");
/// A new thread from a message: leaving it out, or keeping it when the text is `at`.
pub const FORK: CommandId = CommandId("thread.fork");
/// Back to before a message; the text `files` puts the files back too.
pub const REWIND: CommandId = CommandId("thread.rewind");
pub const ALLOW: CommandId = CommandId("approval.allow");
pub const ALWAYS: CommandId = CommandId("approval.always");
pub const DENY: CommandId = CommandId("approval.deny");
/// Answers an ask with the option whose id is the text: any option the server offered.
pub const ANSWER: CommandId = CommandId("approval.answer");
/// Answers every waiting ask `cancelled`: what the app does as it quits.
pub const CANCEL_ASKS: CommandId = CommandId("approval.cancel-all");
pub const SETTINGS_READ: CommandId = CommandId("settings.read");
/// The setting a `setting` address names takes the text as its value, read as JSON.
pub const SETTINGS_WRITE: CommandId = CommandId("settings.write");
/// Asks for a child agent's whole conversation, aimed at [`child_address`].
pub const CHILD_REPLAY: CommandId = CommandId("agent.replay");
/// The text answers the question the child waits on.
pub const CHILD_ANSWER: CommandId = CommandId("agent.answer");
pub const CHILD_STOP: CommandId = CommandId("agent.stop");
/// Asks for the thread list now: with the link down, a request the client cannot write makes it
/// try the server at once instead of waiting out its backoff.
pub const RECONNECT: CommandId = CommandId("server.reconnect");

const fn row(
    id: CommandId,
    title: &'static str,
    key: Option<&'static str>,
    kinds: &'static [Kind],
) -> Command {
    Command {
        id,
        title,
        key,
        menu: Some("Thread"),
        kinds,
        route: Route::Wire,
    }
}

/// The wire commands a person picks from a menu, a button or a key. The rest are run by
/// views as they open, close and send.
pub const COMMANDS: &[Command] = &[
    row(NEW, "New Chat", Some("cmd+n"), &[Kind::Place]),
    row(OPEN, "Open Thread", None, &[Kind::Thread]),
    row(CANCEL, "Stop", None, &[Kind::Thread]),
    row(PIN, "Pin Thread", None, &[Kind::Thread]),
    row(SEEN, "Mark as Seen", None, &[Kind::Thread]),
    row(FORK, "Fork from Here", None, &[Kind::Message]),
    row(REWIND, "Rewind to Here", None, &[Kind::Message]),
    row(ALLOW, "Allow Once", Some("return"), &[Kind::Approval]),
    row(
        ALWAYS,
        "Always Allow the Narrowest Scope Yi Offers",
        Some("cmd+return"),
        &[Kind::Approval],
    ),
    row(DENY, "Deny", Some("escape"), &[Kind::Approval]),
    Command {
        id: RECONNECT,
        title: "Reconnect",
        key: None,
        menu: None,
        kinds: &[],
        route: Route::Wire,
    },
];

/// The blocks of a prompt: the words, then what each attached thing adds.
fn blocks(text: &str, attached: &[Thing]) -> Vec<Value> {
    let mut blocks = vec![json!({"type": "text", "text": text})];
    blocks.extend(attach::blocks(attached));
    blocks
}

/// The thing a command is aimed at, when it is of `kind`: its id.
fn aimed(target: &Target, kind: Kind) -> Option<&str> {
    match target {
        Target::Thing(address) if address.kind() == kind => Some(address.id()),
        Target::Thing(_) | Target::App | Target::Pane(_) => None,
    }
}

/// The address of a stored message, `yi://message/<thread>/<entry>`: what fork and rewind
/// are aimed at.
pub fn message_address(thread: &ThreadId, entry: &EntryId) -> Result<Address, AddressError> {
    Address::new(Kind::Message, format!("{thread}/{entry}"))
}

/// The address of one child agent of a thread, `yi://agent/<thread>/<agent>`: what the
/// child commands are aimed at.
pub fn child_address(thread: &ThreadId, child: &AgentId) -> Result<Address, AddressError> {
    Address::new(Kind::Agent, format!("{thread}/{child}"))
}

/// The thread and the child a [`child_address`] names.
pub fn child_of(target: &Target) -> Option<(ThreadId, AgentId)> {
    let (thread, child) = aimed(target, Kind::Agent)?.split_once('/')?;
    Some((ThreadId::new(thread), AgentId::new(child)))
}

/// The message a command is aimed at: the thread and the entry of its address.
fn message(target: &Target) -> Option<(ThreadId, EntryId)> {
    let (thread, entry) = aimed(target, Kind::Message)?.split_once('/')?;
    Some((ThreadId::new(thread), EntryId::new(entry)))
}

impl Wire {
    /// Carries out one wire command: the requests it makes go to `out`.
    pub(crate) fn run(
        &mut self,
        run: &Run,
        attached: &[Thing],
        at: Millis,
        out: &mut Vec<Request>,
    ) {
        let text = run.text.as_deref();
        match run.command {
            NEW => {
                if let Some(cwd) = aimed(&run.target, Kind::Place) {
                    let id = self.call(request::NEW, request::new_thread(cwd), at, out);
                    let first = text.map(|text| blocks(text, attached)).unwrap_or_default();
                    self.first_prompt(&id, first);
                }
            }
            RECONNECT => {
                self.call(request::LIST, json!({}), at, out);
            }
            SETTINGS_READ => {
                self.call(request::SETTINGS, json!({}), at, out);
            }
            SETTINGS_WRITE => {
                if let (Some(setting), Some(text)) = (aimed(&run.target, Kind::Setting), text) {
                    // A value that is not JSON is the text itself: `high`, not `"high"`.
                    let value = serde_json::from_str(text).unwrap_or_else(|_| Value::from(text));
                    let params = request::set_default(setting, value);
                    self.call(request::SET_DEFAULT, params, at, out);
                }
            }
            ALLOW => self.choose(&run.target, Choice::Allow, out),
            ALWAYS => self.choose(&run.target, Choice::Always, out),
            DENY => self.choose(&run.target, Choice::Deny, out),
            ANSWER => {
                let ask = aimed(&run.target, Kind::Approval).map(ApprovalId::new);
                if let (Some(ask), Some(option)) = (ask, text) {
                    self.answer_with(&ask, option, out);
                }
            }
            // With the link down a cancelled answer reaches nobody and the ask would be lost.
            CANCEL_ASKS if matches!(self.link, Link::Up { .. }) => self.cancel(|_| true, out),
            CANCEL_ASKS => {}
            FORK => {
                if let Some((thread, entry)) = message(&run.target) {
                    let params = request::fork(&thread, &entry, text == Some("at"));
                    self.call(request::FORK, params, at, out);
                }
            }
            REWIND => {
                if let Some((thread, entry)) = message(&run.target) {
                    let params = request::rewind(&thread, &entry, text == Some("files"));
                    self.call(request::REWIND, params, at, out);
                }
            }
            CHILD_REPLAY | CHILD_STOP => {
                if let Some((thread, child)) = child_of(&run.target) {
                    let method = match run.command {
                        CHILD_STOP => request::CHILD_ABORT,
                        _ => request::CHILD_REPLAY,
                    };
                    self.call(method, request::of_child(&thread, &child), at, out);
                }
            }
            CHILD_ANSWER => self.answer_child(&run.target, text, at, out),
            _ => {
                if let Some(thread) = aimed(&run.target, Kind::Thread).map(ThreadId::new) {
                    self.aim(run.command, &thread, text, attached, at, out);
                }
            }
        }
    }

    /// The commands aimed at one thread.
    fn aim(
        &mut self,
        command: CommandId,
        thread: &ThreadId,
        text: Option<&str>,
        attached: &[Thing],
        at: Millis,
        out: &mut Vec<Request>,
    ) {
        let (method, params) = match (command, text) {
            (OPEN, _) => return self.open(thread, at, out),
            (CLOSE, _) => return self.close(thread, out),
            (PROMPT, Some(text)) => {
                let blocks = blocks(text, attached);
                if self.busy(thread) {
                    let (thread, text) = (thread.clone(), text.to_owned());
                    return self.later.push(Later {
                        thread,
                        text,
                        blocks,
                    });
                }
                (request::PROMPT, request::prompt(thread, &blocks))
            }
            (CANCEL, _) => {
                if self.later.iter().any(|later| &later.thread == thread) {
                    self.stopped.insert(thread.clone());
                }
                self.call(request::CANCEL, request::of_thread(thread), at, out);
                // ACP: the client answers the stopped turn's asks `cancelled`.
                let open = |ask: &Ask| &ask.thread == thread && ask.answer.is_none();
                return self.cancel(open, out);
            }
            (SEND_QUEUED, _) => {
                self.stopped.remove(thread);
                return self.release(at, out);
            }
            (SEEN, _) => (request::SEEN, request::of_thread(thread)),
            (STEER, Some(text)) => (request::STEER, request::steer(thread, text)),
            (SLASH, Some(line)) => (request::SLASH, request::slash(thread, line)),
            (SET_OPTION, Some(text)) => match text.split_once('=') {
                Some((option, value)) => {
                    let params = request::set_option(thread, option, value);
                    (request::SET_OPTION, params)
                }
                None => return,
            },
            (PIN, _) => {
                let pinned = self.threads.get(thread).and_then(|thread| thread.pinned);
                (request::PIN, request::pin(thread, pinned.is_none()))
            }
            // A command this table lacks, or one that needs words and has none.
            _ => return,
        };
        self.call(method, params, at, out);
    }

    /// A view starts showing a thread. Its branch is asked for only if this app does not
    /// follow it already, so two views of one thread make one resume.
    fn open(&mut self, id: &ThreadId, at: Millis, out: &mut Vec<Request>) {
        let up = matches!(self.link, Link::Up { .. });
        let thread = self
            .threads
            .entry(id.clone())
            .or_insert_with(|| Thread::new(id.clone()));
        thread.views = thread.views.saturating_add(1);
        let apart = matches!(thread.sync, Sync::Apart | Sync::Broken);
        let unseen = thread.unseen() > 0;
        // While the link is down nothing can be asked; coming up resumes what views show.
        if apart && up {
            self.resume(id, at, out);
        }
        if unseen && up {
            self.call(request::SEEN, request::of_thread(id), at, out);
        }
    }

    /// A view stops showing a thread. Once none shows it, its asks are answered `cancelled`;
    /// with the link down they stay, since an answer that cannot be sent would lose them.
    fn close(&mut self, id: &ThreadId, out: &mut Vec<Request>) {
        let up = matches!(self.link, Link::Up { .. });
        let Some(thread) = self.threads.get_mut(id) else {
            return;
        };
        thread.views = thread.views.saturating_sub(1);
        if thread.views == 0 && up {
            self.cancel(|ask| &ask.thread == id, out);
        }
    }

    /// Answers the ask a command is aimed at: one by its address, or the oldest still
    /// waiting, of a thread or of all.
    fn choose(&mut self, target: &Target, choice: Choice, out: &mut Vec<Request>) {
        let ask = match target {
            Target::Thing(address) if address.kind() == Kind::Approval => {
                self.asks.iter().find(|ask| ask.id.as_str() == address.id())
            }
            Target::Thing(address) if address.kind() == Kind::Thread => self
                .waiting()
                .find(|ask| ask.thread.as_str() == address.id()),
            Target::Thing(_) => None,
            Target::App | Target::Pane(_) => self.waiting().next(),
        };
        if let Some(id) = ask.map(|ask| ask.id.clone()) {
            self.answer(&id, choice, out);
        }
    }

    /// Answers the question a child waits on, by the id the server noted it under.
    fn answer_child(
        &mut self,
        target: &Target,
        text: Option<&str>,
        at: Millis,
        out: &mut Vec<Request>,
    ) {
        let (Some((thread, child)), Some(text)) = (child_of(target), text) else {
            return;
        };
        let agents = self.threads.get(&thread).map(|t| t.agents.as_slice());
        let agent = agents.unwrap_or_default().iter().find(|a| a.id == child);
        let Some((question, _)) = agent.and_then(|agent| agent.question()) else {
            return;
        };
        let params = request::answer_child(&thread, &child, question, text);
        self.call(request::CHILD_ANSWER, params, at, out);
    }

    /// Remembers the prompt a new thread is to be given as soon as the server has made it.
    fn first_prompt(&mut self, id: &RequestId, first: Vec<Value>) {
        if let Some(Asked::New { first: slot, .. }) =
            self.pending.get_mut(id).map(|pending| &mut pending.asked)
        {
            *slot = first;
        }
    }
}
