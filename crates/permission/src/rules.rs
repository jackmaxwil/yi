use sha2::{Digest, Sha256};
use thiserror::Error;
use yi_types::permission::{RuleDecision, RuleKind, SessionPermissionRule, SessionPermissionState};

pub const SCHEMA_VERSION: u8 = 2;
pub const MAX_RULES: usize = 1024;
pub const MAX_IDENTITY_BYTES: usize = 4096;

#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuleStateError {
    #[error("invalid permission state: {0}")]
    Invalid(&'static str),
    #[error("permission identity is empty or exceeds {MAX_IDENTITY_BYTES} bytes")]
    InvalidIdentity,
    #[error("session rule cap ({MAX_RULES}) reached")]
    CapReached,
}

fn digest(canonical: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    hasher.finalize().into()
}

fn write_identity_field(out: &mut String, field: &str) {
    out.push_str(&format!("{}:", field.len()));
    out.push_str(field);
    out.push('\n');
}

pub fn canonical_command_identity(command: &str, cwd: &str) -> String {
    let mut out = String::new();
    write_identity_field(&mut out, "yi-permission-state-v2");
    write_identity_field(&mut out, "command");
    write_identity_field(&mut out, command);
    write_identity_field(&mut out, cwd);
    out
}

pub fn canonical_tool_identity(tool_name: &str, arguments_json: &str) -> String {
    let mut out = String::new();
    write_identity_field(&mut out, "yi-permission-state-v2");
    write_identity_field(&mut out, tool_name);
    write_identity_field(&mut out, arguments_json);
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigRuleAction {
    Allow,
    Deny,
    Ask,
}

/// Configured pattern rule (design M2). Precedence over session rules is
/// enforced in decide(): configured deny > session rule > session grant.
#[derive(Debug, Clone)]
pub struct ConfigRule {
    pub tool: String,
    pub pattern: globset::GlobMatcher,
    pub action: ConfigRuleAction,
}

impl ConfigRule {
    pub fn new(tool: &str, pattern: &str, action: ConfigRuleAction) -> Result<Self, String> {
        let matcher = globset::GlobBuilder::new(pattern)
            .literal_separator(false)
            .build()
            .map_err(|error| format!("invalid permission pattern {pattern:?}: {error}"))?
            .compile_matcher();
        Ok(Self {
            tool: tool.to_owned(),
            pattern: matcher,
            action,
        })
    }

    pub fn matches(&self, tool_name: &str, subject: &str) -> bool {
        (self.tool == "*" || self.tool == tool_name) && self.pattern.is_match(subject)
    }
}

/// Compiled once and asked many times: a rule's `paths:` is parsed at discovery.
#[derive(Debug, Clone)]
pub struct PathGlob(globset::GlobMatcher);

impl PathGlob {
    pub fn new(pattern: &str) -> Result<Self, String> {
        Ok(Self(
            globset::GlobBuilder::new(pattern)
                .literal_separator(false)
                .build()
                .map_err(|error| format!("invalid permission pattern {pattern:?}: {error}"))?
                .compile_matcher(),
        ))
    }

    pub fn is_match(&self, subject: &str) -> bool {
        self.0.is_match(subject)
    }
}

/// Session rule state over the yi-types wire shape: digests are recomputed
/// from `canonical` on load and never trusted from disk.
#[derive(Default)]
pub struct SessionRules {
    state: SessionPermissionState,
    digests: Vec<[u8; 32]>,
}

impl SessionRules {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load(state: SessionPermissionState) -> Result<Self, RuleStateError> {
        validate(&state)?;
        let digests = state
            .rules
            .iter()
            .map(|rule| digest(&rule.canonical))
            .collect();
        Ok(Self { state, digests })
    }

    pub fn state(&self) -> &SessionPermissionState {
        &self.state
    }

    pub fn decision_for(&self, kind: RuleKind, canonical: &str) -> Option<RuleDecision> {
        let wanted = digest(canonical);
        self.state
            .rules
            .iter()
            .zip(&self.digests)
            .find(|(rule, rule_digest)| {
                rule.kind == kind && **rule_digest == wanted && rule.canonical == canonical
            })
            .map(|(rule, _)| rule.decision)
    }

    pub fn insert(
        &mut self,
        kind: RuleKind,
        canonical: &str,
        display_identity: &str,
        decision: RuleDecision,
    ) -> Result<(), RuleStateError> {
        if canonical.is_empty() || canonical.len() > MAX_IDENTITY_BYTES {
            return Err(RuleStateError::InvalidIdentity);
        }
        if let Some(position) = self
            .state
            .rules
            .iter()
            .position(|rule| rule.kind == kind && rule.canonical == canonical)
        {
            let generation = self.state.next_generation;
            self.state.next_generation = generation.saturating_add(1);
            if let Some(rule) = self.state.rules.get_mut(position) {
                rule.decision = decision;
                rule.generation = generation;
                rule.display_identity = display_identity.to_owned();
            }
            return Ok(());
        }
        if self.state.rules.len() >= MAX_RULES {
            return Err(RuleStateError::CapReached);
        }
        let id = self.state.next_generation;
        self.state.next_generation = id.saturating_add(1);
        self.state.rules.push(SessionPermissionRule {
            id,
            kind,
            canonical: canonical.to_owned(),
            display_identity: display_identity.to_owned(),
            decision,
            generation: id,
        });
        self.digests.push(digest(canonical));
        Ok(())
    }
}

fn validate(state: &SessionPermissionState) -> Result<(), RuleStateError> {
    if state.version != SCHEMA_VERSION {
        return Err(RuleStateError::Invalid("unsupported schema version"));
    }
    if state.next_generation == 0 {
        return Err(RuleStateError::Invalid("next_generation must be positive"));
    }
    if state.rules.len() > MAX_RULES {
        return Err(RuleStateError::Invalid("rule count exceeds cap"));
    }
    for (index, rule) in state.rules.iter().enumerate() {
        if rule.id == 0
            || rule.generation == 0
            || rule.id > rule.generation
            || rule.id >= state.next_generation
            || rule.generation >= state.next_generation
            || rule.canonical.is_empty()
            || rule.canonical.len() > MAX_IDENTITY_BYTES
            || rule.display_identity.is_empty()
        {
            return Err(RuleStateError::Invalid("rule field out of bounds"));
        }
        for prior in &state.rules[..index] {
            if prior.id == rule.id || (prior.kind == rule.kind && prior.canonical == rule.canonical)
            {
                return Err(RuleStateError::Invalid("duplicate rule id or key"));
            }
        }
    }
    Ok(())
}
