pub mod digest;
pub mod guard;
pub mod review;
pub mod signals;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use yi_types::advisor::{Advice, AdviceKind, AdvisorySeverity};
use yi_types::message::{AgentMessage, UserContent};

use crate::session::AgentSession;
use signals::{Fired, SignalKind, Signals};

pub type HoldSink = Arc<dyn Fn(&Advice) -> bool + Send + Sync>;

pub const ADVISOR_GUIDANCE: &str = "weigh, don't blindly obey";
pub const DEFAULT_CADENCE: u64 = 25;
pub const OUTCOME_WINDOW: u64 = 5;

/// Design D28 two-tier enable: signals on by default (deterministic, ~zero
/// tokens), the LLM reviewer off; cadence applies only when explicitly set —
/// a cadence review on a clean run is pure spend.
#[derive(Clone)]
pub struct AdvisorConfig {
    pub signals: bool,
    pub reviewer: bool,
    pub cadence: Option<u64>,
    pub user_budget: usize,
    pub prose_budget: usize,
    pub tokens_per_hour: Option<u64>,
    pub attention: Option<String>,
}

impl Default for AdvisorConfig {
    fn default() -> Self {
        Self {
            signals: true,
            reviewer: false,
            cadence: None,
            user_budget: digest::DEFAULT_USER_BUDGET,
            prose_budget: digest::DEFAULT_PROSE_BUDGET,
            tokens_per_hour: None,
            attention: None,
        }
    }
}

/// Design V3: an hourly token budget over a ring buffer of spends.
pub struct Budget {
    tokens_per_hour: Option<u64>,
    spent: VecDeque<(u64, u64)>,
}

impl Budget {
    pub fn new(tokens_per_hour: Option<u64>) -> Self {
        Self {
            tokens_per_hour,
            spent: VecDeque::new(),
        }
    }

    pub fn record(&mut self, now_ms: u64, tokens: u64) {
        self.spent.push_back((now_ms, tokens));
        while self.spent.len() > 512 {
            self.spent.pop_front();
        }
    }

    pub fn remaining(&self, now_ms: u64) -> Option<u64> {
        let cap = self.tokens_per_hour?;
        let hour_ago = now_ms.saturating_sub(3_600_000);
        let used: u64 = self
            .spent
            .iter()
            .filter(|(at, _)| *at >= hour_ago)
            .map(|(_, tokens)| tokens)
            .sum();
        Some(cap.saturating_sub(used))
    }
}

/// Design V2: fire on any signal, or on cadence when one is configured,
/// gated by the budget.
pub fn should_review(
    fired: &[Fired],
    calls_since_review: u64,
    cadence: Option<u64>,
    budget_remaining: Option<u64>,
) -> bool {
    let due = !fired.is_empty() || cadence.is_some_and(|cadence| calls_since_review >= cadence);
    due && budget_remaining.is_none_or(|remaining| remaining > 0)
}

/// Design V5: the single judgment seam. `RuleReviewer` maps signals to
/// canned advice at zero tokens; `LlmReviewer` is one prompt per review.
pub trait Reviewer: Send + Sync {
    fn review(&self, fired: &[Fired], digest_chunk: &str) -> Vec<Advice>;
}

pub struct RuleReviewer;

impl Reviewer for RuleReviewer {
    fn review(&self, fired: &[Fired], _digest_chunk: &str) -> Vec<Advice> {
        fired
            .iter()
            .map(|signal| {
                let evidence = signal.evidence.join("; ");
                match signal.kind {
                    SignalKind::UnbackedClaim => Advice {
                        severity: AdvisorySeverity::Warn,
                        kind: AdviceKind::Risk,
                        target: None,
                        text: format!(
                            "Claim may be unbacked by the tool log: {evidence}. Run the missing action or correct the claim."
                        ),
                    },
                    SignalKind::VerificationSkip => Advice {
                        severity: AdvisorySeverity::Warn,
                        kind: AdviceKind::Risk,
                        target: None,
                        text: format!(
                            "{evidence}. Run the relevant test or build before declaring done; name what is unverified."
                        ),
                    },
                    SignalKind::ToolFailureStreak => Advice {
                        severity: AdvisorySeverity::Warn,
                        kind: AdviceKind::Correction,
                        target: None,
                        text: format!(
                            "{evidence}. Step back and re-read the error before retrying the same approach."
                        ),
                    },
                    SignalKind::RepeatTool | SignalKind::NoOpEditRepeat => Advice {
                        severity: AdvisorySeverity::Note,
                        kind: AdviceKind::Correction,
                        target: None,
                        text: format!("{evidence}. This looks like a loop; change the approach."),
                    },
                    SignalKind::PreIrreversible => Advice {
                        severity: AdvisorySeverity::Note,
                        kind: AdviceKind::Risk,
                        target: None,
                        text: format!("{evidence}. Confirm the target before proceeding."),
                    },
                }
            })
            .collect()
    }
}

