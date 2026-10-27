use std::path::PathBuf;

use yi_permission::{ConfigRule, Decision, Hold, PermissionMode, SessionRules, ToolCall, decide};
use yi_types::event::AgentEvent;
use yi_types::permission::Answerer;

use super::{CallOutcome, Containment, PermissionAsk, PermissionBroker};
use crate::claude_hooks::Verdict;

pub(super) struct Judged<'a> {
    pub(super) call: &'a ToolCall<'a>,
    pub(super) config_rules: &'a [ConfigRule],
    pub(super) session_rules: &'a SessionRules,
    pub(super) holds: &'a [Hold],
}

/// With hooks on, what is written under the project's `.claude` runs on the host as a hook.
pub(crate) fn claude_dir(broker: Option<&PermissionBroker>) -> Option<PathBuf> {
    broker?.claude_hooks().map(|hooks| hooks.protected())
}

impl PermissionBroker {
    /// The decision a hook's verdict leaves, and whether a hook's allow may answer its ask: only
    /// the mode's own, never a rule's, a hold's, a credential read's or one under `.claude`.
    pub(super) fn hooked(
        &self,
        decision: Decision,
        hook: &Verdict,
        judged: &Judged<'_>,
    ) -> (Decision, bool) {
        let call = judged.call;
        let host_run = claude_dir(Some(self))
            .filter(|_| !call.reads_only)
            .filter(|dir| {
                (call.targets.iter())
                    .any(|path| yi_permission::lexically_beneath(std::slice::from_ref(dir), path))
            });
        let ask = |why: String| Decision::Ask {
            title: format!("{} requires permission", call.tool_name),
            description: format!("{why}: {}", call.display),
            reviewable: false,
        };
        let decision = match (host_run, decision) {
            (_, decision @ Decision::Deny { .. }) => return (decision, false),
            (Some(dir), _) => {
                let why = format!(
                    "it writes under {}, whose files Claude Code hooks run on the host",
                    dir.display()
                );
                return (ask(why), false);
            }
            (None, decision) => decision,
        };
        let mode_asks = match &decision {
            Decision::Ask { reviewable, .. } => *reviewable || self.mode() == PermissionMode::Ask,
            Decision::Contain { .. } => self.sandbox.is_none(),
            Decision::Allow { .. } | Decision::Deny { .. } => false,
        };
        let decision = match (hook, decision) {
            (Verdict::Ask(why), Decision::Allow { .. } | Decision::Contain { .. }) => {
                ask(format!("a Claude Code PreToolUse hook asks ({why})"))
            }
            (
                Verdict::Ask(why),
                Decision::Ask {
                    title, description, ..
                },
            ) => Decision::Ask {
                title,
                description: format!("{description}; a Claude Code PreToolUse hook asks ({why})"),
                reviewable: false,
            },
            (_, decision) => decision,
        };
        let context = &self.context;
        let (rules, session, holds) = (judged.config_rules, judged.session_rules, judged.holds);
        let answers = *hook == Verdict::Allow
            && mode_asks
            && !matches!(
                decide(call, PermissionMode::Yolo, rules, session, holds, context),
                Decision::Ask { .. }
            );
        (decision, answers)
    }

    /// A hook's allow answering the mode's ask: journaled as answered, contained where it can be.
    pub(super) fn hook_answered(
        &self,
        ask: &PermissionAsk<'_>,
        reason: &str,
        command: Option<&str>,
    ) -> CallOutcome {
        let tool_call_id = ask.tool_call_id.unwrap_or_default();
        let _ = self.events.send(AgentEvent::PermissionRequested {
            tool_call_id: tool_call_id.to_owned(),
            title: ask.title.to_owned(),
            description: ask.text(),
        });
        let by = Answerer::Other("claudeCodeHook".to_owned());
        self.settle(tool_call_id, ask, true, by);
        let containment = match command {
            Some(_) if self.sandbox.is_some() => self.containment_with(None::<PathBuf>, false),
            _ => Containment::Uncontained,
        };
        CallOutcome {
            allowed: true,
            reason: format!("allowed by a Claude Code PreToolUse hook ({reason})"),
            containment,
        }
    }
}
