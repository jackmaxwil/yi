use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};
use tokio::sync::broadcast;
use yi_permission::{
    ActionId, ActionLedger, ActionState, CatastrophicContext, ConfigRule, Decision, Hold,
    PermissionMode, RequestId, ReviewedAsk, SessionRules, ToolCall, UserVerdict,
    canonical_command_identity, canonical_tool_identity, decide,
};
use yi_tools::ToolKind;
use yi_types::event::AgentEvent;
use yi_types::permission::{Answerer, PermissionRecord, RuleDecision, RuleKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskOutcome {
    AllowOnce,
    AllowAlways(usize),
    Reject,
}

/// One approval request. `description` and `patch` are separate so a structured consumer
/// (ACP, §17.2) sends the patch as content while a text one renders `text()` over both.
pub struct PermissionAsk<'a> {
    pub title: &'a str,
    pub description: &'a str,
    pub patch: Option<&'a str>,
    pub changes: &'a [PathBuf],
    pub grants: &'a [yi_permission::Grant],
}

impl PermissionAsk<'_> {
    pub fn text(&self) -> String {
        match self.patch {
            Some(patch) => format!(
                "{}\n{}",
                self.description,
                PermissionBroker::cut_preview(patch)
            ),
            None => self.description.to_owned(),
        }
    }
}

pub type Asker = Arc<dyn Fn(&PermissionAsk<'_>) -> AskOutcome + Send + Sync>;
pub type Journal = Arc<dyn Fn(PermissionRecord) + Send + Sync>;

/// What the reviewer is told about a call, and whether it may see it at all.
#[derive(Clone, Copy)]
struct Reviewed<'a> {
    reviewable: bool,
    tool_name: &'a str,
    command: Option<&'a str>,
    reason: &'a str,
}

pub struct PermissionBroker {
    sandbox: Option<yi_tools::Sandbox>,
    contained_failures: Mutex<std::collections::BTreeSet<yi_tools::Refusal>>,
    mode: Mutex<PermissionMode>,
    config_rules: Vec<ConfigRule>,
    session_rules: Mutex<SessionRules>,
    holds: Mutex<Vec<Hold>>,
    context: CatastrophicContext,
    cwd: PathBuf,
    asker: Option<Asker>,
    events: broadcast::Sender<AgentEvent>,
    /// Set once at wiring, only when a model role names the reviewer. Unset,
    /// nothing in this file behaves differently from before it existed.
    reviewer: std::sync::OnceLock<Arc<crate::auto_review::Reviewer>>,
    ledger: Mutex<ActionLedger>,
    confirms: AtomicU64,
    journal: std::sync::OnceLock<Journal>,
}

pub struct CallOutcome {
    pub allowed: bool,
    pub reason: String,
    /// The call runs inside the platform sandbox rather than freely.
    pub contained: bool,
}

pub(crate) fn extract_targets(
    tool_name: &str,
    args: &Map<String, Value>,
    cwd: &Path,
) -> Vec<PathBuf> {
    let mut targets = Vec::new();
    let mut push = |raw: &str| targets.push(yi_permission::resolve_target(raw, cwd));
    if let Some(path) = args.get("path").and_then(Value::as_str) {
        push(path);
    }
    if tool_name == "edit"
        && let Some(patch) = args.get("patch").and_then(Value::as_str)
    {
        for line in patch.lines() {
            let trimmed = line.trim();
            if let Some(inner) = trimmed
                .strip_prefix('[')
                .and_then(|rest| rest.strip_suffix(']'))
            {
                let path_part = inner.rsplit_once('#').map_or(inner, |(path, _)| path);
                if !path_part.is_empty() {
                    push(path_part);
                }
            }
        }
    }
    targets
}

impl PermissionBroker {
    pub fn new(
        mode: PermissionMode,
        cwd: PathBuf,
        config_rules: Vec<ConfigRule>,
        asker: Option<Asker>,
        events: broadcast::Sender<AgentEvent>,
    ) -> Self {
        Self {
            sandbox: None,
            contained_failures: Mutex::new(std::collections::BTreeSet::new()),
            mode: Mutex::new(mode),
            config_rules,
            session_rules: Mutex::new(SessionRules::new()),
            holds: Mutex::new(Vec::new()),
            context: CatastrophicContext::detect(&cwd),
            cwd,
            asker,
            events,
            reviewer: std::sync::OnceLock::new(),
            ledger: Mutex::new(ActionLedger::new()),
            confirms: AtomicU64::new(0),
            journal: std::sync::OnceLock::new(),
        }
    }

    pub fn set_journal(&self, journal: Journal) {
        let _first_wiring_wins = self.journal.set(journal);
    }

