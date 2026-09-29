#![forbid(unsafe_code)]
#![deny(clippy::string_slice)]

mod catastrophic;
mod decide;
mod review;
mod rules;
mod safety;

pub use catastrophic::{
    CatastrophicContext, command_reads_credentials, git_dirs, is_catastrophic, lexical_normalize,
    resolve_target, wraps,
};
pub use decide::{
    Decision, Hold, HoldPattern, HoldSource, ParseOutcome, PermissionMode, ToolCall, decide,
    mode_fragment, parse_command,
};
pub use review::{
    ActionId, ActionLedger, ActionState, LEDGER_CAP, RequestId, ReviewedAsk, UserVerdict,
};
pub use rules::{
    ConfigRule, ConfigRuleAction, Grant, PathGlob, RuleStateError, SessionRules,
    canonical_command_identity, canonical_tool_identity, grants,
};
pub use safety::{Class, Parsed, Verdict, classify, parse, refused_scopes, verdict, write_targets};
