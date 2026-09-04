#![forbid(unsafe_code)]
#![deny(clippy::string_slice)]

mod catastrophic;
mod decide;
mod review;
mod rules;
mod safety;

pub use catastrophic::{
    CatastrophicContext, command_reads_credentials, is_catastrophic, lexical_normalize,
};
pub use decide::{
    Decision, Hold, HoldSource, ParseOutcome, PermissionMode, ToolCall, decide, mode_fragment,
    parse_command,
};
pub use review::{
    ActionId, ActionLedger, ActionState, LEDGER_CAP, RequestId, ReviewedAsk, UserVerdict,
};
pub use rules::{
    ConfigRule, ConfigRuleAction, RuleStateError, SessionRules, canonical_command_identity,
    canonical_tool_identity, glob_matches,
};
pub use safety::{Class, Parsed, Verdict, classify, parse, verdict};
