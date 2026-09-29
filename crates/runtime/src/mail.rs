//! Family envelopes: ids and per-pair order, the durable inbox, and request waiters (D214).
use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use tokio::sync::oneshot;
use yi_types::mail::{Envelope, Kind, MailId};
use yi_types::message::{AgentMessage, UserContent};

use crate::subagent::{PARENT_NAME, SubagentHost};

pub(crate) const INBOX_ENTRY: &str = "agent_message";
const READ_ENTRY: &str = "agent_message_read";
pub(crate) const HUMAN_ANSWER: &str = "human_answer";
pub(crate) const HUMAN: &str = "human";
pub(crate) const FINAL_TEXT: &str = "final_text";
const CHASE: &str = "[host] request";
const CHASED: &str = "is still open and your turn ended without answering it";
pub(crate) const BODY_CAP: usize = crate::mailbox::CONTEXT_TOTAL_CAP;
const HEAD_BYTES: usize = 4096;
/// Requests one sender may have waiting at once; a flood refuses instead of growing the map.
const MAX_OUTSTANDING: usize = 16;

/// What a sender may say of its message; who, to whom and in what order [`Desk::seal`] fills.
#[derive(Default)]
pub(crate) struct Draft {
    pub(crate) text: String,
    pub(crate) followup: bool,
    pub(crate) kind: Kind,
    conversation: Option<MailId>,
    reply_to: Option<MailId>,
    deadline_ms: Option<u64>,
    reference: Option<yi_types::url::Url>,
    /// Minted by [`SubagentHost::request`] so its waiter stands before the envelope exists.
    id: Option<MailId>,
    pub(crate) answered_by: Option<&'static str>,
}

impl Draft {
    pub(crate) fn plain(text: &str, followup: bool) -> Self {
        Self {
            text: text.to_owned(),
            followup,
            ..Self::default()
        }
    }

    /// A message the host itself sends for a lease: a `cancel` down, a `failure` up.
    pub(crate) fn of(kind: Kind, text: &str) -> Self {
        Self {
            kind,
            ..Self::plain(text, false)
        }
    }

    pub(crate) fn from_payload(payload: &Map<String, Value>) -> Result<(String, Self), String> {
        let text_of = |key: &str| payload.get(key).and_then(Value::as_str);
        let target = text_of("target").unwrap_or_default().to_owned();
        let text = text_of("message").ok_or("agent_message.send requires a message")?;
        let reply_to = text_of("reply_to").map(|id| MailId(id.to_owned()));
        let kind = match payload.get("kind").filter(|kind| !kind.is_null()) {
            Some(kind) => serde_json::from_value(kind.clone()).map_err(|_| {
                format!(
                    "kind {kind} is not one of inform, request, reply, progress, failure, cancel"
                )
            })?,
            None if reply_to.is_some() => Kind::Reply,
            None => Kind::Inform,
        };
        if (kind == Kind::Reply) != reply_to.is_some() {
            return Err(
                "a reply names the request it answers: kind reply and reply_to=<id> go together"
                    .to_owned(),
            );
        }
        let reference = text_of("ref")
            .map(|raw| raw.parse().map_err(|error| format!("ref {raw}: {error}")))
            .transpose()?;
        let draft = Self {
            text: text.to_owned(),
            followup: payload.get("followup").and_then(Value::as_bool) == Some(true),
            kind,
            conversation: text_of("conversation").map(|id| MailId(id.to_owned())),
            reply_to,
            deadline_ms: payload.get("deadline_ms").and_then(Value::as_u64),
            reference,
            id: None,
            answered_by: None,
        };
        Ok((target, draft))
    }