/// omp advise-tool injection, verbatim shape: one `<advisory>` element,
/// severity and target as attributes, guidance framing advice-not-orders.
pub fn advisory_text(advice: &Advice) -> String {
    let severity = match advice.severity {
        AdvisorySeverity::Note => "note",
        AdvisorySeverity::Warn => "warn",
        AdvisorySeverity::Hold => "hold",
    };
    let target = advice
        .target
        .as_deref()
        .map(|target| format!(" target=\"{target}\""))
        .unwrap_or_default();
    format!(
        "<advisory severity=\"{severity}\"{target} guidance=\"{ADVISOR_GUIDANCE}\">\n{}\n</advisory>",
        advice.text
    )
}

#[derive(Default)]
pub struct AdvisorStats {
    pub reviews: u64,
    pub signals_fired: u64,
    pub notes: u64,
    pub warns: u64,
    pub holds: u64,
    pub suppressed: u64,
    pub targets_touched: u64,
    pub outcomes: u64,
}

pub fn stats_text(stats: &AdvisorStats) -> String {
    format!(
        "advisor: {} reviews, {} signals fired, delivered {} note(s) / {} warn(s) / {} hold(s), {} suppressed; outcomes: {}/{} targets touched within {OUTCOME_WINDOW} actions",
        stats.reviews,
        stats.signals_fired,
        stats.notes,
        stats.warns,
        stats.holds,
        stats.suppressed,
        stats.targets_touched,
        stats.outcomes,
    )
}

struct PendingOutcome {
    advice_id: String,
    target: String,
    actions_left: u64,
    touched: bool,
}

struct AdvisorState {
    signals: Signals,
    guard: guard::EmissionGuard,
    budget: Budget,
    calls_since_review: u64,
    stats: AdvisorStats,
    pending: Vec<PendingOutcome>,
    counter: u64,
    log: std::collections::VecDeque<(String, AgentMessage)>,
    log_counter: u64,
    cursor: u64,
    directives: Vec<String>,
}

/// Design §7.3 pipeline, in-process: observes the session's event stream,
/// runs signals → trigger → reviewers → guard → delivery → outcome ledger.
pub struct AdvisorRuntime {
    state: Mutex<AdvisorState>,
    config: AdvisorConfig,
    deliver: Arc<dyn Fn(AgentMessage) + Send + Sync>,
    hold_sink: Option<HoldSink>,
    reviewer: Arc<dyn Reviewer>,
}

impl AdvisorRuntime {
    pub fn new(
        config: AdvisorConfig,
        deliver: Arc<dyn Fn(AgentMessage) + Send + Sync>,
        hold_sink: Option<HoldSink>,
        irreversible: Option<Arc<signals::IrreversibleProbe>>,
    ) -> Self {
        Self {
            state: Mutex::new(AdvisorState {
                signals: Signals::new(irreversible),
                guard: guard::EmissionGuard::default(),
                budget: Budget::new(config.tokens_per_hour),
                calls_since_review: 0,
                stats: AdvisorStats::default(),
                pending: Vec::new(),
                counter: 0,
                log: std::collections::VecDeque::new(),
                log_counter: 0,
                cursor: 0,
                directives: Vec::new(),
            }),
            config,
            deliver,
            hold_sink,
            reviewer: Arc::new(RuleReviewer),
        }
    }

