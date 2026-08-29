#![forbid(unsafe_code)]

mod catastrophic;
mod decide;
mod rules;
mod safety;

pub use catastrophic::{
    CatastrophicContext, command_reads_credentials, is_catastrophic, lexical_normalize,
};
pub use decide::{
    Decision, Hold, HoldSource, ParseOutcome, PermissionMode, ToolCall, decide, mode_fragment,
    parse_command,
};
pub use rules::{
    ConfigRule, ConfigRuleAction, RuleStateError, SessionRules, canonical_command_identity,
    canonical_tool_identity,
};
pub use safety::{Class, Parsed, Verdict, classify, parse, verdict};