    /// Refused whole at the host boundary: a trimmed body would arrive as if it were complete.
    pub(crate) fn admit(&self, from: &str) -> Result<(), String> {
        if self.text.len() > BODY_CAP {
            return Err(format!(
                "the message body is {} bytes and at most {BODY_CAP} travel in one message; nothing was sent. rlm.put(name, obj) the payload and send its family://<name> address as ref",
                self.text.len()
            ));
        }
        let allowed = match self.kind {
            Kind::Progress | Kind::Failure => from != PARENT_NAME,
            Kind::Cancel => from == PARENT_NAME,
            Kind::Inform | Kind::Request | Kind::Reply => true,
        };
        if allowed {
            Ok(())
        } else {
            Err(format!(
                "\"{from}\" cannot send kind {}: progress and failure go up from a child, cancel comes down from the parent",
                self.kind.as_str()
            ))
        }
    }
}

struct Waiter {
    sender: String,
    respondent: String,
    reply: oneshot::Sender<Result<Envelope, String>>,
    asked: String,
    opened: u64,
    shown: bool,
}

/// Mail state under one lock held across a routing, so `seq` order is queue order.
#[derive(Default)]
pub(crate) struct Desk {
    // ponytail: ids restart with the host, as its children do; seed from the inbox if a
    // restarted host ever adopts live children.
    minted: u64,
    seqs: HashMap<(String, String), u64>,
    waiters: HashMap<MailId, Waiter>,
    answered: HashMap<MailId, String>,
    nudged: std::collections::HashSet<MailId>,
}

impl Desk {
    fn mint(&mut self, from: &str) -> MailId {
        self.minted = self.minted.saturating_add(1);
        MailId(format!("{from}-{}", self.minted))
    }

    /// `between` is the sender's and the receiver's incarnation, read off the registry.
    pub(crate) fn seal(
        &mut self,
        (from, to): (&str, &str),
        between: (Option<u32>, Option<u32>),
        draft: &Draft,
    ) -> Envelope {
        let id = draft.id.clone().unwrap_or_else(|| self.mint(from));
        let seq = self
            .seqs
            .entry((from.to_owned(), to.to_owned()))
            .or_default();
        *seq = seq.saturating_add(1);
        Envelope {
            conversation: draft
                .conversation
                .clone()
                .or_else(|| draft.reply_to.clone())
                .unwrap_or_else(|| id.clone()),
            id,
            from: from.to_owned(),
            from_incarnation: between.0,
            to: to.to_owned(),
            to_incarnation: between.1,
            kind: draft.kind,
            in_reply_to: draft.reply_to.clone(),
            seq: *seq,
            sent_at: yi_session::now_ms(),
            deadline_ms: draft.deadline_ms,
            body: draft.text.clone(),
            reference: draft.reference.clone(),
            answered_by: draft.answered_by.map(str::to_owned),
        }
    }

    /// Invariant: only the named respondent's reply to the asking sender resolves a waiter.
    pub(crate) fn resolve(&mut self, envelope: &Envelope) -> bool {
        let Some(id) = envelope.in_reply_to.as_ref() else {
            return false;
        };
        let answers = self.waiters.get(id).is_some_and(|waiter| {
            envelope.kind == Kind::Reply
                && waiter.respondent == envelope.from
                && waiter.sender == envelope.to
        });
        if answers && let Some(waiter) = self.waiters.remove(id) {
            let who = match envelope.answered_by.as_deref() {
                Some(HUMAN) => "the human".to_owned(),
                Some(_) => format!("\"{}\"'s final text", envelope.from),
                None => format!("\"{}\"", envelope.from),
            };
            self.answered.insert(id.clone(), who);
            let _the_requester_may_have_timed_out = waiter.reply.send(Ok(envelope.clone()));
            return true;
        }
        false
    }

    /// Invariant: the first answer to a request wins; a later one is refused, naming who won.
    pub(crate) fn refuse_second_answer(&self, to: &str, draft: &Draft) -> Result<(), String> {
        let Some(id) = draft.reply_to.as_ref() else {
            return Ok(());
        };
        if let Some(who) = self.answered.get(id) {
            return Err(format!(
                "{id} was already answered by {who}; nothing was sent"
            ));
        }
        let open = self
            .waiters
            .get(id)
            .is_some_and(|waiter| waiter.sender == to && waiter.respondent == PARENT_NAME);
        if draft.answered_by == Some(HUMAN) && !open {
            return Err(format!(
                "{id} is no longer open: it timed out or \"{to}\" stopped asking; nothing was sent"
            ));
        }
        Ok(())
    }
}

