//! Contracts (plan section 6.1): the item list a todo is verified against, the token that names
//! one verification, the verdict it produces, and the pure aggregation of section 6.2.

use std::num::NonZeroU64;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::canonical::{ArtifactRef, CanonicalError, Digest, canonical_digest};
use super::doc::{PlanId, TodoLabel};
use super::ledger::{AttemptId, IdError};
use crate::SECRET_NAME_MARKS;
use crate::plan::PlanVersion;

/// Items per contract; sixteen is the bound the aggregation is reasoned over.
pub const ITEMS_MAX: usize = 16;

/// Refused verdicts on one todo before it steps to `Blocked { on: User }`.
pub const DONE_REFUSAL_CAP: u32 = 3;

/// Juries convened on one todo per plan version; the next judged item escalates to the user.
pub const JUDGE_CAP_PER_TODO: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContractError {
    NoItems,
    TooManyItems {
        count: usize,
        max: usize,
    },
    DuplicateItem {
        id: ItemId,
    },
    JurySize {
        id: ItemId,
        n: u8,
    },
    ServiceUnavailable,
    Floor {
        class: ContractClass,
        need: &'static str,
    },
    UnknownItem {
        id: ItemId,
    },
    MissingItem {
        id: ItemId,
    },
    DuplicateVerdict {
        id: ItemId,
    },
}

impl std::fmt::Display for ContractError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoItems => write!(formatter, "contract declares no items"),
            Self::TooManyItems { count, max } => {
                write!(
                    formatter,
                    "contract declares {count} items; the cap is {max}"
                )
            }
            Self::DuplicateItem { id } => write!(formatter, "contract item {id} is declared twice"),
            Self::JurySize { id, n } => write!(
                formatter,
                "contract item {id} asks a jury of {n}; a jury is 1 (single-judge) or 3 (two votes decide)"
            ),
            Self::ServiceUnavailable => write!(
                formatter,
                "contract class service is refused at declaration until services land"
            ),
            Self::Floor { class, need } => {
                write!(formatter, "contract class {class} needs {need}")
            }
            Self::UnknownItem { id } => write!(formatter, "verdict names unknown item {id}"),
            Self::MissingItem { id } => write!(formatter, "verdict carries no line for item {id}"),
            Self::DuplicateVerdict { id } => {
                write!(formatter, "verdict carries two lines for item {id}")
            }
        }
    }
}

impl std::error::Error for ContractError {}

/// An item weight, `1..=100`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct Weight(u16);

impl Weight {
    pub const MAX: u16 = 100;

    /// # Errors
    /// Zero or over [`Weight::MAX`].
    pub fn new(value: u16) -> Result<Self, IdError> {
        if value == 0 {
            return Err(IdError::Zero { what: "weight" });
        }
        if value > Self::MAX {
            return Err(IdError::Overflow { what: "weight" });
        }
        Ok(Self(value))
    }

    pub fn get(self) -> u16 {
        self.0
    }
}

impl TryFrom<u16> for Weight {
    type Error = IdError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<Weight> for u16 {
    fn from(weight: Weight) -> Self {
        weight.0
    }
}

/// A threshold in thousandths, `1..=1000`; scores and coverage are reported as plain `u16`
/// permille because zero is a legal result and never a legal policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct Permille(u16);

impl Permille {
    pub const FULL: Self = Self(1000);

    /// # Errors
    /// Zero or over 1000.
    pub fn new(value: u16) -> Result<Self, IdError> {
        if value == 0 {
            return Err(IdError::Zero { what: "permille" });
        }
        if value > 1000 {
            return Err(IdError::Overflow { what: "permille" });
        }
        Ok(Self(value))
    }

    pub fn get(self) -> u16 {
        self.0
    }

    fn full() -> Self {
        Self::FULL
    }
}

impl Default for Permille {
    fn default() -> Self {
        Self::FULL
    }
}

impl TryFrom<u16> for Permille {
    type Error = IdError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<Permille> for u16 {
    fn from(permille: Permille) -> Self {
        permille.0
    }
}

text_id!(ItemId, "item id");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContractClass {
    Writer,
    Reader,
    Inline,
    Service,
}

impl std::fmt::Display for ContractClass {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Writer => "writer",
            Self::Reader => "reader",
            Self::Inline => "inline",
            Self::Service => "service",
        })
    }
}

