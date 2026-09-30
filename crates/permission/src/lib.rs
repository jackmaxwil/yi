#![forbid(unsafe_code)]
#![deny(clippy::string_slice)]

mod catastrophic;
mod decide;
mod review;
mod rules;
mod safety;

pub use catastrophic::{
    CatastrophicContext, ReadGate, beneath, command_reads_credentials, credential_stores,
    denied_file, git_dirs, identities, is_catastrophic, lexical_normalize, lexically_beneath,
    read_is_catastrophic, resolve_links, resolve_target, wraps,
};
pub use decide::{
    Decision, Hold, HoldPattern, HoldSource, PermissionMode, ToolCall, decide, mode_fragment,
};
pub use review::{
    ActionId, ActionLedger, ActionState, LEDGER_CAP, RequestId, ReviewedAsk, UserVerdict,
};
pub use rules::{
    ConfigRule, ConfigRuleAction, Grant, PathGlob, RuleStateError, SessionRules,
    canonical_command_identity, canonical_tool_identity, grants, write_grant,
};
pub use safety::{
    Class, Parsed, Verdict, classify, command_segments, host_need, needs_host, parse,
    refused_scopes, verdict, write_targets,
};