impl Desk {
    fn only_request(&self, asker: &str, respondent: &str) -> Option<(MailId, bool)> {
        let mut open = self
            .waiters
            .iter()
            .filter(|(_, waiter)| waiter.sender == asker && waiter.respondent == respondent);
        let (id, waiter) = open.next()?;
        open.next().is_none().then(|| (id.clone(), waiter.shown))
    }

    pub(crate) fn asking(&self) -> HashMap<String, String> {
        self.oldest_asks()
            .into_values()
            .map(|(id, waiter)| {
                let note = format!("asks {id}: {}", waiter.asked);
                (waiter.sender.clone(), note)
            })
            .collect()
    }

    pub(crate) fn show<'a>(&mut self, notes: impl IntoIterator<Item = &'a str>) {
        for note in notes {
            let id = note
                .strip_prefix("asks ")
                .and_then(|rest| rest.split_once(':'));
            let open = id.and_then(|(id, _)| self.waiters.get_mut(&MailId(id.to_owned())));
            if let Some(waiter) = open {
                waiter.shown = true;
            }
        }
    }

    fn oldest_asks(&self) -> HashMap<&str, (&MailId, &Waiter)> {
        let mut oldest: HashMap<&str, (&MailId, &Waiter)> = HashMap::new();
        let asked = self
            .waiters
            .iter()
            .filter(|(_, waiter)| waiter.respondent == PARENT_NAME);
        for (id, waiter) in asked {
            let slot = oldest.entry(waiter.sender.as_str()).or_insert((id, waiter));
            if waiter.opened < slot.1.opened {
                *slot = (id, waiter);
            }
        }
        oldest
    }

    /// A terminated respondent answers nothing, so its waiters are refused at once with the
    /// termination named; a timeout would say the wrong thing a long time later.
    pub(crate) fn drop_respondent(&mut self, respondent: &str, how: &str) {
        let ids: Vec<MailId> = self
            .waiters
            .iter()
            .filter(|(_, waiter)| waiter.respondent == respondent)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            if let Some(waiter) = self.waiters.remove(&id) {
                let refusal = format!("\"{respondent}\" was {how} before it replied to {id}");
                let _the_requester_may_be_gone = waiter.reply.send(Err(refusal));
            }
        }
    }
}

/// Invariant: the inbox entry is written before any delivery, and a refused write refuses it.
pub(crate) fn inbox(store: &yi_session::SharedSession, envelope: &Envelope) -> Result<(), String> {
    let data = serde_json::to_value(envelope).map_err(|error| error.to_string())?;
    yi_session::lock_session(store)
        .append_custom("main", INBOX_ENTRY, Some(data))
        .map(drop)
        .map_err(|error| {
            format!(
                "the inbox of \"{}\" refused the envelope, so nothing was delivered: {error}",
                envelope.to
            )
        })
}