/// The floor a class must clear at declaration (plan section 6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Floor {
    /// At least one critical `cmd` or `example` item.
    Behavioral,
    /// At least one critical `schema` item.
    Shape,
    /// Either floor: an inline todo declares its own role by what it can prove.
    Either,
    /// At least one critical `cmd` item; F3c owns the shutdown contract.
    Health,
}

pub fn floor_of(class: ContractClass) -> Floor {
    match class {
        ContractClass::Writer => Floor::Behavioral,
        ContractClass::Reader => Floor::Shape,
        ContractClass::Inline => Floor::Either,
        ContractClass::Service => Floor::Health,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct JuryPolicy {
    pub n: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decider {
    /// A frozen checker manifest: command, cwd policy, protected files.
    Cmd {
        checker: ArtifactRef,
        timeout_ms: u64,
    },
    /// Validates this attempt's output artifact.
    Schema { schema: ArtifactRef },
    Example {
        cases: ArtifactRef,
        runner: ArtifactRef,
        timeout_ms: u64,
    },
    /// A jury of walled readers of another model family (plan section 6.4).
    Judge {
        rubric: ArtifactRef,
        evidence: Vec<ArtifactRef>,
        policy: JuryPolicy,
    },
}

impl Decider {
    /// The artifacts a decider freezes, in declaration order.
    pub fn criteria(&self) -> Vec<&ArtifactRef> {
        match self {
            Self::Cmd { checker, .. } => vec![checker],
            Self::Schema { schema } => vec![schema],
            Self::Example { cases, runner, .. } => vec![cases, runner],
            Self::Judge {
                rubric, evidence, ..
            } => std::iter::once(rubric).chain(evidence).collect(),
        }
    }

    fn is_behavioral(&self) -> bool {
        matches!(self, Self::Cmd { .. } | Self::Example { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractItem {
    pub id: ItemId,
    pub critical: bool,
    pub weight: Weight,
    pub decider: Decider,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contract {
    pub class: ContractClass,
    pub items: Vec<ContractItem>,
    #[serde(default = "Permille::full")]
    pub threshold: Permille,
    #[serde(default = "Permille::full")]
    pub min_coverage: Permille,
    /// Globs relative to the checkout; a write to a covered path previews the `cmd` items.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub covers: Vec<String>,
}

impl Contract {
    /// # Errors
    /// No items, too many, a duplicate id, an undeclared jury size, a service, a floor unmet.
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.items.is_empty() {
            return Err(ContractError::NoItems);
        }
        if self.items.len() > ITEMS_MAX {
            return Err(ContractError::TooManyItems {
                count: self.items.len(),
                max: ITEMS_MAX,
            });
        }
        let mut seen: Vec<&ItemId> = Vec::with_capacity(self.items.len());
        for item in &self.items {
            if seen.contains(&&item.id) {
                return Err(ContractError::DuplicateItem {
                    id: item.id.clone(),
                });
            }
            seen.push(&item.id);
            // A quorum is predeclared per size. A judge never meets a floor below: alone, it fails.
            if let Decider::Judge { policy, .. } = &item.decider
                && !matches!(policy.n, 1 | 3)
            {
                return Err(ContractError::JurySize {
                    id: item.id.clone(),
                    n: policy.n,
                });
            }
        }
        if self.class == ContractClass::Service {
            return Err(ContractError::ServiceUnavailable);
        }
        let critical = self.items.iter().filter(|item| item.critical);
        let behavioral = critical.clone().any(|item| item.decider.is_behavioral());
        let shape = critical
            .clone()
            .any(|item| matches!(item.decider, Decider::Schema { .. }));
        let (met, need) = match floor_of(self.class) {
            Floor::Behavioral => (behavioral, "at least one critical cmd or example item"),
            Floor::Shape => (shape, "at least one critical schema item"),
            Floor::Either => (
                behavioral || shape,
                "at least one critical cmd, example or schema item",
            ),
            Floor::Health => (
                critical
                    .clone()
                    .any(|item| matches!(item.decider, Decider::Cmd { .. })),
                "at least one critical health cmd item",
            ),
        };
        if met {
            Ok(())
        } else {
            Err(ContractError::Floor {
                class: self.class,
                need,
            })
        }
    }

    /// # Errors
    /// The contract did not serialize into the token's `contract_digest`.
    pub fn digest(&self) -> Result<Digest, CanonicalError> {
        let value = serde_json::to_value(self).map_err(|error| CanonicalError::Serialize {
            detail: error.to_string(),
        })?;
        canonical_digest(&value)
    }

    /// Every frozen artifact, in item order.
    pub fn criteria(&self) -> Vec<&ArtifactRef> {
        self.items
            .iter()
            .flat_map(|item| item.decider.criteria())
            .collect()
    }

    /// # Errors
    /// The criteria list did not serialize into the token's `criteria_digest`.
    pub fn criteria_digest(&self) -> Result<Digest, CanonicalError> {
        let digests: Vec<String> = self
            .criteria()
            .iter()
            .map(|artifact| artifact.digest.to_string())
            .collect();
        canonical_digest(&serde_json::Value::from(digests))
    }

    /// # Errors
    /// The lines do not name the items exactly once each.
    pub fn pair(
        &self,
        lines: &[ItemLine],
    ) -> Result<Vec<(ContractItem, ItemVerdict)>, ContractError> {
        for line in lines {
            if !self.items.iter().any(|item| item.id == line.id) {
                return Err(ContractError::UnknownItem {
                    id: line.id.clone(),
                });
            }
        }
        self.items
            .iter()
            .map(|item| {
                let mut found = lines.iter().filter(|line| line.id == item.id);
                let first = found.next().ok_or_else(|| ContractError::MissingItem {
                    id: item.id.clone(),
                })?;
                if found.next().is_some() {
                    return Err(ContractError::DuplicateVerdict {
                        id: item.id.clone(),
                    });
                }
                Ok((item.clone(), first.verdict.clone()))
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemVerdict {
    Pass,
    Fail { detail: String },
    Abstain { reason: String },
    Escalate { question: String },
}

impl std::fmt::Display for ItemVerdict {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pass => formatter.write_str("pass"),
            Self::Fail { detail } => write!(formatter, "fail: {detail}"),
            Self::Abstain { reason } => write!(formatter, "abstain: {reason}"),
            Self::Escalate { question } => write!(formatter, "escalate: {question}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Pass,
    Fail,
    Abstain,
    Escalate,
}

impl Outcome {
    /// The order the refusal path reads: a critical failure is the worst news.
    pub fn rank(self) -> u8 {
        match self {
            Self::Fail => 0,
            Self::Abstain => 1,
            Self::Escalate => 2,
            Self::Pass => 3,
        }
    }
}

impl std::fmt::Display for Outcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Abstain => "abstain",
            Self::Escalate => "escalate",
        })
    }
}

/// The result of [`aggregate`]: the outcome and the two permille it was decided on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Aggregate {
    pub outcome: Outcome,
    pub score: u16,
    pub coverage: u16,
}

fn permille_of(part: u64, whole: NonZeroU64) -> Option<u16> {
    let scaled = part.checked_mul(1000)?;
    u16::try_from(scaled / whole).ok()
}

/// Plan section 6.2, rules 1 to 6 in order, over `u64` with checked arithmetic. Total and never
/// panics; an arithmetic failure is an abstention, never a pass.
pub fn aggregate(
    items: &[(ContractItem, ItemVerdict)],
    threshold: Permille,
    min_coverage: Permille,
) -> Aggregate {
    let abstain = |score: u16, coverage: u16| Aggregate {
        outcome: Outcome::Abstain,
        score,
        coverage,
    };
    let mut total: u64 = 0;
    let mut decided: u64 = 0;
    let mut passed: u64 = 0;
    for (item, _) in items {
        total = total.saturating_add(u64::from(item.weight.get()));
    }
    for (item, verdict) in items {
        let weight = u64::from(item.weight.get());
        match verdict {
            ItemVerdict::Pass => {
                decided = decided.saturating_add(weight);
                passed = passed.saturating_add(weight);
            }
            ItemVerdict::Fail { .. } => decided = decided.saturating_add(weight),
            ItemVerdict::Abstain { .. } | ItemVerdict::Escalate { .. } => {}
        }
    }
    let coverage = NonZeroU64::new(total)
        .and_then(|whole| permille_of(decided, whole))
        .unwrap_or(0);
    let score = NonZeroU64::new(decided)
        .and_then(|whole| permille_of(passed, whole))
        .unwrap_or(0);
    // Rule 1.
    if items
        .iter()
        .any(|(item, verdict)| item.critical && matches!(verdict, ItemVerdict::Fail { .. }))
    {
        return Aggregate {
            outcome: Outcome::Fail,
            score,
            coverage,
        };
    }
    // Rule 2.
    if items
        .iter()
        .any(|(_, verdict)| matches!(verdict, ItemVerdict::Escalate { .. }))
    {
        return Aggregate {
            outcome: Outcome::Escalate,
            score,
            coverage,
        };
    }
    // Rule 3.
    if items
        .iter()
        .any(|(item, verdict)| item.critical && matches!(verdict, ItemVerdict::Abstain { .. }))
    {
        return abstain(score, coverage);
    }
    // Rule 4.
    let Some(whole) = NonZeroU64::new(total) else {
        return abstain(0, 0);
    };
    let Some(decided_nz) = NonZeroU64::new(decided) else {
        return abstain(0, coverage);
    };
    // Rule 5.
    let Some(coverage) = permille_of(decided, whole) else {
        return abstain(score, 0);
    };
    if coverage < min_coverage.get() {
        return abstain(score, coverage);
    }
    // Rule 6.
    let Some(score) = permille_of(passed, decided_nz) else {
        return abstain(0, coverage);
    };
    Aggregate {
        outcome: if score >= threshold.get() {
            Outcome::Pass
        } else {
            Outcome::Fail
        },
        score,
        coverage,
    }
}

/// One verification, named whole: a change to any part makes a verdict stale.
#[must_use]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationToken {
    pub plan: PlanId,
    pub version: PlanVersion,
    pub todo: TodoLabel,
    pub attempt: AttemptId,
    pub contract_digest: Digest,
    pub criteria_digest: Digest,
    pub output_digest: Digest,
    /// The shadow-gitdir tree id, or the snapshotter's own id for the workspace.
    pub snapshot: String,
    pub integration: Option<u64>,
}

impl VerificationToken {
    /// # Errors
    /// The token did not serialize.
    pub fn digest(&self) -> Result<Digest, CanonicalError> {
        let value = serde_json::to_value(self).map_err(|error| CanonicalError::Serialize {
            detail: error.to_string(),
        })?;
        canonical_digest(&value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Vote {
    Pass,
    Fail,
    Abstain,
}

/// The one answer a juror may give (plan section 6.4), parsed strictly: an unknown key, a
/// missing one or a fourth verdict word is not an answer.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JurorAnswer {
    pub verdict: Vote,
    pub reason: String,
    pub quotes: Vec<Quote>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quote {
    pub url: String,
    pub line: usize,
    pub text: String,
}

/// One juror of a judged item: the model the host seated, its vote and its stated reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JurorLine {
    pub model: String,
    pub vote: Vote,
    pub reason: String,
    /// Set by the quote check, never read off `reason`, which is the juror's own text.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unbacked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemLine {
    pub id: ItemId,
    pub verdict: ItemVerdict,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jurors: Vec<JurorLine>,
    /// A passing command's line and the tail of what it printed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
}

#[must_use]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    pub token: VerificationToken,
    pub outcome: Outcome,
    pub score: u16,
    pub coverage: u16,
    pub items: Vec<ItemLine>,
    /// False when a checker declared inputs the snapshot does not pin.
    #[serde(default = "reproducible_default")]
    pub reproducible: bool,
    pub elapsed_ms: u64,
    pub at: u64,
}

fn reproducible_default() -> bool {
    true
}

impl Verdict {
    /// Every item on its own line, the shape a refusal returns.
    pub fn lines(&self) -> String {
        self.items
            .iter()
            .map(|line| format!("- {}: {}", line.id, line.verdict))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// How a `Done` came to be: verified by the kernel, accepted by the user, or carried in from a
/// format-1 file whose success nobody checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    VerifiedDone,
    AcceptedByUser,
    LegacyUnverified,
}

impl std::fmt::Display for Resolution {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::VerifiedDone => "verified_done",
            Self::AcceptedByUser => "accepted_by_user",
            Self::LegacyUnverified => "legacy_unverified",
        })
    }
}

/// The checker manifest format this tree reads; bumped, never reinterpreted.
pub const MANIFEST_FORMAT: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cwd {
    SnapshotRoot,
    SnapshotSubdir,
}

/// A frozen checker manifest (`fixtures/plans/contracts/contracts.md`): every field required,
/// unknown keys refused, names never values.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckerManifest {
    pub manifest: u32,
    pub command: String,
    pub cwd: Cwd,
    pub cwd_subdir: Option<String>,
    pub protected: Vec<String>,
    pub timeout_ms: u64,
    pub env: Vec<String>,
    pub reads_outside_snapshot: bool,
}

impl CheckerManifest {
    /// # Errors
    /// Not the format, a bad cwd policy or subdir, or an environment name this session refuses.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let manifest: Self =
            serde_json::from_slice(bytes).map_err(|error| format!("checker manifest: {error}"))?;
        if manifest.manifest != MANIFEST_FORMAT {
            return Err(format!(
                "checker manifest format {} is not {MANIFEST_FORMAT}",
                manifest.manifest
            ));
        }
        if manifest.command.trim().is_empty() {
            return Err("checker manifest names no command".to_owned());
        }
        match (manifest.cwd, &manifest.cwd_subdir) {
            (Cwd::SnapshotRoot, Some(_)) => {
                return Err("checker manifest: cwd_subdir is set with cwd snapshot_root".to_owned());
            }
            (Cwd::SnapshotSubdir, None) => {
                return Err("checker manifest: cwd snapshot_subdir needs cwd_subdir".to_owned());
            }
            (Cwd::SnapshotSubdir, Some(subdir)) => {
                let path = std::path::Path::new(subdir);
                let escapes = path.is_absolute()
                    || path
                        .components()
                        .any(|part| !matches!(part, std::path::Component::Normal(_)));
                if escapes || subdir.is_empty() {
                    return Err(format!(
                        "checker manifest: cwd_subdir {subdir:?} is not a relative path inside the snapshot"
                    ));
                }
            }
            (Cwd::SnapshotRoot, None) => {}
        }
        for name in &manifest.env {
            let upper = name.to_ascii_uppercase();
            if name.is_empty() || name.contains('=') || name.chars().any(char::is_whitespace) {
                return Err(format!(
                    "checker manifest: {name:?} is not an environment name"
                ));
            }
            // A stored criterion can be read back, so a name merely holding a mark is refused.
            if SECRET_NAME_MARKS.iter().any(|mark| upper.contains(mark)) {
                return Err(format!(
                    "checker manifest: {name} looks like a secret; a criterion may not read one"
                ));
            }
            if std::env::var_os(name).is_none() {
                return Err(format!(
                    "checker manifest: this session holds no {name}; a criterion cannot ask for more than the session has"
                ));
            }
        }
        Ok(manifest)
    }

    pub fn workdir(&self, root: &std::path::Path) -> std::path::PathBuf {
        match &self.cwd_subdir {
            Some(subdir) => root.join(subdir),
            None => root.to_path_buf(),
        }
    }
}

/// One example case: the runner reads `input` on stdin and must print `expected`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub input: Value,
    pub expected: Value,
}