    pub fn set_reviewer(&mut self, reviewer: Arc<dyn Reviewer>) {
        self.reviewer = reviewer;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, AdvisorState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn stats(&self) -> String {
        stats_text(&self.lock().stats)
    }

    /// Feed one finished message from the primary's stream. Returns the
    /// fired signals and the digest chunk when a review is due, for an
    /// (optional) async LLM pass layered by the caller.
    pub fn observe(&self, message: &AgentMessage, now_ms: u64) -> Option<(Vec<Fired>, String)> {
        if !self.config.signals {
            return None;
        }
        let (advices, outcomes, review) = {
            let mut state = self.lock();
            state.log_counter = state.log_counter.saturating_add(1);
            let id = format!("m{}", state.log_counter);
            state.log.push_back((id.clone(), message.clone()));
            while state.log.len() > 512 {
                state.log.pop_front();
            }
            if let AgentMessage::User {
                content: UserContent::Text(text),
                ..
            } = message
            {
                // V12: standing constraints, verbatim, append-only.
                let new_directives = digest::directives(text, &id);
                state.directives.extend(new_directives);
            }
            self.track_outcomes(&mut state, message);
            let fired = state.signals.observe(message);
            state.stats.signals_fired =
                state.stats.signals_fired.saturating_add(fired.len() as u64);
            if matches!(message, AgentMessage::ToolResult { .. }) {
                state.calls_since_review = state.calls_since_review.saturating_add(1);
            }
            let remaining = state.budget.remaining(now_ms);
            if !should_review(
                &fired,
                state.calls_since_review,
                self.config.cadence,
                remaining,
            ) {
                let outcomes = Self::drain_outcomes(&mut state);
                drop(state);
                for outcome in outcomes {
                    self.emit_outcome(&outcome, now_ms);
                }
                return None;
            }
            state.calls_since_review = 0;
            state.stats.reviews = state.stats.reviews.saturating_add(1);
            state.guard.begin_cycle();
            let chunk = self.digest_chunk(&state);
            state.cursor = state.log_counter;
            let advices = self.reviewer.review(&fired, &chunk);
            let mut accepted = Vec::new();
            for advice in advices {
                if state.guard.accept(&advice.text) {
                    accepted.push(advice);
                } else {
                    state.stats.suppressed = state.stats.suppressed.saturating_add(1);
                }
            }
            let outcomes = Self::drain_outcomes(&mut state);
            (accepted, outcomes, Some((fired, chunk)))
        };
        for outcome in outcomes {
            self.emit_outcome(&outcome, now_ms);
        }
        for advice in advices {
            self.deliver_advice(advice, now_ms);
        }
        review
    }

    /// V4 digest of the log since the cursor: header (directives panel) plus
    /// one entry-id'd line per item; never thinking.
    fn digest_chunk(&self, state: &AdvisorState) -> String {
        let mut lines = Vec::new();
        if !state.directives.is_empty() {
            lines.push(format!("directives:\n{}", state.directives.join("\n")));
        }
        for (id, message) in &state.log {
            let numeric: u64 = id.trim_start_matches('m').parse().unwrap_or(0);
            if numeric <= state.cursor {
                continue;
            }
            if let Some(line) = digest::digest_line(
                &digest::LogItem { id, message },
                self.config.user_budget,
                self.config.prose_budget,
            ) {
                lines.push(line);
            }
        }
        lines.join("\n")
    }

    /// V13: the full text of a digest-named entry — never thinking, never
    /// another session.
    pub fn transcript(&self, entry_id: &str) -> Option<String> {
        let state = self.lock();
        state.log.iter().find_map(|(id, message)| {
            if id != entry_id {
                return None;
            }
            match message {
                AgentMessage::User {
                    content: UserContent::Text(text),
                    ..
                } => Some(text.clone()),
                AgentMessage::Assistant { content, .. } => Some(signals::assistant_text(content)),
                _ => None,
            }
        })
    }

    /// Feed advices from an external (LLM) reviewer through the same guard,
    /// stats, and delivery as the rule path.
    pub fn deliver_reviewed(&self, advices: Vec<Advice>, now_ms: u64) {
        let accepted: Vec<Advice> = {
            let mut state = self.lock();
            advices
                .into_iter()
                .filter(|advice| {
                    let ok = state.guard.accept(&advice.text);
                    if !ok {
                        state.stats.suppressed = state.stats.suppressed.saturating_add(1);
                    }
                    ok
                })
                .collect()
        };
        for advice in accepted {
            self.deliver_advice(advice, now_ms);
        }
    }

    pub fn record_spend(&self, now_ms: u64, tokens: u64) {
        self.lock().budget.record(now_ms, tokens);
    }

    fn track_outcomes(&self, state: &mut AdvisorState, message: &AgentMessage) {
        if let AgentMessage::Assistant { content, .. } = message {
            for block in content {
                if let yi_types::message::Content::ToolCall { arguments, .. } = block {
                    let canonical = serde_json::to_string(arguments).unwrap_or_default();
                    for pending in &mut state.pending {
                        if pending.actions_left == 0 {
                            continue;
                        }
                        pending.actions_left = pending.actions_left.saturating_sub(1);
                        if canonical.contains(&pending.target) {
                            pending.touched = true;
                            pending.actions_left = 0;
                        }
                    }
                }
            }
        }
    }

    fn drain_outcomes(state: &mut AdvisorState) -> Vec<PendingOutcome> {
        let mut done = Vec::new();
        let mut index = 0;
        while index < state.pending.len() {
            if state.pending[index].actions_left == 0 {
                let outcome = state.pending.swap_remove(index);
                state.stats.outcomes = state.stats.outcomes.saturating_add(1);
                if outcome.touched {
                    state.stats.targets_touched = state.stats.targets_touched.saturating_add(1);
                }
                done.push(outcome);
            } else {
                index = index.saturating_add(1);
            }
        }
        done
    }

    fn emit_outcome(&self, outcome: &PendingOutcome, now_ms: u64) {
        let details = json!({
            "adviceId": outcome.advice_id,
            "targetTouchedWithin": if outcome.touched { Some(OUTCOME_WINDOW) } else { None },
        });
        (self.deliver)(AgentMessage::Custom {
            custom_type: "advisory_outcome".to_owned(),
            content: UserContent::Text(String::new()),
            display: false,
            details: Some(details),
            timestamp: now_ms,
        });
    }

    fn deliver_advice(&self, mut advice: Advice, now_ms: u64) {
        let mut state = self.lock();
        state.counter = state.counter.saturating_add(1);
        let advice_id = format!("adv-{}", state.counter);
        if advice.severity == AdvisorySeverity::Hold {
            let held = self.hold_sink.as_ref().is_some_and(|sink| sink(&advice));
            if held {
                state.stats.holds = state.stats.holds.saturating_add(1);
                return;
            }
            // D28: an Ask nobody can answer is a hang-to-timeout — headless,
            // a Hold degrades to Warn.
            advice.severity = AdvisorySeverity::Warn;
        }
        match advice.severity {
            AdvisorySeverity::Note => state.stats.notes = state.stats.notes.saturating_add(1),
            AdvisorySeverity::Warn => state.stats.warns = state.stats.warns.saturating_add(1),
            AdvisorySeverity::Hold => {}
        }
        if let Some(target) = &advice.target {
            state.pending.push(PendingOutcome {
                advice_id: advice_id.clone(),
                target: target.clone(),
                actions_left: OUTCOME_WINDOW,
                touched: false,
            });
        }
        drop(state);
        let details = serde_json::to_value(&advice).ok().map(|mut value| {
            if let Value::Object(map) = &mut value {
                map.insert("adviceId".to_owned(), Value::String(advice_id));
            }
            value
        });
        (self.deliver)(AgentMessage::Custom {
            custom_type: "advisory".to_owned(),
            content: UserContent::Text(advisory_text(&advice)),
            display: false,
            details,
            timestamp: now_ms,
        });
    }
}

#[derive(Default)]
pub struct AdvisorDeps {
    pub hold_sink: Option<HoldSink>,
    pub irreversible: Option<Arc<signals::IrreversibleProbe>>,
    pub llm: Option<Arc<review::LlmReviewer>>,
}

/// Attaches the advisor to a session's event stream (design V8 delivery:
/// running → steer at the next boundary, idle → follow-up queue; the advisor
/// never wakes an idle primary). The LLM reviewer, when enabled, runs
/// signal-gated in a spawned task and feeds the same guard and delivery.
pub fn attach_advisor(
    session: &AgentSession,
    config: AdvisorConfig,
    deps: AdvisorDeps,
) -> Arc<AdvisorRuntime> {
    let reviewer_enabled = config.reviewer;
    let deliver = session.advisory_hook();
    let runtime = Arc::new(AdvisorRuntime::new(
        config,
        deliver,
        deps.hold_sink,
        deps.irreversible,
    ));
    let mut events = session.subscribe();
    let observer = Arc::clone(&runtime);
    let llm = deps.llm.filter(|_| reviewer_enabled);
    tokio::spawn(async move {
        while let Ok(event) = events.recv().await {
            if let yi_types::event::AgentEvent::MessageEnd { message } = event {
                if matches!(message, AgentMessage::Custom { .. }) {
                    continue;
                }
                let review = observer.observe(&message, yi_session::now_ms());
                if let (Some((fired, chunk)), Some(llm)) = (review, llm.as_ref()) {
                    let advices = llm.review(&observer, &fired, &chunk).await;
                    observer.deliver_reviewed(advices, yi_session::now_ms());
                }
            }
        }
    });
    runtime
}
