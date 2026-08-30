pub mod digest;
pub mod guard;
pub mod review;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use yi_types::advisor::{Advice, AdvisorySeverity};
use yi_types::message::{AgentMessage, UserContent};

use crate::session::AgentSession;

pub type HoldSink = Arc<dyn Fn(&Advice) -> bool + Send + Sync>;

pub const ADVISOR_GUIDANCE: &str = "weigh, don't blindly obey";
pub const DEFAULT_CADENCE: u64 = 25;
pub const OUTCOME_WINDOW: u64 = 5;

/// The LLM reviewer is off until a model role names it. Cadence applies only
/// when set: a cadence review on a clean run is pure spend.
#[derive(Clone)]
pub struct AdvisorConfig {
    pub reviewer: bool,
    pub cadence: Option<u64>,
    pub user_budget: usize,
    pub prose_budget: usize,
    pub tokens_per_hour: Option<u64>,
    pub attention: Option<String>,
    /// Where V11 promotion writes; `None` refuses to promote.
    pub rules_dir: Option<std::path::PathBuf>,
}

impl Default for AdvisorConfig {
    fn default() -> Self {
        Self {
            reviewer: false,
            cadence: None,
            user_budget: digest::DEFAULT_USER_BUDGET,
            prose_budget: digest::DEFAULT_PROSE_BUDGET,
            tokens_per_hour: None,
            attention: None,
            rules_dir: None,
        }
    }
}

/// An hourly token budget over a ring buffer of spends.
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

/// Cadence when configured, gated by the budget.
pub fn should_review(
    calls_since_review: u64,
    cadence: Option<u64>,
    budget_remaining: Option<u64>,
) -> bool {
    let due = cadence.is_some_and(|cadence| calls_since_review >= cadence);
    due && budget_remaining.is_none_or(|remaining| remaining > 0)
}

/// One `<advisory>` element, severity and target as attributes.
/// How many delivered advices stay promotable; older ones scroll out.
pub const PROMOTABLE: usize = 32;

fn rule_name(advice_id: &str, target: &str) -> String {
    let slug: String = target
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug
        .split('-')
        .filter(|part| !part.is_empty())
        .take(4)
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        advice_id.to_owned()
    } else {
        format!("{slug}-{advice_id}")
    }
}

/// A Hold becomes a gate, anything softer a reminder; the provenance line is
/// what keeps D54 honest about whose sentence the rule was.
fn rule_markdown(advice_id: &str, advice: &Advice, target: &str) -> String {
    let (mode, scope) = if advice.severity == AdvisorySeverity::Hold {
        ("gate", "tool")
    } else {
        ("remind", "tool")
    };
    format!(
        "---\ntrigger: {target}\nscope: {scope}\nmode: {mode}\ngap: 5\n---\n{}\n\n(promoted from advisor {advice_id}; edit or delete this file to change it)\n",
        advice.text.trim()
    )
}

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
    pub notes: u64,
    pub warns: u64,
    pub holds: u64,
    pub suppressed: u64,
    pub targets_touched: u64,
    pub outcomes: u64,
}

pub fn stats_text(stats: &AdvisorStats) -> String {
    format!(
        "advisor: {} reviews, delivered {} note(s) / {} warn(s) / {} hold(s), {} suppressed; outcomes: {}/{} targets touched within {OUTCOME_WINDOW} actions",
        stats.reviews,
        stats.notes,
        stats.warns,
        stats.holds,
        stats.suppressed,
        stats.targets_touched,
        stats.outcomes,
    )
}

/// Why the review was asked for; the summary itself rides the digest as its own
/// `compaction:` line, so this note stays one sentence and never wears that
/// prefix.
pub const COMPACTION_NOTE: &str =
    "the primary's view was replaced by a compaction summary; audit it";

struct PendingOutcome {
    advice_id: String,
    target: String,
    actions_left: u64,
    touched: bool,
}

struct AdvisorState {
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
    forced: bool,
    context_note: Option<String>,
    /// Delivered advice the user can still promote, newest last (V11).
    delivered: std::collections::VecDeque<(String, Advice)>,
}