/// Invariant: agent words travel inside this element, never as bare user text.
pub(crate) fn present(envelope: &Envelope) -> AgentMessage {
    let Envelope { id, from, body, .. } = envelope;
    let mut tag = format!("<agent_message from=\"{from}\"");
    if let Some(incarnation) = envelope.from_incarnation {
        tag.push_str(&format!(" from_incarnation=\"{incarnation}\""));
    }
    // A kept transcript outlives the incarnation it was written to, which reads it again.
    if let Some(incarnation) = envelope.to_incarnation {
        tag.push_str(&format!(" to_incarnation=\"{incarnation}\""));
    }
    if envelope.kind != Kind::Inform {
        tag.push_str(&format!(" kind=\"{}\" id=\"{id}\"", envelope.kind.as_str()));
    }
    if let Some(reference) = &envelope.reference {
        tag.push_str(&format!(" ref=\"{reference}\""));
    }
    let mut text = format!("{tag}>\n{body}\n</agent_message>");
    if envelope.kind == Kind::Request {
        text.push_str(&format!(
            "\nAnswer with rlm.send(\"{from}\", text, reply_to=\"{id}\")."
        ));
    }
    AgentMessage::Custom {
        custom_type: INBOX_ENTRY.to_owned(),
        content: UserContent::Text(text),
        display: true,
        details: serde_json::to_value(envelope).ok(),
        timestamp: envelope.sent_at,
    }
}

pub(crate) fn envelope_id(message: &AgentMessage) -> Option<&str> {
    match message {
        AgentMessage::Custom {
            custom_type,
            details: Some(details),
            ..
        } if custom_type == INBOX_ENTRY => details.get("id").and_then(Value::as_str),
        _ => None,
    }
}

pub(crate) fn progress_from(message: &AgentMessage) -> Option<&str> {
    let details = match message {
        AgentMessage::Custom {
            custom_type,
            details: Some(details),
            ..
        } if custom_type == INBOX_ENTRY => details,
        _ => return None,
    };
    if details.get("kind")? != "progress" {
        return None;
    }
    details.get("from")?.as_str()
}

fn shown(entries: &[yi_types::entry::Entry]) -> std::collections::HashSet<&str> {
    use yi_types::entry::Entry;
    entries
        .iter()
        .flat_map(|entry| match entry {
            Entry::Message { message, .. } => envelope_id(message).into_iter().collect(),
            Entry::Custom {
                custom_type,
                data: Some(Value::Array(ids)),
                ..
            } if custom_type == READ_ENTRY => ids.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        })
        .collect()
}

fn presented(store: &yi_session::SharedSession, id: &MailId) -> bool {
    let query = yi_session::EntryQuery::default();
    let entries = yi_session::lock_session(store).find_entries_on_branch(
        "main",
        &query,
        &yi_session::BranchBounds::default(),
    );
    entries.is_ok_and(|entries| shown(&entries).contains(id.0.as_str()))
}

/// Invariant: an inbox entry with no presented message entry is queued again; `progress` never.
pub(crate) fn unread(entries: &[yi_types::entry::Entry]) -> Vec<AgentMessage> {
    use yi_types::entry::Entry;
    let shown = shown(entries);
    entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Custom {
                custom_type, data, ..
            } if custom_type == INBOX_ENTRY => {
                serde_json::from_value::<Envelope>(data.clone()?).ok()
            }
            _ => None,
        })
        .filter(|envelope| {
            envelope.kind != Kind::Progress && !shown.contains(envelope.id.0.as_str())
        })
        .map(|envelope| present(&envelope))
        .collect()
}

pub(crate) fn received(
    store: Option<yi_session::SharedSession>,
    taken: Vec<AgentMessage>,
) -> Vec<Value> {
    let envelopes: Vec<Value> = taken
        .into_iter()
        .filter_map(|message| match message {
            AgentMessage::Custom { details, .. } => details,
            _ => None,
        })
        .collect();
    let ids: Vec<Value> = envelopes
        .iter()
        .filter_map(|envelope| envelope.get("id").cloned())
        .collect();
    if let Some(store) = store.filter(|_| !ids.is_empty()) {
        mark_read(&store, ids);
    }
    envelopes
}

pub(crate) fn mark_read(store: &yi_session::SharedSession, ids: Vec<Value>) {
    let _refused_read_mark_rereads_after_a_crash =
        yi_session::lock_session(store).append_custom("main", READ_ENTRY, Some(Value::Array(ids)));
}

pub(crate) fn is_chase(message: &AgentMessage) -> bool {
    let text = message.plain_text();
    text.starts_with(CHASE) && text.contains(CHASED)
}

