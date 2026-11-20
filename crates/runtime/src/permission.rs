use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::claude_hooks::Verdict;
use serde_json::{Map, Value};
use tokio::sync::broadcast;
use yi_permission::{
    CatastrophicContext, ConfigRule, Decision, Hold, PermissionMode, SessionRules, ToolCall,
    canonical_command_identity, canonical_tool_identity, decide,
};
use yi_tools::hashline::types::FileOp;
use yi_tools::{CancelFlag, ToolKind};
use yi_types::event::AgentEvent;
use yi_types::permission::{
    Answerer, PermissionRecord, RuleDecision, RuleKind, SessionPermissionRule,
};

mod egress;
mod forge;
mod hooked;
pub(crate) use hooked::claude_dir;
mod kept;
pub(crate) use kept::protected;
mod late;
use late::{Answered, Late, stop_released};

mod ask;
pub use ask::{AskOutcome, Asker, PermissionAsk};
pub type Journal<T = PermissionRecord> = Arc<dyn Fn(T) + Send + Sync>;

const REFUSED_CAP: usize = 256;

pub struct PermissionBroker {
    sandbox: Option<yi_tools::Sandbox>,
    contained_failures: Mutex<std::collections::BTreeSet<yi_tools::SandboxRefusal>>,
    /// Shared with every child: a mode switch or a hold reaches the whole family.
    mode: Arc<Mutex<PermissionMode>>,
    config_rules: Vec<ConfigRule>,
    /// The holder's own: a child starts from a copy ([`PermissionBroker::for_child`]).
    session_rules: Mutex<SessionRules>,
    holds: Arc<Mutex<Vec<Hold>>>,
    context: CatastrophicContext,
    cwd: PathBuf,
    asker: Option<Asker>,
    events: broadcast::Sender<AgentEvent>,
    /// Canonical identities of calls the user refused, so the classifier cannot overrule them.
    refused: Mutex<std::collections::BTreeSet<String>>,
    confirms: AtomicU64,
    journal: std::sync::OnceLock<Journal>,
    /// Appends each kept rule to the holder's own session JSONL, the one ledger of them.
    rule_journal: std::sync::OnceLock<Journal<SessionPermissionRule>>,
    approver: std::sync::OnceLock<Arc<crate::classifier::Approver>>,
    stop: std::sync::OnceLock<CancelFlag>,
    prompts_close_on_settle: std::sync::atomic::AtomicBool,
    /// A walled holder's retry never leaves the sandbox for a protected dir.
    walled: bool,
    /// Nobody is attached to answer a question, so a root's `ask_user` cannot end the run.
    unattended: bool,
    /// Loaded once from the root's tree and shared down: a lane child reads the root's project.
    claude_hooks: std::sync::OnceLock<Option<Arc<crate::claude_hooks::Hooks>>>,
    egress: egress::Named,
    host_recipes: std::sync::OnceLock<Vec<yi_types::config::HostRecipe>>,
}

pub struct CallOutcome {
    pub allowed: bool,
    pub reason: String,
    pub containment: Containment,
}

/// What an allowed call runs as. Only bash reads it, and only where a sandbox exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Containment {
    /// Inside the sandbox, with these directories writable besides the holder's tree.
    Contained {
        widen: Vec<PathBuf>,
    },
    Uncontained,
}