    fn settle(&self, tool_call_id: &str, ask: &PermissionAsk<'_>, allowed: bool, by: Answerer) {
        let _ = self.events.send(AgentEvent::PermissionResolved {
            tool_call_id: tool_call_id.to_owned(),
            allowed,
        });
        if let Some(journal) = self.journal.get() {
            journal(PermissionRecord {
                tool_call_id: tool_call_id.to_owned(),
                title: ask.title.to_owned(),
                description: ask.text(),
                allowed,
                by,
                extra: std::collections::BTreeMap::new(),
            });
        }
    }

    /// A second call is a child session re-wiring the same broker; the first
    /// reviewer stands.
    pub fn set_reviewer(&self, reviewer: Arc<crate::auto_review::Reviewer>) {
        let _first_wiring_wins = self.reviewer.set(reviewer);
    }

    pub fn has_reviewer(&self) -> bool {
        self.reviewer.get().is_some()
    }

    /// The sandbox that makes containment real. Without one, a contained
    /// decision degrades to a question.
    #[must_use]
    pub fn with_sandbox(mut self, sandbox: Option<yi_tools::Sandbox>) -> Self {
        self.sandbox = sandbox;
        self
    }

    pub fn sandbox(&self) -> Option<&yi_tools::Sandbox> {
        self.sandbox.as_ref()
    }

    /// The sandbox is the first attempt and the question the second, remembered by the refused
    /// path so unrelated work stays contained, or by program and verb when none was found.
    pub fn note_containment_failure(&self, command: &str, exit_code: Option<i32>, output: &str) {
        if let Some(sandbox) = &self.sandbox
            && let Some(refusal) = yi_tools::refusal(sandbox, &self.cwd, exit_code, output, command)
            && let Ok(mut failures) = self.contained_failures.lock()
        {
            failures.insert(refusal);
        }
    }

    fn contained_and_failed(&self, sandbox: &yi_tools::Sandbox, command: Option<&str>) -> bool {
        let covers = |refusal: &yi_tools::Refusal| {
            command.is_some_and(|command| refusal.covers(sandbox, &self.cwd, command))
        };
        self.contained_failures
            .lock()
            .is_ok_and(|failures| failures.iter().any(covers))
    }

    /// A question with no tool call behind it, asked once and answered once: an allow-always
    /// is an allow-once here, so a confirmation never becomes a standing rule.
    pub fn confirm(&self, ask: &PermissionAsk<'_>) -> AskOutcome {
        let ordinal = self
            .confirms
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        let tool_call_id = format!("confirm-{ordinal}");
        let _ = self.events.send(AgentEvent::PermissionRequested {
            tool_call_id: tool_call_id.clone(),
            title: ask.title.to_owned(),
            description: ask.text(),
        });
        let outcome = self
            .asker
            .as_ref()
            .map_or(AskOutcome::Reject, |asker| asker(ask));
        let by = if self.asker.is_some() {
            Answerer::User
        } else {
            Answerer::Nobody
        };
        let allowed = matches!(outcome, AskOutcome::AllowOnce | AskOutcome::AllowAlways(_));
        self.settle(&tool_call_id, ask, allowed, by);
        outcome
    }

    /// Whether an interactive asker exists — without one, an advisor Hold
    /// would be an Ask nobody can answer (D28), so callers degrade it.
    pub fn can_ask(&self) -> bool {
        self.asker.is_some()
    }

    /// Matching calls become Ask, reason shown, until cleared or expired.
    pub fn insert_hold(&self, hold: Hold) {
        if let Ok(mut holds) = self.holds.lock() {
            holds.push(hold);
        }
    }

