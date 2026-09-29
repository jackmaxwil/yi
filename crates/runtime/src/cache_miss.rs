//! Cache outcomes as a fold over the session JSONL (design C5): a request that reads more than
//! 1,024 tokens less than the prompt before it gets one named cause, and two unexplained total
//! misses in a row on a write-billed route raise one shown notice. No LLM, nothing stored.
//! A cause names only what usage shows: which marks were sent is not in the record.

use yi_ai::breakpoints::Engine;
use yi_types::entry::Entry;
use yi_types::event::{AgentEvent, Wait};
use yi_types::message::AgentMessage;
use yi_types::model::CACHE_DIAGNOSTIC;

use crate::AgentSession;

pub const CACHE_ALERT_TYPE: &str = "cache_alert";

const SLACK: u64 = 1024;
/// Below the largest minimum cacheable prompt (Claude Haiku 4.5), a total miss is expected.
const MIN_CACHEABLE: u64 = 4096;
// ponytail: the 5m TTL for every route; an ADAPT 1h conversation (C6) needs the plan's TTL here.
const TTL_MS: u64 = 5 * 60 * 1000;

/// Why a request read less than the prompt before it, in the order the causes are tested.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissCause {
    ModelSwitch,
    /// Schema, tools or system moved: a yi bug since the system prompt is constant (D310).
    StableKeyChanged,
    Expired,
    UpstreamSwitch,
    /// The previous request wrote and this one did not read it back: an eviction, a mark on a
    /// tail no later request sends, or a route the fold cannot see.
    WriteNotRead,
    /// A write-billed route that neither wrote nor read: no mark sent, or every mark ignored.
    NothingCached,
    Unexplained,
}

impl MissCause {
    pub fn label(self) -> &'static str {
        match self {
            Self::ModelSwitch => "model switch",
            Self::StableKeyChanged => "stable key changed",
            Self::Expired => "gap exceeded TTL",
            Self::UpstreamSwitch => "upstream switch",
            Self::WriteNotRead => "previous write not read",
            Self::NothingCached => "nothing written or read",
            Self::Unexplained => "unexplained",
        }
    }
}

struct Request {
    route: String,
    prompt: u64,
    write: u64,
    stable: Option<String>,
    upstream: Option<String>,
    at: u64,
}

#[derive(Default)]
pub struct MissTracker {
    last: Option<Request>,
    /// Before D310 a late attach rewrote the system and the record carried no key.
    system_moved: bool,
    total_misses: u32,
    announced: bool,
    notice: Option<String>,
}

fn tokens(count: i64) -> u64 {
    u64::try_from(count).unwrap_or(0)
}

impl MissTracker {
    pub fn observe_entry(&mut self, entry: &Entry) -> Option<MissCause> {
        match entry {
            Entry::Message {
                message, timestamp, ..
            } => self.observe(message, *timestamp),
            // A declared reset: the history is rewritten, so the next request owes no read.
            Entry::Compaction { .. } | Entry::BranchSummary { .. } => {
                self.last = None;
                None
            }
            Entry::Custom { custom_type, .. } if custom_type == "ext_state" => {
                self.system_moved = self.last.is_some();
                None
            }
            _ => None,
        }
    }

    pub fn observe(&mut self, message: &AgentMessage, at: u64) -> Option<MissCause> {
        let AgentMessage::Assistant {
            api,
            provider,
            model,
            usage,
            diagnostics,
            ..
        } = message
        else {
            return None;
        };
        let (read, write) = (tokens(usage.cache_read), tokens(usage.cache_write));
        let prompt = tokens(usage.input)
            .saturating_add(read)
            .saturating_add(write);
        if prompt == 0 || usage.unknown {
            return None;
        }
        let detail = |kind: &str, key: &str| {
            let note = diagnostics
                .iter()
                .flatten()
                .find(|d| d.diagnostic_type == kind)?;
            Some(note.details.as_ref()?.get(key)?.as_str()?.to_owned())
        };
        let request = Request {
            route: format!("{provider}/{model}"),
            prompt,
            write,
            stable: detail(CACHE_DIAGNOSTIC, "stable"),
            upstream: detail("upstream", "provider"),
            at,
        };
        let moved = std::mem::take(&mut self.system_moved);
        let billed = !matches!(Engine::of_route(api, provider, model), Engine::Prefix);
        let cause = self
            .last
            .as_ref()
            .filter(|last| last.prompt.saturating_sub(read) > SLACK)
            .map(|last| {
                let differs =
                    |a: &Option<String>, b: &Option<String>| a.is_some() && b.is_some() && a != b;
                if last.route != request.route {
                    MissCause::ModelSwitch
                } else if differs(&last.stable, &request.stable)
                    || (request.stable.is_none() && moved)
                {
                    MissCause::StableKeyChanged
                } else if request.at.saturating_sub(last.at) > TTL_MS {
                    MissCause::Expired
                } else if differs(&last.upstream, &request.upstream) {
                    MissCause::UpstreamSwitch
                } else if last.write > 0 {
                    MissCause::WriteNotRead
                } else if read == 0 && billed {
                    MissCause::NothingCached
                } else {
                    MissCause::Unexplained
                }
            });
        let unexplained = matches!(
            cause,
            Some(MissCause::WriteNotRead | MissCause::NothingCached | MissCause::Unexplained)
        );
        self.total_misses = if unexplained && read == 0 && billed && prompt >= MIN_CACHEABLE {
            self.total_misses.saturating_add(1)
        } else {
            0
        };
        if self.total_misses >= 2 && !self.announced {
            self.announced = true;
            self.notice = cause.map(|cause| {
                format!(
                    "[cache] {} has read 0 cached tokens over {} requests ({}k prompt, cause: {}); every request is billed in full. Check the catalog entry or report it.",
                    request.route,
                    self.total_misses,
                    prompt / 1000,
                    cause.label(),
                )
            });
        }
        self.last = Some(request);
        cause
    }

    /// The tripwire's text, once per session.
    pub fn take_notice(&mut self) -> Option<String> {
        self.notice.take()
    }
}

/// Watches the session's own requests and shows the notice when the tripwire fires.
pub fn attach(session: &AgentSession) {
    let mut tracker = MissTracker::default();
    session.show_notices(CACHE_ALERT_TYPE, move |event| {
        match event {
            AgentEvent::MessageEnd { message } => {
                tracker.observe(message, yi_session::now_ms());
            }
            AgentEvent::Wait {
                wait: Some(Wait::Compaction { .. }),
            } => tracker.last = None,
            _ => {}
        }
        tracker.take_notice()
    });
}