/// Why a command must leave the sandbox (network, an install, a credential store), and whether
/// every segment must: `false` is a compound whose leaving part would carry the rest out.
pub(crate) fn leaves_sandbox(
    command: &str,
    context: &CatastrophicContext,
) -> Option<(&'static str, bool)> {
    let reads = |text: &str| {
        yi_permission::command_reads_credentials(text, context).map(|_| "credential stores")
    };
    let needs: Vec<Option<&'static str>> = yi_permission::command_segments(command)
        .iter()
        .map(|argv| yi_permission::host_need(argv).or_else(|| reads(&argv.join(" "))))
        .collect();
    // Only a strict parse splits truly: the lenient split folds `$(…)` or a newline into argv.
    let parsed = matches!(
        yi_permission::parse(command),
        yi_permission::Parsed::Segments(_)
    );
    match needs.iter().flatten().next() {
        Some(why) => Some((why, parsed && needs.iter().all(Option::is_some))),
        None => reads(command).map(|why| (why, false)),
    }
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
        // A glob walks from its literal head, so the gates judge that directory too.
        if let Some(head) = yi_tools::glob_head(path) {
            push(&head);
        }
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
        // The tool's own parse names every path it writes: a section's `MV` destination too.
        let parsed = yi_tools::hashline::input::Patch::parse(patch, Some(cwd));
        for section in parsed.map(|patch| patch.sections).unwrap_or_default() {
            push(&section.path);
            if let Ok(Some(FileOp::Move { dest })) = section.parse().map(|parsed| parsed.file_op) {
                push(&dest);
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
            mode: Arc::new(Mutex::new(mode)),
            config_rules,
            session_rules: Mutex::new(SessionRules::new()),
            holds: Arc::new(Mutex::new(Vec::new())),
            context: CatastrophicContext::detect(&cwd),
            cwd,
            asker,
            events,
            refused: Mutex::new(std::collections::BTreeSet::new()),
            confirms: AtomicU64::new(0),
            journal: std::sync::OnceLock::new(),
            rule_journal: std::sync::OnceLock::new(),
            approver: std::sync::OnceLock::new(),
            stop: std::sync::OnceLock::new(),
            prompts_close_on_settle: std::sync::atomic::AtomicBool::new(false),
            walled: false,
            unattended: false,
            claude_hooks: std::sync::OnceLock::new(),
            egress: egress::Named::default(),
            host_recipes: std::sync::OnceLock::new(),
        }
    }

    pub(crate) fn claude_hooks(&self) -> Option<Arc<crate::claude_hooks::Hooks>> {
        (self.claude_hooks)
            .get_or_init(|| crate::claude_hooks::Hooks::load(&self.cwd))
            .clone()
    }

    pub fn set_journal(&self, journal: Journal) {
        let _first_wiring_wins = self.journal.set(journal);
    }

    pub fn set_stop(&self, stop: CancelFlag) {
        let _first_wiring_wins = self.stop.set(stop);
    }

    pub fn set_approver(&self, approver: Arc<crate::classifier::Approver>) {
        let _first_wiring_wins = self.approver.set(approver);
    }

    pub fn prompts_close_on_settle(&self) {
        self.prompts_close_on_settle.store(true, Ordering::Relaxed);
    }

    pub fn asks_close_on_settle(&self) -> bool {
        self.prompts_close_on_settle.load(Ordering::Relaxed)
    }

    fn settle(&self, tool_call_id: &str, ask: &PermissionAsk<'_>, allowed: bool, by: Answerer) {
        self.resolved(tool_call_id, allowed);
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

    fn resolved(&self, tool_call_id: &str, allowed: bool) {
        let _ = self.events.send(AgentEvent::PermissionResolved {
            tool_call_id: tool_call_id.to_owned(),
            allowed,
        });
    }

    /// The sandbox that makes containment real. Without one, a contained
    /// decision degrades to a question.
    #[must_use]
    pub fn with_unattended(mut self, unattended: bool) -> Self {
        self.unattended = unattended;
        self
    }

    pub fn unattended(&self) -> bool {
        self.unattended
    }

    pub fn with_sandbox(mut self, sandbox: Option<yi_tools::Sandbox>) -> Self {
        self.sandbox = sandbox;
        self
    }

    pub fn sandbox(&self) -> Option<&yi_tools::Sandbox> {
        self.sandbox.as_ref()
    }

    /// The sandbox is the first attempt and the question the second, remembered by the refused
    /// path so unrelated work stays contained, or by program and verb when none was found.
    pub fn note_containment_failure(&self, refusal: yi_tools::SandboxRefusal) {
        if let Ok(mut failures) = self.contained_failures.lock() {
            failures.insert(refusal);
        }
    }

    fn retried_refusal(&self, command: Option<&str>) -> Option<yi_tools::SandboxRefusal> {
        let (sandbox, command) = (self.sandbox.as_ref()?, command?);
        let failures = self.contained_failures.lock().ok()?;
        // The deepest refused path: a refusal at `~/x` would otherwise claim every retry in home.
        (failures.iter())
            .filter(|refusal| refusal.retried_by(sandbox, &self.cwd, command))
            .max_by_key(|refusal| match refusal {
                yi_tools::SandboxRefusal::Path(path) => path.components().count(),
                yi_tools::SandboxRefusal::Scopes(_) => 0,
            })
            .cloned()
    }

    fn containment_with(&self, refused: Option<PathBuf>) -> Containment {
        let mut widen = self.kept_writes();
        widen.extend(refused);
        Containment::Contained { widen }
    }

    /// An allowed bash call runs contained unless it must leave (#600 stage 2b); `Err` asks, as a
    /// refused contained call does, save the session pass a kept rule gives a pathless refusal.
    fn allowed_outcome(
        &self,
        command: Option<&str>,
        refusal: Option<&yi_tools::SandboxRefusal>,
        passed: bool,
        reason: String,
    ) -> Result<CallOutcome, String> {
        let outside = |reason: String| CallOutcome {
            allowed: true,
            reason,
            containment: Containment::Uncontained,
        };
        let Some(command) = command.filter(|_| self.sandbox.is_some()) else {
            return Ok(outside(reason));
        };
        if self.mode() == PermissionMode::Yolo {
            return Ok(outside(reason));
        }
        match leaves_sandbox(command, &self.context) {
            Some((why, true)) => {
                return Ok(outside(format!(
                    "{reason}; it runs outside the sandbox ({why})"
                )));
            }
            Some((why, false)) => {
                return Err(format!(
                    "part of the command must run outside the sandbox ({why}) and would take the rest with it ({reason})"
                ));
            }
            None => {}
        }
        match refusal {
            None => Ok(CallOutcome {
                allowed: true,
                reason,
                containment: self.containment_with(None),
            }),
            Some(yi_tools::SandboxRefusal::Scopes(_)) if passed => Ok(outside(format!(
                "{reason}; it runs outside the sandbox (an \"always\" kept this exact command after a refusal naming no path)"
            ))),
            Some(_) => Err(format!(
                "the sandbox refused its last contained run ({reason})"
            )),
        }
    }

    /// What approving a bash call runs as, and the clause its question adds: a call needing the
    /// network or a credential store leaves; a refused write's retry widens by its unprotected dir.
    fn approved_containment(
        &self,
        command: Option<&str>,
        refusal: Option<&yi_tools::SandboxRefusal>,
    ) -> (Containment, String) {
        let (Some(sandbox), Some(command)) = (&self.sandbox, command) else {
            return (Containment::Uncontained, String::new());
        };
        let outside = |why: &str| {
            let note = format!("; approving runs this one call outside the sandbox ({why})");
            (Containment::Uncontained, note)
        };
        if let Some((why, _)) = leaves_sandbox(command, &self.context) {
            return outside(why);
        }
        match refusal {
            None => (self.containment_with(None), String::new()),
            Some(yi_tools::SandboxRefusal::Scopes(_)) => outside("the refusal named no path"),
            Some(yi_tools::SandboxRefusal::Path(path)) => {
                let dir = path.parent().unwrap_or(path);
                let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
                if protected(Some(sandbox), &dir) {
                    return outside(&format!("`{}` is protected", dir.display()));
                }
                let note = format!("; approving widens this run by `{}`", dir.display());
                (self.containment_with(Some(dir)), note)
            }
        }
    }

    /// A contained call's profile: the holder's tree (a child's broker keeps its parent's cwd),
    /// the approved directories, and the wall, enforced rather than read off the command text.
    pub fn sandbox_for(
        &self,
        cwd: &Path,
        wall: &crate::wall::Wall,
        widen: &[PathBuf],
    ) -> Option<yi_tools::Sandbox> {
        let base = self.sandbox.as_ref()?;
        let mut profile = match std::env::var_os("HOME").filter(|_| cwd != self.cwd) {
            Some(home) => yi_tools::Sandbox {
                deny_read: base.deny_read.clone(),
                host_owned: base.host_owned.clone(),
                ..yi_tools::Sandbox::for_workspace(cwd, Path::new(&home), None)
                    .with_caches(Path::new(&home))
            },
            None => base.clone(),
        };
        profile.writable.extend_from_slice(widen);
        profile.deny_write.extend_from_slice(&wall.deny_write);
        profile.deny_write.extend(claude_dir(Some(self)));
        profile.deny_read.extend_from_slice(&wall.deny_read);
        Some(profile)
    }

    /// A question with no tool call behind it, asked once and answered once: an allow-always
    /// is an allow-once here, so a confirmation never becomes a standing rule.
    pub fn confirm(&self, ask: &PermissionAsk<'_>) -> AskOutcome {
        self.confirm_judged(ask, None).0
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
        let call = (tool_name, kind, irreversible, tool_call_id);
        self.decide_hooked(call, args, preview, &Verdict::Silent)
    }

    pub(crate) fn decide_hooked(
        &self,
        (tool_name, kind, irreversible, tool_call_id): (&str, ToolKind, bool, &str),
        args: &Map<String, Value>,
        preview: Option<&str>,
        hook: &Verdict,
    ) -> CallOutcome {
        let command = if tool_name == "bash" {
            args.get("command").and_then(Value::as_str)
        } else {
            None
        };
        if let Some(outcome) = self.host_recipe(command) {
            return outcome;
        }
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
        let decision = self.forge_gate(command, decision);
        let judged = hooked::Judged {
            call: &call,
            config_rules: &self.config_rules,
            session_rules: &session_rules,
            holds: &active_holds,
        };
        let (decision, hook_answers) = self.hooked(decision, hook, &judged);
        // The session pass is the exact command an "always" kept, never a scope (review of #933).
        let passed = session_rules.decision_for(rule_kind, &canonical) == Some(RuleDecision::Allow);
        drop(session_rules);
        let refusal = self.retried_refusal(command);
        if let Some(refused) = self.walled_retry(command, refusal.as_ref()) {
            return self.denied(refused);
        }
        let mut split = None;
        let (title, description, reviewable, reason) = match decision {
            Decision::Allow { reason } => {
                split = (command.and_then(|command| leaves_sandbox(command, &self.context)))
                    .filter(|(_, every)| !every)
                    .map(|(why, _)| why);
                match self.allowed_outcome(command, refusal.as_ref(), passed, reason) {
                    Ok(outcome) => return outcome,
                    Err(reason) => (
                        format!("{tool_name} requires permission"),
                        format!("{reason}: {display}"),
                        true,
                        reason,
                    ),
                }
            }
            Decision::Deny { reason } => return self.denied(reason),
            Decision::Contain { reason } if self.sandbox.is_some() && refusal.is_none() => {
                return CallOutcome {
                    allowed: true,
                    reason,
                    containment: self.containment_with(None),
                };
            }
            // A containment Yi cannot enforce, or one the sandbox already refused, is the same
            // class of unknown as an unprovable command, so the classifier may see it too.
            Decision::Contain { reason } => (
                format!("{tool_name} requires permission"),
                format!("{reason}: {display}"),
                true,
                reason,
            ),
            Decision::Ask {
                title,
                description,
                reviewable,
            } => (title, description.clone(), reviewable, description),
        };
        if hook_answers {
            let ask = PermissionAsk {
                title: &title,
                description: &description,
                patch: preview,
                changes: &targets,
                grants: &[],
                tool_call_id: Some(tool_call_id),
                stop: None,
            };
            return self.hook_answered(&ask, &reason, command);
        }
        let (containment, note) = self.approved_containment(command, refusal.as_ref());
        // An "always" on a widened retry keeps the directory, never a rule that would run the
        // command itself outside the sandbox.
        let pathless =
            command.is_some_and(|command| leaves_sandbox(command, &self.context).is_none());
        let kept = match (&containment, &refusal) {
            (Containment::Contained { widen, .. }, Some(yi_tools::SandboxRefusal::Path(_))) => {
                widen.last().map(|dir| yi_permission::write_grant(dir))
            }
            (Containment::Uncontained, Some(yi_tools::SandboxRefusal::Scopes(_))) if pathless => {
                Some(yi_permission::Grant {
                    kind: rule_kind,
                    canonical: canonical.clone(),
                    label: "this exact command, outside the sandbox, for the rest of this session"
                        .to_owned(),
                })
            }
            _ => None,
        };
        let grants = match &kept {
            Some(grant) => vec![grant.clone()],
            None => yi_permission::grants(&call, &self.context),
        };
        let outcome = self.gated_ask(
            &PermissionAsk {
                title: &title,
                description: &format!("{description}{note}"),
                patch: preview,
                changes: &targets,
                grants: &grants,
                tool_call_id: Some(tool_call_id),
                stop: self.stop.get(),
            },
            reviewable,
            (tool_call_id, tool_name, command, &format!("{reason}{note}")),
            (rule_kind, &canonical, &display),
        );
        if !outcome.allowed {
            // Nobody could answer; say what works, never "add an allow rule", which it had.
            return match (split, &refusal) {
                (Some(why), _) if self.asker.is_none() => {
                    self.denied(crate::gate::headless_split(why))
                }
                (None, Some(refusal)) if self.asker.is_none() => self.denied(
                    crate::gate::headless_refusal(self.sandbox.as_ref(), refusal),
                ),
                _ => crate::gate::named_rule(outcome, self.asker.is_none(), (tool_name, &display)),
            };
        }
        if let (Some(grant), Some(refusal), Containment::Contained { .. }) =
            (&kept, &refusal, &containment)
            && self.keeps(grant)
            && let Ok(mut failures) = self.contained_failures.lock()
        {
            failures.remove(refusal);
        }
        CallOutcome {
            allowed: true,
            reason: format!("{}{note}", outcome.reason),
            containment,
        }
    }

    /// The line an allowed bash call's result carries when it ran outside a sandbox that exists:
    /// the tool text promises containment, so leaving it is said where the model reads.
    pub fn outside_notice(
        &self,
        tool_name: &str,
        args: &Map<String, Value>,
        outcome: &CallOutcome,
    ) -> Option<String> {
        let outside = outcome.allowed && outcome.containment == Containment::Uncontained;
        // A `job=N` wait or kill runs no command, so it ran nowhere.
        let ran = tool_name == "bash" && args.get("command").and_then(Value::as_str).is_some();
        let promised = ran && self.sandbox.is_some() && self.mode() != PermissionMode::Yolo;
        (outside && promised).then(|| format!("sandbox: {}", outcome.reason))
    }

    fn keeps(&self, grant: &yi_permission::Grant) -> bool {
        self.session_rules.lock().is_ok_and(|rules| {
            rules.decision_for(grant.kind, &grant.canonical) == Some(RuleDecision::Allow)
        })
    }

    /// The classifier gets the first word in auto mode; whatever it does not settle goes to
    /// whoever answers.
    fn gated_ask(
        &self,
        ask: &PermissionAsk<'_>,
        reviewable: bool,
        (tool_call_id, tool_name, command, reason): (&str, &str, Option<&str>, &str),
        (rule_kind, canonical, display): (RuleKind, &str, &str),
    ) -> CallOutcome {
        let cwd = self.cwd.to_string_lossy();
        let call = crate::classifier::Call {
            tool: tool_name,
            display,
            command,
            reason,
            cwd: &cwd,
        };
        let approver = (self.approver.get()).filter(|_| {
            reviewable
                && !self.answered(canonical)
                && self.mode() == PermissionMode::Auto
                && self.asker.is_some()
        });
        let late = approver.and_then(|approver| Late::of(approver, &call));
        if let Some(approver) = approver.filter(|_| late.is_none())
            && let crate::classifier::Judgement::Allow(safe) = approver.judge(&call)
        {
            let _ = self.events.send(AgentEvent::PermissionRequested {
                tool_call_id: tool_call_id.to_owned(),
                title: ask.title.to_owned(),
                description: ask.text(),
            });
            self.settle(tool_call_id, ask, true, Answerer::Classifier);
            return CallOutcome {
                allowed: true,
                reason: format!("allowed by the classifier (P(safe) {safe:.2})"),
                containment: Containment::Uncontained,
            };
        }
        self.run_ask(ask, tool_call_id, rule_kind, canonical, display, late)
    }

    fn denied(&self, reason: String) -> CallOutcome {
        CallOutcome {
            allowed: false,
            reason,
            containment: Containment::Uncontained,
        }
    }

    /// Past [`REFUSED_CAP`] the oldest answers are forgotten wholesale: a forgotten refusal
    /// only lets the classifier judge that call again.
    fn remember_refusal(&self, canonical: &str, refused: bool) {
        let mut set = self
            .refused
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !refused {
            set.remove(canonical);
            return;
        }
        if set.len() >= REFUSED_CAP {
            set.clear();
        }
        set.insert(canonical.to_owned());
    }

    fn answered(&self, canonical: &str) -> bool {
        self.refused
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(canonical)
    }

    fn run_ask(
        &self,
        ask: &PermissionAsk<'_>,
        tool_call_id: &str,
        rule_kind: RuleKind,
        canonical: &str,
        display: &str,
        late: Option<Late<'_>>,
    ) -> CallOutcome {
        let rendered = ask.text();
        let _ = self.events.send(AgentEvent::PermissionRequested {
            tool_call_id: tool_call_id.to_owned(),
            title: ask.title.to_owned(),
            description: rendered.clone(),
        });
        let outcome = match &self.asker {
            Some(_) => {
                let answered = late.and_then(|late| self.late(ask, &late));
                let answered = self.answered_or_person(ask, answered);
                let (outcome, safe) = self.settle_answered(tool_call_id, ask, answered);
                if let Some(safe) = safe {
                    return CallOutcome {
                        allowed: true,
                        reason: format!(
                            "allowed by the classifier once no one answered (P(safe) {safe:.2})"
                        ),
                        containment: Containment::Uncontained,
                    };
                }
                outcome
            }
            None => {
                self.settle(tool_call_id, ask, false, Answerer::Nobody);
                let asked = yi_permission::Decision::Ask {
                    title: ask.title.to_owned(),
                    description: rendered,
                    reviewable: false,
                };
                return CallOutcome {
                    allowed: false,
                    containment: Containment::Uncontained,
                    reason: crate::gate::Report::reason_of(&crate::gate::compile_ask(asked, false)),
                };
            }
        };
        let allowed = matches!(outcome, AskOutcome::AllowOnce | AskOutcome::AllowAlways(_));
        self.remember_refusal(canonical, !allowed);
        if let AskOutcome::AllowAlways(index) = outcome {
            self.keep_grant(ask.grants.get(index), rule_kind, canonical, display);
        }
        if allowed {
            CallOutcome {
                allowed: true,
                reason: "allowed by user".to_owned(),
                containment: Containment::Uncontained,
            }
        } else {
            CallOutcome {
                allowed: false,
                reason: format!("The user denied this call. {rendered}"),
                containment: Containment::Uncontained,
            }
        }
    }

    fn answered_or_person(&self, ask: &PermissionAsk<'_>, answered: Option<Answered>) -> Answered {
        answered.unwrap_or_else(|| {
            let outcome = self.asker.as_ref().map_or(AskOutcome::Reject, |a| a(ask));
            Answered::Person {
                outcome,
                stopped: stop_released(ask),
            }
        })
    }
}
