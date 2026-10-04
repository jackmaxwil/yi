//! Cache outcomes as a fold over the session JSONL (design C5): a request that reads more than
//! 1,024 tokens less than the prompt before it gets one named cause, and two unexplained total
//! misses in a row on a write-billed route raise one shown notice. No LLM, nothing stored.
//! A cause names only what usage shows: which marks were sent is not in the record.
//! The same fold estimates the next gap and write for the cost-minimising TTL choice (D315):
//! [`TtlEstimate::cheapest`] writes a loop request's history for an hour when the hour's write
//! premium on the tokens the request writes costs less than the expected rewrite a five-minute
//! entry would pay, the chance of that rewrite being the session's own share of pauses between
//! five minutes and an hour.

use std::sync::{Arc, Mutex};

use yi_ai::breakpoints::{Engine, Ttl};
use yi_types::entry::Entry;
use yi_types::event::{AgentEvent, Wait};
use yi_types::message::AgentMessage;
use yi_types::model::CACHE_DIAGNOSTIC;
use yi_types::model::Model;

use crate::AgentSession;

pub const CACHE_ALERT_TYPE: &str = "cache_alert";

const SLACK: u64 = 1024;
/// Below the largest minimum cacheable prompt (Claude Haiku 4.5), a total miss is expected.
const MIN_CACHEABLE: u64 = 4096;
const FIVE_MINUTES_MS: u64 = 5 * 60 * 1000;
const HOUR_MS: u64 = 60 * 60 * 1000;
/// Pseudo-gaps added to the short band: one pause early in a short session counts for a third
/// of its gaps rather than a half.
const SHORT_PSEUDO_GAPS: f64 = 2.0;

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
    /// When the request started: a TTL runs from there (#873).
    at: u64,
    /// Its history breakpoints' TTL, as its `cache` diagnostic records.
    ttl: Ttl,
}

#[derive(Default)]
pub struct MissTracker {
    last: Option<Request>,
    /// Gaps between request starts: within five minutes, within the hour, longer.
    gaps: [u32; 3],
    last_start: Option<u64>,
    /// Tokens a request adds to the prompt, smoothed (0.7 old, 0.3 new).
    growth: Option<u64>,
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
        let elapsed = diagnostics
            .iter()
            .flatten()
            .find(|d| d.diagnostic_type == CACHE_DIAGNOSTIC)
            .and_then(|note| note.details.as_ref()?.get("elapsed_ms")?.as_u64())
            .unwrap_or(0);
        let engine = Engine::of_route(api, provider, model);
        // A record from before D315 names no TTL: its history went out for five minutes.
        let request = Request {
            route: format!("{provider}/{model}"),
            prompt,
            write,
            stable: detail(CACHE_DIAGNOSTIC, "stable"),
            upstream: detail("upstream", "provider"),
            at: at.saturating_sub(elapsed),
            ttl: if detail(CACHE_DIAGNOSTIC, "ttl").as_deref() == Some(Ttl::Hour1.label()) {
                Ttl::Hour1
            } else {
                Ttl::Min5
            },
        };
        if let Some(start) = self.last_start {
            let gap = request.at.saturating_sub(start);
            let band = usize::from(gap > FIVE_MINUTES_MS) + usize::from(gap > HOUR_MS);
            if let Some(count) = self.gaps.get_mut(band) {
                *count = count.saturating_add(1);
            }
        }
        self.last_start = Some(request.at);
        if let Some(added) = self
            .last
            .as_ref()
            .and_then(|last| prompt.checked_sub(last.prompt))
        {
            self.growth = Some(self.growth.map_or(added, |growth| {
                growth
                    .saturating_mul(7)
                    .saturating_add(added.saturating_mul(3))
                    / 10
            }));
        }
        let moved = std::mem::take(&mut self.system_moved);
        let billed = !matches!(engine, Engine::Prefix);
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
                } else if request.at.saturating_sub(last.at) > lifetime_ms(last.ttl) {
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

    /// What the fold knows for the next request's TTL; `None` before a first request and after
    /// a compaction, whose next request writes a prompt of unknown size.
    pub fn estimate(&self) -> Option<TtlEstimate> {
        let last = self.last.as_ref()?;
        Some(TtlEstimate {
            last_start: last.at,
            last_ttl: last.ttl,
            last_prompt: last.prompt,
            growth: self.growth,
            gaps: self.gaps,
        })
    }

    /// The tripwire's text, once per session.
    pub fn take_notice(&mut self) -> Option<String> {
        self.notice.take()
    }
}