    pub fn mode(&self) -> PermissionMode {
        *self
            .mode
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A switch re-attaches the mode slot: the model is told the new policy.
    pub fn set_mode_and_fragment(&self, mode: PermissionMode, session: &crate::AgentSession) {
        self.set_mode(mode);
        if let Some(host) = session.extensions()
            && let Ok(mut host) = host.lock()
        {
            host.attach(
                crate::ext::Slot::new(crate::ext::Rank::Mode, "permission"),
                yi_permission::mode_fragment(mode).to_owned(),
            );
        }
    }

    pub fn set_mode(&self, mode: PermissionMode) {
        *self
            .mode
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = mode;
    }

    pub fn clear_holds(&self) {
        if let Ok(mut holds) = self.holds.lock() {
            holds.clear();
        }
    }

    /// Headless, an Ask degrades to a denial carrying the evidence, never a
    /// silent terminal error.
    pub fn decide_call(
        &self,
        tool_name: &str,
        kind: ToolKind,
        irreversible: bool,
        tool_call_id: &str,
        args: &Map<String, Value>,
        preview: Option<&str>,
    ) -> CallOutcome {
        let command = if tool_name == "bash" {
            args.get("command").and_then(Value::as_str)
        } else {
            None
        };
        let cwd_text = self.cwd.to_string_lossy().into_owned();
        let (rule_kind, canonical, display) = match command {
            Some(command) => (
                RuleKind::Command,
                canonical_command_identity(command, &cwd_text),
                command.to_owned(),
            ),
            None => {
                let arguments_json =
                    serde_json::to_string(&Value::Object(args.clone())).unwrap_or_default();
                let display = args
                    .get("path")
                    .and_then(Value::as_str)
                    .map(|path| format!("{tool_name} {path}"))
                    .unwrap_or_else(|| tool_name.to_owned());
                (
                    RuleKind::StructuredTool,
                    canonical_tool_identity(tool_name, &arguments_json),
                    display,
                )
            }
        };
        let targets = extract_targets(tool_name, args, &self.cwd);
        let workspace = yi_permission::lexical_normalize(&self.cwd);
        let call = ToolCall {
            tool_name,
            // Invariant: a ledger tool takes no path argument and writes only inside the
            // plans directory under a lease, so there is no target to adjudicate.
            reads_only: matches!(kind, ToolKind::Read | ToolKind::Ledger),
            irreversible,
            in_workspace: !targets.is_empty()
                && targets
                    .iter()
                    .all(|target| yi_permission::lexical_normalize(target).starts_with(&workspace)),
            rule_kind,
            canonical: &canonical,
            display: &display,
            targets: &targets,
            command,
        };
        let session_rules = self
            .session_rules
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = yi_session::now_ms();
        let active_holds: Vec<Hold> = self
            .holds
            .lock()
            .map(|holds| {
                holds
                    .iter()
                    .filter(|hold| !hold.expired(now))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let decision = decide(
            &call,
            self.mode(),
            &self.config_rules,
            &session_rules,
            &active_holds,
            &self.context,
        );
        drop(session_rules);
        let grants = yi_permission::grants(&call, &self.context);
        match decision {
            Decision::Allow { reason } => CallOutcome {
                allowed: true,
                reason,
                contained: false,
            },
            // Containment is an allowance the sandbox enforces; without one
            // there is nothing to enforce it, so the question stands.
            Decision::Contain { reason } => match &self.sandbox {
                Some(sandbox) if !self.contained_and_failed(sandbox, command) => CallOutcome {
                    allowed: true,
                    reason,
                    contained: true,
                },
                // A containment Yi cannot enforce is the same class of unknown
                // as an unprovable command, so the reviewer may see it too.
                _ => self.gated_ask(
                    &PermissionAsk {
                        title: &format!("{tool_name} requires permission"),
                        description: &format!("{reason}: {display}"),
                        patch: preview,
                        changes: &targets,
                        grants: &grants,
                    },
                    Reviewed {
                        reviewable: true,
                        tool_name,
                        command,
                        reason: &reason,
                    },
                    tool_call_id,
                    rule_kind,
                    &canonical,
                    &display,
                ),
            },
            Decision::Deny { reason } => CallOutcome {
                allowed: false,
                reason,
                contained: false,
            },
            Decision::Ask {
                title,
                description,
                reviewable,
            } => self.gated_ask(
                &PermissionAsk {
                    title: &title,
                    description: &description,
                    patch: preview,
                    changes: &targets,
                    grants: &grants,
                },
                Reviewed {
                    reviewable,
                    tool_name,
                    command,
                    reason: &description,
                },
                tool_call_id,
                rule_kind,
                &canonical,
                &display,
            ),
        }
    }

    /// The §8 auto-review gate. Off (no role named) or out of jurisdiction, this is exactly
    /// [`PermissionBroker::run_ask`] and nothing else has changed.
    fn gated_ask(
        &self,
        ask: &PermissionAsk<'_>,
        reviewed: Reviewed<'_>,
        tool_call_id: &str,
        rule_kind: RuleKind,
        canonical: &str,
        display: &str,
    ) -> CallOutcome {
        let Some(reviewer) = self.reviewer.get().cloned() else {
            return self.run_ask(ask, tool_call_id, rule_kind, canonical, display);
        };
        if !reviewed.reviewable || self.mode() != PermissionMode::Auto {
            return self.run_ask(ask, tool_call_id, rule_kind, canonical, display);
        }
        // Invariant: a call the deterministic ladder could not prove is announced and settled
        // whoever answers. The TUI's waiting cell and ACP's RequiresAction read this pair.
        let _ = self.events.send(AgentEvent::PermissionRequested {
            tool_call_id: tool_call_id.to_owned(),
            title: ask.title.to_owned(),
            description: ask.text(),
        });
        let (outcome, by) =
            self.reviewed_outcome(reviewer, ask, reviewed, rule_kind, canonical, display);
        self.settle(tool_call_id, ask, outcome.allowed, by);
        outcome
    }

    fn reviewed_outcome(
        &self,
        reviewer: Arc<crate::auto_review::Reviewer>,
        ask: &PermissionAsk<'_>,
        reviewed: Reviewed<'_>,
        rule_kind: RuleKind,
        canonical: &str,
        display: &str,
    ) -> (CallOutcome, Answerer) {
        let action = ActionId::of(canonical);
        match self.recall(action) {
            Some((request, ActionState::UserApproved)) => {
                let allowed = CallOutcome {
                    allowed: true,
                    reason: format!("allowed by the user answering request {request}"),
                    contained: false,
                };
                return (allowed, Answerer::User);
            }
            Some((request, ActionState::UserDenied)) => {
                let denied = self.denied(format!(
                    "The user denied request {request} for this exact call. It stays denied; take another approach."
                ));
                return (denied, Answerer::User);
            }
            // Idempotent by action: a retry of an identical denied call gets the same request
            // back, never a second review or a second question for the user.
            Some((request, ActionState::DeniedPendingUser)) => {
                let evidence = self.evidence_of(request);
                let denied = self.denied(Self::escalation_text(&evidence, request));
                return (denied, Answerer::Reviewer);
            }
            None => {}
        }
        let request = crate::auto_review::ReviewRequest {
            tool: reviewed.tool_name.to_owned(),
            display: display.to_owned(),
            command: reviewed.command.map(str::to_owned),
            cwd: self.cwd.to_string_lossy().into_owned(),
            targets: ask
                .changes
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            patch: ask.patch.map(Self::cut_preview),
            reason: reviewed.reason.to_owned(),
        };
        let outcome = match self.consult(reviewer, request) {
            crate::auto_review::ReviewOutcome::Allow => CallOutcome {
                allowed: true,
                reason: "allowed by the auto reviewer".to_owned(),
                contained: false,
            },
            crate::auto_review::ReviewOutcome::Deny { reason } => {
                let stored = ReviewedAsk {
                    title: ask.title.to_owned(),
                    description: ask.description.to_owned(),
                    patch: ask.patch.map(str::to_owned),
                    targets: ask.changes.to_vec(),
                    display: display.to_owned(),
                    canonical: canonical.to_owned(),
                    kind: rule_kind,
                    grants: ask.grants.to_vec(),
                    evidence: reason.clone(),
                };
                let request = self.open_request(action, stored);
                self.denied(Self::escalation_text(&reason, request))
            }
        };
        (outcome, Answerer::Reviewer)
    }

    fn escalation_text(evidence: &str, request: RequestId) -> String {
        format!(
            "The auto reviewer refused this call: {evidence}. Request {request} is open — call ask_user with request {request} to put it to the user. Re-issuing this call unchanged will not change the answer."
        )
    }

    fn evidence_of(&self, request: RequestId) -> String {
        self.ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .ask_of(request)
            .map(|ask| ask.evidence.clone())
            .unwrap_or_default()
    }

    fn denied(&self, reason: String) -> CallOutcome {
        CallOutcome {
            allowed: false,
            reason,
            contained: false,
        }
    }

    fn recall(&self, action: ActionId) -> Option<(RequestId, ActionState)> {
        let mut ledger = self
            .ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let found = ledger.state_of(action)?;
        if found.1 == ActionState::UserApproved {
            let _single_use = ledger.take_approval(action);
        }
        Some(found)
    }

    fn open_request(&self, action: ActionId, ask: ReviewedAsk) -> RequestId {
        self.ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .open(action, ask)
    }

    /// Invariant: fail safe. A provider error, an absent runtime and the
    /// timeout all leave through the same denial as a refusal would.
    fn consult(
        &self,
        reviewer: Arc<crate::auto_review::Reviewer>,
        request: crate::auto_review::ReviewRequest,
    ) -> crate::auto_review::ReviewOutcome {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return crate::auto_review::ReviewOutcome::Deny {
                reason: "the reviewer could not be reached from this thread".to_owned(),
            };
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        let timeout = std::time::Duration::from_secs(crate::levers::get().review_timeout_s);
        handle.spawn(async move {
            // Incident: nothing held the join handle, so a stalled provider kept streaming,
            // and billing, past the denial below. The deadline rides the future itself.
            let outcome = tokio::time::timeout(timeout, reviewer.review(&request))
                .await
                .unwrap_or_else(|_elapsed| crate::auto_review::ReviewOutcome::Deny {
                    reason: "the reviewer did not answer in time".to_owned(),
                });
            let _receiver_may_have_timed_out = sender.send(outcome);
        });
        receiver
            .recv_timeout(timeout)
            .unwrap_or_else(|_| crate::auto_review::ReviewOutcome::Deny {
                reason: format!("the reviewer did not answer within {}s", timeout.as_secs()),
            })
    }

    /// The `ask_user` seam: one open request, replayed verbatim to the human.
    pub fn resolve_request(&self, request: u64, tool_call_id: &str) -> String {
        let request = RequestId::new(request);
        let Some(stored) = self
            .ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .ask_of(request)
            .cloned()
        else {
            return format!("There is no open request {request}.");
        };
        // The asker blocks on a human; the ledger lock is released first so a
        // concurrent decision is not held behind the answer.
        let Some(asker) = &self.asker else {
            return format!(
                "Request {request} cannot be put to anyone: no interactive surface is available. {} Run with --yolo, or add an allow rule for this call.",
                stored.description
            );
        };
        let ask = crate::auto_review::ask_text(&stored);
        let _ = self.events.send(AgentEvent::PermissionRequested {
            tool_call_id: tool_call_id.to_owned(),
            title: stored.title.clone(),
            description: ask.text(),
        });
        let outcome = asker(&ask);
        let verdict = match outcome {
            AskOutcome::AllowOnce | AskOutcome::AllowAlways(_) => UserVerdict::Approved,
            AskOutcome::Reject => UserVerdict::Denied,
        };
        if let AskOutcome::AllowAlways(index) = outcome {
            self.keep_grant(
                stored.grants.get(index),
                stored.kind,
                &stored.canonical,
                &stored.display,
            );
        }
        let _request_was_found_above = self
            .ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .resolve(request, verdict);
        self.settle(
            tool_call_id,
            &ask,
            verdict == UserVerdict::Approved,
            Answerer::User,
        );
        crate::auto_review::resolution_text(&stored.display, verdict)
    }

    fn keep_grant(
        &self,
        grant: Option<&yi_permission::Grant>,
        kind: RuleKind,
        canonical: &str,
        display: &str,
    ) {
        let (kind, canonical, label) = match grant {
            Some(grant) => (grant.kind, grant.canonical.as_str(), grant.label.as_str()),
            None => (kind, canonical, display),
        };
        let _cap_is_soft = self
            .session_rules
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(kind, canonical, label, RuleDecision::Allow);
    }

    /// A long patch is cut: the prompt is a decision aid, not the file.
    pub(crate) fn cut_preview(patch: &str) -> String {
        const PREVIEW_LINES: usize = 40;
        let mut lines: Vec<&str> = patch.lines().take(PREVIEW_LINES).collect();
        if patch.lines().count() > PREVIEW_LINES {
            lines.push("… patch truncated");
        }
        lines.join("\n")
    }

    fn run_ask(
        &self,
        ask: &PermissionAsk<'_>,
        tool_call_id: &str,
        rule_kind: RuleKind,
        canonical: &str,
        display: &str,
    ) -> CallOutcome {
        let rendered = ask.text();
        let _ = self.events.send(AgentEvent::PermissionRequested {
            tool_call_id: tool_call_id.to_owned(),
            title: ask.title.to_owned(),
            description: rendered.clone(),
        });
        let outcome = match &self.asker {
            Some(asker) => asker(ask),
            None => {
                self.settle(tool_call_id, ask, false, Answerer::Nobody);
                let asked = yi_permission::Decision::Ask {
                    title: ask.title.to_owned(),
                    description: rendered,
                    reviewable: false,
                };
                return CallOutcome {
                    allowed: false,
                    contained: false,
                    reason: crate::gate::Report::reason_of(&crate::gate::compile_ask(asked, false)),
                };
            }
        };
        let allowed = matches!(outcome, AskOutcome::AllowOnce | AskOutcome::AllowAlways(_));
        self.settle(tool_call_id, ask, allowed, Answerer::User);
        if let AskOutcome::AllowAlways(index) = outcome {
            self.keep_grant(ask.grants.get(index), rule_kind, canonical, display);
        }
        if allowed {
            CallOutcome {
                allowed: true,
                reason: "allowed by user".to_owned(),
                contained: false,
            }
        } else {
            CallOutcome {
                allowed: false,
                reason: format!("The user denied this call. {rendered}"),
                contained: false,
            }
        }
    }
}
