#![forbid(unsafe_code)]
#![deny(clippy::string_slice)]

mod browser;
mod catastrophic;
mod decide;
mod desktop;
pub mod outbox;
mod review;
mod rules;
mod safety;
mod scope;

pub use browser::{VERSION as BROWSER_VERSION, WEB as BROWSER_WEB, boxed as browser_boxed};
pub use catastrophic::{
    CD_DIRS_MAX, CatastrophicContext, Links, OpenHold, ReadGate, beneath, cd_target,
    command_reads_alias, command_reads_credentials, credential_stores, denied_file, file_id,
    git_dirs, identities, is_catastrophic, lexical_normalize, lexically_beneath, links,
    read_is_catastrophic, resolve_links, resolve_target, trunk_of, wraps,
};
pub use decide::{
    Decision, Hold, HoldPattern, HoldSource, PermissionMode, ToolCall, decide, mode_fragment,
};
pub use desktop::{drives, source};
pub use review::{
    ActionId, ActionLedger, ActionState, LEDGER_CAP, RequestId, ReviewedAsk, UserVerdict,
};
pub use rules::{
    ConfigRule, ConfigRuleAction, Grant, PathGlob, RuleStateError, SessionRules,
    canonical_command_identity, canonical_tool_identity, egress_grant, grants, is_exact_command,
    write_grant, write_grant_dir,
};
pub use safety::{
    Class, Parsed, Verdict, classify, command_segments, host_need, parse, refused_scopes, verdict,
    verdict_of, without_redirects, write_targets,
};
pub use scope::write_scope;
