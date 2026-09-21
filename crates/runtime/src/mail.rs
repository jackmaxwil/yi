//! Envelopes between family members: ids and per-pair order, the durable inbox, and the
//! waiters a request parks on (D214).
use std::collections::HashMap;

use serde_json::{Map, Value};
use tokio::sync::oneshot;
use yi_types::mail::{Envelope, Kind, MailId};
use yi_types::message::{AgentMessage, UserContent};

use crate::subagent::{PARENT_NAME, SubagentHost};

pub(crate) const INBOX_ENTRY: &str = "agent_message";
pub(crate) const BODY_CAP: usize = crate::mailbox::CONTEXT_TOTAL_CAP;
/// Requests one sender may have waiting at once; a flood refuses instead of growing the map.
const MAX_OUTSTANDING: usize = 16;

/// What a sender may say about its own message. Who sent it, to whom and in what order is
/// the host's to fill at [`Desk::seal`], never the payload's.
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
}

impl Draft {
    pub(crate) fn plain(text: &str, followup: bool) -> Self {
        Self {
            text: text.to_owned(),
            followup,
            ..Self::default()
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
    reply: oneshot::Sender<Envelope>,
}

/// The host's mail state under one lock, held across a whole routing so that the order `seq`
/// is handed out in is the order the queues are pushed in.
#[derive(Default)]
pub(crate) struct Desk {
    // ponytail: ids restart with the host, as its children do; seed from the inbox if a
    // restarted host ever adopts live children.
    minted: u64,
    seqs: HashMap<(String, String), u64>,
    waiters: HashMap<MailId, Waiter>,
}

impl Desk {
    fn mint(&mut self, from: &str) -> MailId {
        self.minted = self.minted.saturating_add(1);
        MailId(format!("{from}-{}", self.minted))
    }

    pub(crate) fn seal(&mut self, from: &str, to: &str, draft: &Draft) -> Envelope {
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
            to: to.to_owned(),
            kind: draft.kind,
            in_reply_to: draft.reply_to.clone(),
            seq: *seq,
            sent_at: yi_session::now_ms(),
            deadline_ms: draft.deadline_ms,
            body: draft.text.clone(),
            reference: draft.reference.clone(),
        }
    }

    /// Invariant: only a reply from the respondent the request named resolves its waiter, and
    /// the removal is the one retirement; any other reply is history and nothing more.
    pub(crate) fn resolve(&mut self, envelope: &Envelope) {
        let Some(id) = envelope.in_reply_to.as_ref() else {
            return;
        };
        let answers = self.waiters.get(id).is_some_and(|waiter| {
            envelope.kind == Kind::Reply && waiter.respondent == envelope.from
        });
        if answers && let Some(waiter) = self.waiters.remove(id) {
            let _the_requester_may_have_timed_out = waiter.reply.send(envelope.clone());
        }
    }
}

/// Invariant: the inbox entry is written before any delivery, and a refused write refuses the
/// send, so a receipt never names an envelope the receiver's store does not hold.
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

/// Invariant: an agent's words reach another agent inside this element and never as bare
/// user text; the envelope rides in `details`. A request names the call that answers it.
pub(crate) fn present(envelope: &Envelope) -> AgentMessage {
    let Envelope { id, from, body, .. } = envelope;
    let mut tag = format!("<agent_message from=\"{from}\"");
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

/// Retires a request's waiter on every way out: a reply, a timeout, a refused send, or the
/// requesting cell cancelled mid-wait.
struct Parked<'a>(&'a SubagentHost, MailId);

impl Drop for Parked<'_> {
    fn drop(&mut self) {
        if let Ok(mut desk) = self.0.mail.lock() {
            desk.waiters.remove(&self.1);
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
            };
            desk.waiters.insert(id.clone(), waiter);
            (id, reply)
        };
        let _parked = Parked(self, id.clone());
        let draft = Draft {
            kind: Kind::Request,
            deadline_ms: Some(yi_session::now_ms().saturating_add(timeout_ms)),
            id: Some(id.clone()),
            ..Draft::plain(text, false)
        };
        let mut sent = self.route_mail(from, target, &draft)?;
        let wait = std::time::Duration::from_millis(timeout_ms);
        let Ok(Ok(answer)) = tokio::time::timeout(wait, reply).await else {
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