/// The fold's view of the next request: when the last one started and its history's TTL, its
/// prompt, the smoothed prompt growth (`None` until two requests are seen), and the gap bands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TtlEstimate {
    pub last_start: u64,
    pub last_ttl: Ttl,
    pub last_prompt: u64,
    pub growth: Option<u64>,
    pub gaps: [u32; 3],
}

fn lifetime_ms(ttl: Ttl) -> u64 {
    match ttl {
        Ttl::Min5 => FIVE_MINUTES_MS,
        Ttl::Hour1 => HOUR_MS,
    }
}

fn tokens_f64(count: u64) -> f64 {
    f64::from(u32::try_from(count).unwrap_or(u32::MAX))
}

impl TtlEstimate {
    /// The TTL whose expected cost over this request and the next is lower, at `model`'s own
    /// prices, for a request sent at `now` ms. An hour costs `(w1 − w5)` on the tokens this
    /// request writes: its growth while the last entry lives, the whole prompt once it is gone.
    /// It saves `(w5 − r)` on the prompt the next request would read, when the gap between them
    /// falls between five minutes and an hour; within five minutes either entry is read, past
    /// the hour neither is. A one-hour mark reads a live five-minute entry at its own position
    /// (probe F8), so switching costs no more than this. `w1` is twice the input price, as
    /// Anthropic prices it; the catalog has no field for it. A growth never seen is priced as
    /// the whole prompt. The chance of a middle gap is the session's own share of them, with
    /// [`SHORT_PSEUDO_GAPS`] added to the short band, so a session that has not yet paused that
    /// long keeps five minutes.
    pub fn cheapest(&self, model: &Model, now: u64) -> Ttl {
        let [short, middle, long] = self.gaps;
        if !matches!(Engine::of(model), Engine::Breakpoint { hour: true, .. }) {
            return Ttl::Min5;
        }
        let price = |number: &serde_json::Number| number.as_f64().unwrap_or(0.0);
        let (input, write, read) = (
            price(&model.cost.input),
            price(&model.cost.cache_write),
            price(&model.cost.cache_read),
        );
        if input <= 0.0 || write <= 0.0 {
            return Ttl::Min5;
        }
        let growth = self.growth.unwrap_or(self.last_prompt);
        let prompt = tokens_f64(self.last_prompt.saturating_add(self.growth.unwrap_or(0)));
        let written = if now.saturating_sub(self.last_start) <= lifetime_ms(self.last_ttl) {
            tokens_f64(growth)
        } else {
            prompt
        };
        let seen = f64::from(short) + f64::from(middle) + f64::from(long);
        let middle = f64::from(middle) / (seen + SHORT_PSEUDO_GAPS);
        if written * (2.0 * input - write) < middle * prompt * (write - read) {
            Ttl::Hour1
        } else {
            Ttl::Min5
        }
    }
}

/// Watches the session's own requests, shows the notice when the tripwire fires, and hands
/// the fold's estimate to the session's provider for its next loop request.
pub fn attach(session: &AgentSession) {
    let tracker = Arc::new(Mutex::new(MissTracker::default()));
    let provider = Arc::clone(session.provider_arc());
    let (resumed, seeded) = (Arc::clone(&tracker), Arc::clone(&provider));
    session.on_attach(move |_, entries, _| {
        let mut folded = MissTracker::default();
        for entry in entries {
            folded.observe_entry(entry);
        }
        // A notice the ledger earned was shown by the process that wrote it.
        folded.notice = None;
        seeded.set_ttl_estimate(folded.estimate());
        if let Ok(mut tracker) = resumed.lock() {
            *tracker = folded;
        }
    });
    session.show_notices(CACHE_ALERT_TYPE, move |event| {
        let Ok(mut tracker) = tracker.lock() else {
            return None;
        };
        match event {
            AgentEvent::MessageEnd { message } => {
                tracker.observe(message, yi_session::now_ms());
            }
            AgentEvent::Wait {
                wait: Some(Wait::Compaction { .. }),
            } => tracker.last = None,
            _ => return tracker.take_notice(),
        }
        provider.set_ttl_estimate(tracker.estimate());
        tracker.take_notice()
    });
}
