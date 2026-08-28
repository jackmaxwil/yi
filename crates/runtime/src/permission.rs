use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};
use tokio::sync::broadcast;
use yi_permission::{
    CatastrophicContext, ConfigRule, Decision, Hold, PermissionMode, SessionRules, ToolCall,
    canonical_command_identity, canonical_tool_identity, decide,
};
use yi_tools::ToolKind;
use yi_types::event::AgentEvent;
use yi_types::permission::{RuleDecision, RuleKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskOutcome {
    AllowOnce,
    AllowAlways,
    Reject,
}

/// One approval request. `description` and `patch` are separate so a
/// structured consumer (ACP C7) sends the patch as content while a text one
/// renders `text()`, which folds the two the same way for everybody.
pub struct PermissionAsk<'a> {
    pub title: &'a str,
    pub description: &'a str,
    pub patch: Option<&'a str>,
    pub changes: &'a [PathBuf],
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

pub struct PermissionBroker {
    mode: Mutex<PermissionMode>,
    config_rules: Vec<ConfigRule>,
    session_rules: Mutex<SessionRules>,
    holds: Mutex<Vec<Hold>>,
    context: CatastrophicContext,
    cwd: PathBuf,
    asker: Option<Asker>,
    events: broadcast::Sender<AgentEvent>,
}

pub struct CallOutcome {
    pub allowed: bool,
    pub reason: String,
}

fn extract_targets(tool_name: &str, args: &Map<String, Value>, cwd: &Path) -> Vec<PathBuf> {
    let mut targets = Vec::new();
    let mut push = |raw: &str| {
        let candidate = PathBuf::from(raw);
        targets.push(if candidate.is_absolute() {
            candidate
        } else {
            cwd.join(candidate)
        });
    };
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
            mode: Mutex::new(mode),
            config_rules,
            session_rules: Mutex::new(SessionRules::new()),
            holds: Mutex::new(Vec::new()),
            context: CatastrophicContext::detect(&cwd),
            cwd,
            asker,
            events,
        }
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

    /// C8: a client may switch policy mid-session; the next decision reads it.
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
        let call = ToolCall {
            tool_name,
            reads_only: matches!(kind, ToolKind::Read),
            irreversible,
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
        match decision {
            Decision::Allow { reason } => CallOutcome {
                allowed: true,
                reason,
            },
            Decision::Deny { reason } => CallOutcome {
                allowed: false,
                reason,
            },
            Decision::Ask { title, description } => self.run_ask(
                &PermissionAsk {
                    title: &title,
                    description: &description,
                    patch: preview,
                    changes: &targets,
                },
                tool_call_id,
                rule_kind,
                &canonical,
                &display,
            ),
        }
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
                let _ = self.events.send(AgentEvent::PermissionResolved {
                    tool_call_id: tool_call_id.to_owned(),
                    allowed: false,
                });
                return CallOutcome {
                    allowed: false,
                    reason: format!(
                        "Permission required but no interactive surface is available. {rendered} Run with --yolo, or add an allow rule for this call."
                    ),
                };
            }
        };
        let allowed = matches!(outcome, AskOutcome::AllowOnce | AskOutcome::AllowAlways);
        let _ = self.events.send(AgentEvent::PermissionResolved {
            tool_call_id: tool_call_id.to_owned(),
            allowed,
        });
        if outcome == AskOutcome::AllowAlways {
            let mut session_rules = self
                .session_rules
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let _cap_is_soft =
                session_rules.insert(rule_kind, canonical, display, RuleDecision::Allow);
        }
        if allowed {
            CallOutcome {
                allowed: true,
                reason: "allowed by user".to_owned(),
            }
        } else {
            CallOutcome {
                allowed: false,
                reason: format!("The user denied this call. {rendered}"),
            }
        }
    }
}