fn push_log(state: &mut AdvisorState, message: AgentMessage) -> String {
    state.log_counter = state.log_counter.saturating_add(1);
    let id = format!("m{}", state.log_counter);
    state.log.push_back((id.clone(), message));
    while state.log.len() > 512 {
        state.log.pop_front();
    }
    id
}

/// work log → cadence trigger → reviewer → guard → delivery → outcome ledger,
/// over the session's event stream.
pub struct AdvisorRuntime {
    state: Mutex<AdvisorState>,
    config: AdvisorConfig,
    deliver: Arc<dyn Fn(AgentMessage) + Send + Sync>,
    hold_sink: Option<HoldSink>,
}

impl AdvisorRuntime {
    pub fn new(
        config: AdvisorConfig,
        deliver: Arc<dyn Fn(AgentMessage) + Send + Sync>,
        hold_sink: Option<HoldSink>,
    ) -> Self {
        Self {
            state: Mutex::new(AdvisorState {
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
                forced: false,
                context_note: None,
                delivered: std::collections::VecDeque::new(),
            }),
            config,
            deliver,
            hold_sink,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, AdvisorState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn stats(&self) -> String {
        stats_text(&self.lock().stats)
    }

    /// A structural moment (a plan transition) requests the next boundary's
    /// review; still budget-gated, so a poke can never overspend.
    pub fn request_review(&self, context_note: Option<String>) {
        let mut state = self.lock();
        state.forced = true;
        if context_note.is_some() {
            state.context_note = context_note;
        }
    }

    /// V5 `CompactionCheck`: the replacement summary enters the advisor's own
    /// log, so the next digest carries it beside the directives panel the judge
    /// audits it against — a head plus an id [`AdvisorRuntime::transcript`] resolves.
    pub fn note_compaction(&self, summary: String, now_ms: u64) {
        {
            let mut state = self.lock();
            let message = AgentMessage::CompactionSummary {
                summary,
                tokens_before: 0,
                timestamp: now_ms,
            };
            push_log(&mut state, message);
        }
        self.request_review(Some(COMPACTION_NOTE.to_owned()));
    }

    /// The digest chunk when a review is due, for the async LLM pass layered
    /// by the caller. Advice reaches the primary only through [`AdvisorRuntime::deliver_reviewed`].
    pub fn observe(&self, message: &AgentMessage, now_ms: u64) -> Option<String> {
        let (outcomes, chunk) = {
            let mut state = self.lock();
            let id = push_log(&mut state, message.clone());
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
            if matches!(message, AgentMessage::ToolResult { .. }) {
                state.calls_since_review = state.calls_since_review.saturating_add(1);
            }
            let remaining = state.budget.remaining(now_ms);
            let forced = state.forced && remaining.is_none_or(|remaining| remaining > 0);
            if !forced && !should_review(state.calls_since_review, self.config.cadence, remaining) {
                let outcomes = Self::drain_outcomes(&mut state);
                drop(state);
                for outcome in outcomes {
                    self.emit_outcome(&outcome, now_ms);
                }
                return None;
            }
            state.forced = false;
            state.calls_since_review = 0;
            state.stats.reviews = state.stats.reviews.saturating_add(1);
            state.guard.begin_cycle();
            let chunk = self.digest_chunk(&state);
            state.cursor = state.log_counter;
            (Self::drain_outcomes(&mut state), chunk)
        };
        for outcome in outcomes {
            self.emit_outcome(&outcome, now_ms);
        }
        Some(chunk)
    }

    /// Since the cursor: a directives header plus one line per item.
    fn digest_chunk(&self, state: &AdvisorState) -> String {
        let mut lines = Vec::new();
        if let Some(note) = &state.context_note {
            lines.push(format!("context: {note}"));
        }
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

    /// Never thinking, never another session.
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
                AgentMessage::Assistant { content, .. } => Some(digest::assistant_text(content)),
                AgentMessage::CompactionSummary { summary, .. } => Some(summary.clone()),
                _ => None,
            }
        })
    }

    /// Through the same guard, stats and delivery as the rule path.
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

    /// The advice a user can still make standing, newest last.
    pub fn promotable(&self) -> Vec<(String, String)> {
        self.lock()
            .delivered
            .iter()
            .map(|(id, advice)| (id.clone(), advice.text.clone()))
            .collect()
    }

    /// V11 (D59): the user adopts one advice as a standing D54 rule file —
    /// never a rule the runtime writes for itself.
    pub fn promote(&self, advice_id: &str) -> Result<(std::path::PathBuf, String), String> {
        let rules_dir = self
            .config
            .rules_dir
            .clone()
            .ok_or_else(|| "no rules directory is configured for this session".to_owned())?;
        let advice = {
            let state = self.lock();
            let wanted = advice_id.trim();
            state
                .delivered
                .iter()
                .rev()
                .find(|(id, _)| id == wanted)
                .map(|(_, advice)| advice.clone())
                .ok_or_else(|| {
                    format!(
                        "no delivered advice with id \"{wanted}\"; the last {PROMOTABLE} are promotable"
                    )
                })?
        };
        let target = advice.target.clone().filter(|target| !target.trim().is_empty()).ok_or_else(|| {
            "this advice names no target, so it cannot become a trigger; write the rule by hand in .yi/rules".to_owned()
        })?;
        let body = rule_markdown(advice_id, &advice, &target);
        std::fs::create_dir_all(&rules_dir)
            .map_err(|error| format!("{}: {error}", rules_dir.display()))?;
        let path = rules_dir.join(format!("{}.md", rule_name(advice_id, &target)));
        std::fs::write(&path, &body).map_err(|error| format!("{}: {error}", path.display()))?;
        Ok((path, body))
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
        // Retained before the D28 degrade: promotion adopts what the reviewer
        // judged, not the softer form headless delivery had to fall back to.
        state
            .delivered
            .push_back((advice_id.clone(), advice.clone()));
        if state.delivered.len() > PROMOTABLE {
            state.delivered.pop_front();
        }
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

/// The post-compaction hook carries no summary, so the text comes back off the
/// lane's newest compaction entry. A session with no store or no advisor gets
/// no audit — silently, because neither is an error.
pub fn note_last_compaction(
    advisor: Option<&AdvisorRuntime>,
    store: Option<&yi_session::SharedSession>,
) {
    let (Some(advisor), Some(store)) = (advisor, store) else {
        return;
    };
    let newest = yi_session::lock_session(store).find_entries_on_branch(
        "main",
        &yi_session::EntryQuery {
            entry_type: Some("compaction"),
            limit: Some(1),
            ..yi_session::EntryQuery::default()
        },
        &yi_session::BranchBounds::default(),
    );
    if let Ok(entries) = newest
        && let Some(yi_types::entry::Entry::Compaction { summary, .. }) = entries.into_iter().next()
    {
        advisor.note_compaction(summary, yi_session::now_ms());
    }
}

#[derive(Default)]
pub struct AdvisorDeps {
    pub hold_sink: Option<HoldSink>,
    pub llm: Option<Arc<review::LlmReviewer>>,
}

/// The LLM reviewer, when enabled, runs signal-gated in a spawned task and
/// feeds the same guard and delivery as the rule path.
pub fn attach_advisor(
    session: &AgentSession,
    config: AdvisorConfig,
    deps: AdvisorDeps,
) -> Arc<AdvisorRuntime> {
    let reviewer_enabled = config.reviewer;
    let deliver = session.advisory_hook();
    let runtime = Arc::new(AdvisorRuntime::new(config, deliver, deps.hold_sink));
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
                if let (Some(chunk), Some(llm)) = (review, llm.as_ref()) {
                    let advices = llm.review(&observer, &chunk).await;
                    observer.deliver_reviewed(advices, yi_session::now_ms());
                }
            }
        }
    });
    runtime
}

/// Parsed with the same reader discovery uses (an unparseable rule is removed,
/// not left to fail silently), then armed without waiting for a restart.
pub fn promote_advice(
    session: &AgentSession,
    advice_id: &str,
) -> Result<std::path::PathBuf, String> {
    let advisor = session
        .advisor()
        .ok_or_else(|| "no advisor is attached to this session".to_owned())?;
    let (path, _body) = advisor.promote(advice_id)?;
    let rule = crate::rules::read_rule(&path).map_err(|error| {
        let _ = std::fs::remove_file(&path);
        format!("promoted rule was not loadable and has been removed: {error}")
    })?;
    session
        .rules_engine()
        .ok_or_else(|| "no rules engine is attached to this session".to_owned())?
        .insert(rule);
    Ok(path)
}
