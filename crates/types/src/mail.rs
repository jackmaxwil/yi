use serde::{Deserialize, Serialize};

use crate::url::Url;

/// Names one envelope within its host: the sender and the host's own count, as `parent-17`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MailId(pub String);

impl std::fmt::Display for MailId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    #[default]
    Inform,
    Request,
    Reply,
    Progress,
    Failure,
    Cancel,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inform => "inform",
            Self::Request => "request",
            Self::Reply => "reply",
            Self::Progress => "progress",
            Self::Failure => "failure",
            Self::Cancel => "cancel",
        }
    }
}

/// One message between family members. The host fills `id`, `from`, `to`, `seq` and `sentAt`;
/// a sender's payload never reaches them. `seq` orders one sender's messages to one recipient.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Envelope {
    pub id: MailId,
    pub from: String,
    /// Which run of a service under its name sent or is addressed, counted from one and
    /// filled by the host: `None` from a member that is no service, and to whoever holds `to`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_incarnation: Option<u32>,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_incarnation: Option<u32>,
    pub kind: Kind,
    pub conversation: MailId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<MailId>,
    pub seq: u64,
    pub sent_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_ms: Option<u64>,
    pub body: String,
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<Url>,
    /// `human` on a parent's reply the human wrote from the reply box, `final_text` on a reply
    /// the host took off the end of a turn that left the request open; absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_by: Option<String>,
}

/// What the host did with an accepted envelope, every one written to the inbox first:
/// `Queued` for a running turn to drain, `Woken` a turn started on it, `Inboxed` neither,
/// `Answered` a reply the waiting request's call returned, which is never presented again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    Queued,
    Woken,
    Inboxed,
    Answered,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub target: String,
    pub id: MailId,
    pub state: Delivery,
    /// When the receiver's model reads it, in words: which queue it took and what drains it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub presented: String,
}

impl crate::entry::CustomRecord for Envelope {
    const TYPE: &'static str = "agent_message";
}