fn pickled(text: &str) -> Vec<u8> {
    let length = u32::try_from(text.len()).unwrap_or(u32::MAX).to_le_bytes();
    [&[0x80, 2, b'X'][..], &length, text.as_bytes(), b"."].concat()
}

/// Retires a waiter on every way out: a reply, a timeout, a refused send or a cancelled cell.
struct Parked<'a>(&'a SubagentHost, MailId, Option<String>);

impl Drop for Parked<'_> {
    fn drop(&mut self) {
        if let Ok(mut desk) = self.0.mail.lock() {
            desk.waiters.remove(&self.1);
            desk.nudged.remove(&self.1);
        }
        if let Some(asker) = &self.2 {
            self.0.publish_member(asker);
        }
    }
}

impl SubagentHost {
    /// Sends a request and waits for its reply. The waiter is installed before the envelope
    /// can reach the respondent, so no reply is early enough to miss it.
    pub async fn request(
        &self,
        from: &str,
        target: &str,
        text: &str,
        timeout_ms: u64,
    ) -> Result<Map<String, Value>, String> {
        if target == "all" {
            return Err("a request has one respondent; send to \"all\" instead".to_owned());
        }
        let timeout_ms = timeout_ms.min(crate::mailbox::WAIT_MAX_MS);
        let respondent = self.member_name(target);
        let (id, reply) = {
            let mut desk = self.mail.lock().map_err(|_| "mail state poisoned")?;
            let waiting = desk.waiters.values().filter(|waiter| waiter.sender == from);
            if waiting.count() >= MAX_OUTSTANDING {
                return Err(format!(
                    "\"{from}\" already has {MAX_OUTSTANDING} requests waiting on a reply; let one resolve or time out first"
                ));
            }
            let id = desk.mint(from);
            let (sender, reply) = oneshot::channel();
            let waiter = Waiter {
                sender: from.to_owned(),
                respondent: respondent.clone(),
                reply: sender,
                asked: text
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .chars()
                    .take(120)
                    .collect(),
                opened: desk.minted,
                shown: false,
            };
            desk.waiters.insert(id.clone(), waiter);
            (id, reply)
        };
        let asks_parent = (respondent == PARENT_NAME).then(|| from.to_owned());
        let _parked = Parked(self, id.clone(), asks_parent.clone());
        let draft = Draft {
            kind: Kind::Request,
            deadline_ms: Some(yi_session::now_ms().saturating_add(timeout_ms)),
            id: Some(id.clone()),
            ..Draft::plain(text, false)
        };
        let mut sent = self.route_mail(from, target, &draft)?;
        if from == PARENT_NAME {
            self.waited_on(false);
        }
        if let Some(asker) = &asks_parent {
            self.publish_member(asker);
        }
        let wait = std::time::Duration::from_millis(timeout_ms);
        let answer = match tokio::time::timeout(wait, reply).await {
            Ok(Ok(Ok(answer))) => Some(answer),
            Ok(Ok(Err(terminated))) => return Err(terminated),
            _ => None,
        };
        let Some(answer) = answer else {
            return Err(format!(
                "no reply to {id} from \"{respondent}\" within {timeout_ms} ms; the request stays in its inbox, and a late reply lands in history without resolving this call"
            ));
        };
        sent.insert("reply".to_owned(), Value::String(answer.body.clone()));
        sent.insert(
            "envelope".to_owned(),
            serde_json::to_value(&answer).unwrap_or_default(),
        );
        Ok(sent)
    }
}

