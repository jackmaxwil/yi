use std::path::PathBuf;

use yi_types::permission::{RuleDecision, RuleKind};

use crate::catastrophic::{CatastrophicContext, command_targets_catastrophic, is_catastrophic};
use crate::rules::{ConfigRule, ConfigRuleAction, SessionRules};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionMode {
    Ask,
    Auto,
    Yolo,
}

/// One short prompt fragment per mode, selected into the system prompt
/// (design M11): a model that is not told the policy retries denied ops.
pub fn mode_fragment(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Ask => {
            "Permission mode: ask. Read-only tools run freely; every write or command asks the user first. A denied call will not succeed on retry — change approach or ask the user."
        }
        PermissionMode::Auto => {
            "Permission mode: auto. Read-only tools run freely; writes and commands run when a rule allows them and ask otherwise. A denied call will not succeed on retry."
        }
        PermissionMode::Yolo => {
            "Permission mode: yolo. Tools run without prompts, except catastrophic targets (system paths, home directory, the workspace .git), which are always denied."
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldSource {
    Advisor,
    User,
}

/// Soft-block (design M5): a matching call becomes Ask with the hold's
/// reason attached; an expired hold is inert.
#[derive(Debug, Clone)]
pub struct Hold {
    pub pattern: String,
    pub reason: String,
    pub source: HoldSource,
    pub expires_at_ms: Option<u64>,
}

impl Hold {
    pub fn expired(&self, now_ms: u64) -> bool {
        self.expires_at_ms.is_some_and(|at| at <= now_ms)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow { reason: String },
    Deny { reason: String },
    Ask { title: String, description: String },
}

/// Bash commands are decided whole in v1 (D35): an unparseable command is a
/// distinct decision input, never re-keyed onto plain `bash` (D26).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseOutcome {
    Parsed(Vec<String>),
    Unparsed,
}

const SHELL_METACHARS: [char; 8] = ['|', '&', ';', '>', '<', '`', '$', '\n'];

pub fn parse_command(command: &str) -> ParseOutcome {
    if command.contains(SHELL_METACHARS) || command.contains("(") {
        return ParseOutcome::Unparsed;
    }
    ParseOutcome::Parsed(vec![command.trim().to_owned()])
}

pub struct ToolCall<'a> {
    pub tool_name: &'a str,
    pub reads_only: bool,
    pub irreversible: bool,
    pub rule_kind: RuleKind,
    pub canonical: &'a str,
    pub display: &'a str,
    pub targets: &'a [PathBuf],
    pub command: Option<&'a str>,
}

/// Invariant: catastrophic denylist (every mode, yolo included) > configured
/// deny > session rule > configured allow/ask > hold > mode fallback.
// ponytail: no (mode, rules_hash) memoization — add it when decide() profiles.
pub fn decide(
    call: &ToolCall<'_>,
    mode: PermissionMode,
    config_rules: &[ConfigRule],
    session_rules: &SessionRules,
    holds: &[Hold],
    catastrophic_context: &CatastrophicContext,
) -> Decision {
    for target in call.targets {
        if is_catastrophic(target, catastrophic_context) {
            return Decision::Deny {
                reason: format!(
                    "{} targets a protected path ({}); this is denied in every mode.",
                    call.tool_name,
                    target.display()
                ),
            };
        }
    }
    if let Some(command) = call.command
        && let Some(hit) = command_targets_catastrophic(command, catastrophic_context)
    {
        return Decision::Deny {
            reason: format!(
                "command targets a protected path ({hit}); this is denied in every mode."
            ),
        };
    }

    let subject = call.command.unwrap_or(call.display);
    if let Some(rule) = config_rules
        .iter()
        .find(|rule| rule.action == ConfigRuleAction::Deny && rule.matches(call.tool_name, subject))
    {
        return Decision::Deny {
            reason: format!(
                "denied by configured rule `{}` for {}: {}",
                rule.pattern.glob(),
                call.tool_name,
                call.display
            ),
        };
    }

    match session_rules.decision_for(call.rule_kind, call.canonical) {
        Some(RuleDecision::Allow) => {
            return Decision::Allow {
                reason: "allowed by session rule".to_owned(),
            };
        }
        Some(RuleDecision::Deny) => {
            return Decision::Deny {
                reason: format!("denied by session rule for {}", call.display),
            };
        }
        None => {}
    }

    if let Some(rule) = config_rules
        .iter()
        .find(|rule| rule.action != ConfigRuleAction::Deny && rule.matches(call.tool_name, subject))
    {
        match rule.action {
            ConfigRuleAction::Allow => {
                return Decision::Allow {
                    reason: format!("allowed by configured rule `{}`", rule.pattern.glob()),
                };
            }
            ConfigRuleAction::Ask => {
                return ask(call, "a configured rule requires confirmation");
            }
            ConfigRuleAction::Deny => {}
        }
    }

    if let Some(hold) = holds
        .iter()
        .find(|hold| hold.pattern == call.tool_name || subject.contains(hold.pattern.as_str()))
    {
        return ask(call, &hold.reason);
    }

    match mode {
        PermissionMode::Yolo => Decision::Allow {
            reason: "allowed by yolo mode".to_owned(),
        },
        PermissionMode::Ask | PermissionMode::Auto => {
            if call.reads_only && !call.irreversible {
                Decision::Allow {
                    reason: "read-only invocation allowed".to_owned(),
                }
            } else {
                ask(call, "tool invocation is not read-only")
            }
        }
    }
}

fn ask(call: &ToolCall<'_>, reason: &str) -> Decision {
    Decision::Ask {
        title: format!("{} requires permission", call.tool_name),
        description: format!("{reason}: {}", call.display),
    }
}