impl SubagentHost {
    pub fn answer(
        &self,
        child: &str,
        question: &str,
        text: &str,
    ) -> Result<Map<String, Value>, String> {
        let name = self.member_name(child);
        let draft = Draft {
            kind: Kind::Reply,
            reply_to: Some(MailId(question.to_owned())),
            answered_by: Some(HUMAN),
            ..Draft::plain(text, false)
        };
        let mut sent = self.route_mail(PARENT_NAME, &name, &draft)?;
        let told = json!({"id": sent["receipts"][0]["id"], "from": PARENT_NAME, "to": name,
            "kind": "reply", "inReplyTo": question, "answeredBy": "human", "body": text});
        if let Some(store) = (self.options.store)() {
            let _the_told_message_still_reaches_the_model = yi_session::lock_session(&store)
                .append_custom("main", HUMAN_ANSWER, Some(told.clone()));
        }
        let words = format!("The human answered {name}'s question {question} for you: {text}");
        let message = AgentMessage::Custom {
            custom_type: HUMAN_ANSWER.to_owned(),
            content: UserContent::Text(words),
            display: true,
            details: Some(told),
            timestamp: yi_session::now_ms(),
        };
        (self.options.report)(message, false);
        sent.insert("answers".to_owned(), Value::String(question.to_owned()));
        Ok(sent)
    }

    pub fn answer_told(&self, child: &str, question: &str, text: &str) -> String {
        match self.answer(child, question, text) {
            Ok(sent) => {
                let target = sent["receipts"][0]["target"].as_str().unwrap_or(child);
                format!("you → {target} (answering {question}): {text}")
            }
            Err(refusal) => refusal,
        }
    }

    /// Invariant: an ending owing a request steers once, then sends its final text. True if woken.
    pub(crate) fn chase_open_requests(
        &self,
        name: &str,
        session: &crate::session::AgentSession,
        messages: &[AgentMessage],
    ) -> bool {
        let owed: Vec<(MailId, String, bool)> = match self.mail.lock() {
            Ok(mut desk) => {
                let open: Vec<(MailId, String)> = desk
                    .waiters
                    .iter()
                    .filter(|(_, waiter)| waiter.respondent == name)
                    .map(|(id, waiter)| (id.clone(), waiter.sender.clone()))
                    .collect();
                open.into_iter()
                    .map(|(id, asker)| {
                        let first = desk.nudged.insert(id.clone());
                        (id, asker, first)
                    })
                    .collect()
            }
            Err(_) => return false,
        };
        let mut steered = false;
        for (id, asker, first) in owed {
            if first {
                let steer = format!(
                    "{CHASE} {id} from \"{asker}\" {CHASED}. Answer it now with rlm.send(\"{asker}\", text, reply_to=\"{id}\"); if this turn also ends without that call, its final text is sent as your reply."
                );
                let woke = session.deliver(crate::session::user_message(&steer), true);
                steered |= woke == yi_types::mail::Delivery::Woken;
                continue;
            }
            let text = crate::subagent::last_assistant_text(messages)
                .unwrap_or_else(|| "(the turn ended with no text)".to_owned());
            let (text, reference) = self.kept_whole(name, &id, text);
            let draft = Draft {
                kind: Kind::Reply,
                reply_to: Some(id),
                answered_by: Some(FINAL_TEXT),
                reference,
                ..Draft::plain(&text, false)
            };
            let _a_requester_gone_since_is_nobody_to_tell = self.route_mail(name, &asker, &draft);
        }
        steered
    }

    /// Incident: a final text over the body cap was dropped; it is kept whole, its head sent.
    fn kept_whole(
        &self,
        owner: &str,
        id: &MailId,
        text: String,
    ) -> (String, Option<yi_types::url::Url>) {
        if text.len() <= BODY_CAP {
            return (text, None);
        }
        let name = format!("reply-{id}");
        let dir = self
            .family
            .get()
            .cloned()
            .unwrap_or_else(|| crate::wiring::family_dir_of(&self.options.parent_session_dir));
        #[expect(
            clippy::cast_precision_loss,
            reason = "rlm.put's `at` is float seconds"
        )]
        let at = yi_session::now_ms() as f64 / 1000.0;
        let sidecar = serde_json::json!({"name": name, "owner": owner, "at": at,
            "bytes": text.len(), "type": "str", "serializer": "pickle", "text": text});
        let kept = std::fs::create_dir_all(&dir)
            .and_then(|()| {
                crate::wiring::write_board(&dir.join(format!("{name}.dill")), &pickled(&text))
            })
            .and_then(|()| {
                crate::wiring::write_board(
                    &dir.join(format!("{name}.json")),
                    sidecar.to_string().as_bytes(),
                )
            });
        let cut = text
            .char_indices()
            .map(|(at, _)| at)
            .take_while(|at| *at <= HEAD_BYTES)
            .last()
            .unwrap_or(0);
        let head = text.get(..cut).unwrap_or_default();
        match kept {
            Ok(()) => (
                format!(
                    "{head}\n[... {} bytes in all: rlm.get({name:?}) or fetch family://{name}]",
                    text.len()
                ),
                format!("family://{name}").parse().ok(),
            ),
            Err(error) => (
                format!(
                    "{head}\n[... {} bytes in all; the rest could not be kept: {error}]",
                    text.len()
                ),
                None,
            ),
        }
    }

    /// A plain send answers a child's one open question once its sender was shown it.
    pub(crate) fn as_answer(
        &self,
        from: &str,
        target: &str,
        draft: &Draft,
    ) -> Result<Option<Draft>, String> {
        if draft.kind != Kind::Inform || matches!(target, "parent" | "all") {
            return Ok(None);
        }
        let asker = self.member_name(target);
        let open = self
            .mail
            .lock()
            .ok()
            .and_then(|desk| desk.only_request(&asker, from));
        let Some((id, shown)) = open else {
            return Ok(None);
        };
        let store = match from {
            PARENT_NAME => (self.options.store)(),
            sender => self.transcript(sender),
        };
        if !shown && !store.is_some_and(|store| presented(&store, &id)) {
            return Err(format!(
                "\"{asker}\" has request {id} open to you, and this send does not answer it: answer with rlm.send(\"{asker}\", text, reply_to=\"{id}\")"
            ));
        }
        Ok(Some(Draft {
            kind: Kind::Reply,
            reply_to: Some(id),
            reference: draft.reference.clone(),
            ..Draft::plain(&draft.text, draft.followup)
        }))
    }

    pub fn open_requests(&self) -> Vec<(String, String, String)> {
        let Ok(desk) = self.mail.lock() else {
            return Vec::new();
        };
        let mut open: Vec<(String, String, String)> = desk
            .waiters
            .iter()
            .map(|(id, waiter)| {
                (
                    id.0.clone(),
                    waiter.sender.clone(),
                    waiter.respondent.clone(),
                )
            })
            .collect();
        open.sort();
        open
    }

    pub(crate) fn cancelled_member(&self, name: &str) -> bool {
        self.children.lock().is_ok_and(|children| {
            Self::key_of(&children, name)
                .ok()
                .and_then(|key| children.get(&key))
                .is_some_and(|record| record.session.cancelled())
        })
    }

    pub(crate) fn publish_all(&self) {
        let keys: Vec<String> = self
            .children
            .lock()
            .map(|children| children.keys().cloned().collect())
            .unwrap_or_default();
        for key in keys {
            self.publish(&key);
        }
    }

    fn publish_member(&self, name: &str) {
        let key = self
            .children
            .lock()
            .ok()
            .and_then(|children| Self::key_of(&children, name).ok());
        if let Some(key) = key {
            self.publish(&key);
        }
    }
}

pub fn register_receive(
    session: &crate::session::AgentSession,
    host: &Arc<SubagentHost>,
    registry: &mut crate::kernel::HostRegistry,
) {
    let (take, host, mail) = (
        session.mail_hook(),
        Arc::clone(host),
        session.mail_arrived(),
    );
    registry.register("rlm.receive", move |payload| {
        let asked = crate::mailbox::timeout_of(&payload);
        let (take, host, mail) = (Arc::clone(&take), Arc::clone(&host), Arc::clone(&mail));
        Box::pin(async move {
            let asked = asked?;
            let clamped = asked.clamp(crate::mailbox::WAIT_MIN_MS, crate::mailbox::WAIT_MAX_MS);
            let started = std::time::Instant::now();
            let deadline = started
                .checked_add(std::time::Duration::from_millis(clamped))
                .unwrap_or(started);
            let _span = yi_types::trace::span("wait.mail_receive");
            let envelopes = crate::session::until(&mail, || {
                let envelopes = take();
                if envelopes.is_empty() && std::time::Instant::now() < deadline {
                    return std::ops::ControlFlow::Continue(Some(deadline));
                }
                std::ops::ControlFlow::Break(envelopes)
            })
            .await;
            host.waited_on(!envelopes.is_empty());
            let mut reply = Map::new();
            reply.insert("envelopes".to_owned(), Value::Array(envelopes));
            reply.insert("timeout_ms".to_owned(), Value::from(clamped));
            reply.insert("clamped".to_owned(), Value::Bool(clamped != asked));
            Ok(reply)
        })
    });
}

#[cfg(test)]
mod tests {
    use super::{Desk, Draft, Kind, MailId, PARENT_NAME, Waiter, oneshot};

    type Answer = oneshot::Receiver<Result<yi_types::mail::Envelope, String>>;

    fn ask(desk: &mut Desk) -> (MailId, Answer) {
        let id = desk.mint("writer");
        let (reply, answer) = oneshot::channel();
        let waiter = Waiter {
            sender: "writer".to_owned(),
            respondent: PARENT_NAME.to_owned(),
            reply,
            asked: format!("question {id}"),
            opened: desk.minted,
            shown: false,
        };
        desk.waiters.insert(id.clone(), waiter);
        (id, answer)
    }

    fn reply(id: &MailId, by_human: bool) -> Draft {
        Draft {
            kind: Kind::Reply,
            reply_to: Some(id.clone()),
            answered_by: by_human.then_some(super::HUMAN),
            ..Draft::plain("notes.md", false)
        }
    }

    fn answered(desk: &mut Desk, id: &MailId, by_human: bool) -> bool {
        let envelope = desk.seal((PARENT_NAME, "writer"), (None, None), &reply(id, by_human));
        desk.resolve(&envelope)
    }

    /// Dies with the card showing one of a child's questions and the reply box resolving another.
    #[test]
    fn the_card_shows_the_oldest_question_and_an_answer_resolves_the_one_it_names() {
        let mut desk = Desk::default();
        let asked: Vec<(MailId, Answer)> = (0..8).map(|_| ask(&mut desk)).collect();
        let note = desk.asking().remove("writer").unwrap_or_default();
        assert!(note.starts_with("asks writer-1: "), "{note}");
        assert!(answered(&mut desk, &asked[7].0, true));
        assert!(
            desk.waiters.contains_key(&asked[0].0),
            "writer-1 is still open"
        );
    }

    /// Dies with answers keyed by child: a late reply to an earlier question went out as mail.
    #[test]
    fn a_late_answer_is_refused_by_the_question_it_names() {
        let mut desk = Desk::default();
        let (first, _first) = ask(&mut desk);
        assert!(answered(&mut desk, &first, true));
        let (second, _second) = ask(&mut desk);
        assert!(answered(&mut desk, &second, false));
        let late = desk.refuse_second_answer("writer", &reply(&first, false));
        let expected = format!("{first} was already answered by the human; nothing was sent");
        assert_eq!(late, Err(expected));
        let (third, _third) = ask(&mut desk);
        desk.waiters.remove(&third);
        let closed = desk.refuse_second_answer("writer", &reply(&third, true));
        assert!(
            closed.is_err_and(|refusal| refusal.starts_with(&format!("{third} is no longer open"))),
            "the human's answer to a closed question is refused by its own id"
        );
    }
}
