//! The spec: requirements quoted from the request, the artifacts the task names, and one
//! check whose TAP output names requirement ids, so state is parsed and never read as prose.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const SPEC_ENTRY_TYPE: &str = "spec";
pub const SPEC_RUN_ENTRY_TYPE: &str = "spec_run";
pub const SPEC_REFUTE_ENTRY_TYPE: &str = "spec_refute";
/// The refute must snapshot every artifact; a tree past this is declared smaller.
pub const ARTIFACT_MAX_BYTES: u64 = 32 * 1024 * 1024;
pub const CHECK_TIMEOUT_MIN: Duration = Duration::from_secs(60);
pub const CHECK_TIMEOUT_MAX: Duration = Duration::from_secs(600);
/// The check timeout is the baseline's duration times this, within the bounds above.
pub const CHECK_TIMEOUT_FACTOR: u32 = 4;

/// `R7`. The field is private; `TryFrom<&str>` is the only constructor and the proof.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ReqId(String);

impl ReqId {
    pub fn numbered(n: usize) -> Self {
        Self(format!("R{n}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The number after `R`, for ordering and the next id.
    pub fn number(&self) -> usize {
        self.0.get(1..).and_then(|n| n.parse().ok()).unwrap_or(0)
    }
}

impl TryFrom<&str> for ReqId {
    type Error = SpecError;

    fn try_from(text: &str) -> Result<Self, Self::Error> {
        let text = text.trim();
        let digits = text.strip_prefix('R').unwrap_or("");
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(SpecError::BadId(text.to_owned()));
        }
        Ok(Self(text.to_owned()))
    }
}

impl TryFrom<String> for ReqId {
    type Error = SpecError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::try_from(text.as_str())
    }
}

impl From<ReqId> for String {
    fn from(id: ReqId) -> Self {
        id.0
    }
}

impl std::fmt::Display for ReqId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// `f2p` must be red at baseline and green at the end; `p2p` green throughout; `metric`
/// carries a `# metric name=value` line and never blocks a stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    F2p,
    P2p,
    Metric,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::F2p => "f2p",
            Self::P2p => "p2p",
            Self::Metric => "metric",
        }
    }
}

impl TryFrom<&str> for Kind {
    type Error = SpecError;

    fn try_from(text: &str) -> Result<Self, Self::Error> {
        match text.trim() {
            "" | "f2p" => Ok(Self::F2p),
            "p2p" => Ok(Self::P2p),
            "metric" => Ok(Self::Metric),
            other => Err(SpecError::BadKind(other.to_owned())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Requirement {
    pub id: ReqId,
    pub text: String,
    #[serde(default)]
    pub kind: Kind,
}

/// A declared artifact, validated once: absolute after resolving against the cwd, never `/`,
/// the cwd, an ancestor of it, or a home directory, which is what lets the refute move it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Artifact(PathBuf);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "why")]
pub enum ArtifactRefused {
    Empty,
    Root,
    CwdOrAncestor,
    Home,
    TooLarge { bytes: u64, max: u64 },
}

impl Artifact {
    pub fn declare(raw: &str, cwd: &Path, home: Option<&Path>) -> Result<Self, ArtifactRefused> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(ArtifactRefused::Empty);
        }
        let path = normalize(&cwd.join(raw));
        if path.parent().is_none() {
            return Err(ArtifactRefused::Root);
        }
        let cwd = normalize(cwd);
        if cwd.starts_with(&path) {
            return Err(ArtifactRefused::CwdOrAncestor);
        }
        if let Some(home) = home
            && normalize(home) == path
        {
            return Err(ArtifactRefused::Home);
        }
        Ok(Self(path))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

/// Lexical `.` and `..` removal: the path may not exist yet, so `canonicalize` cannot.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The `sh -c` command; non-empty by construction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CheckCommand(String);

impl CheckCommand {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<&str> for CheckCommand {
    type Error = SpecError;

    fn try_from(text: &str) -> Result<Self, Self::Error> {
        let text = text.trim();
        if text.is_empty() {
            return Err(SpecError::EmptyCheck);
        }
        Ok(Self(text.to_owned()))
    }
}

impl TryFrom<String> for CheckCommand {
    type Error = SpecError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::try_from(text.as_str())
    }
}

impl From<CheckCommand> for String {
    fn from(command: CheckCommand) -> Self {
        command.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    pub requirements: Vec<Requirement>,
    pub artifacts: Vec<Artifact>,
    pub check: CheckCommand,
    /// The request's own count, the checksum freeze holds the requirements against.
    pub enumerated: usize,
}

/// A frozen spec carries what a draft cannot: the baseline run that typed it, the timeout
/// measured from that run, and where the artifacts' baseline copies live.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Frozen {
    pub draft: Draft,
    pub frozen_at: u64,
    pub baseline: Run,
    pub timeout: Duration,
    pub snapshot: PathBuf,
}

/// The document on disk. `run`, `add`, the refute and the stop rule exist only on `Frozen`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "state")]
pub enum Spec {
    Draft(Draft),
    Frozen(Box<Frozen>),
}

impl Spec {
    pub fn draft(&self) -> &Draft {
        match self {
            Self::Draft(draft) => draft,
            Self::Frozen(frozen) => &frozen.draft,
        }
    }

    pub fn frozen(&self) -> Option<&Frozen> {
        match self {
            Self::Draft(_) => None,
            Self::Frozen(frozen) => Some(frozen.as_ref()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Green,
    Red,
    /// No TAP line named the id.
    Missing,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Green => "green",
            Self::Red => "red",
            Self::Missing => "missing",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mutant {
    Baseline,
    Empty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "kind", content = "mutant")]
pub enum RunKind {
    Baseline,
    Run,
    Refute(Mutant),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "exit", content = "code")]
pub enum Exit {
    Code(i32),
    Signal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "cause")]
pub enum NotRun {
    /// The io error rendered at the persistence boundary: the record is serde, the error is not.
    Spawn {
        error: String,
    },
    Timeout {
        after: Duration,
    },
    NoTap,
}

/// A finite number; NaN and the infinities never enter a `best` comparison.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "f64", into = "f64")]
pub struct Finite(f64);

impl Finite {
    pub fn get(self) -> f64 {
        self.0
    }
}

impl TryFrom<f64> for Finite {
    type Error = SpecError;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        if value.is_finite() {
            Ok(Self(value))
        } else {
            Err(SpecError::NotFinite(value))
        }
    }
}

impl From<Finite> for f64 {
    fn from(value: Finite) -> Self {
        value.0
    }
}

/// A run either ran or did not; a status map exists only when it ran.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "outcome")]
pub enum Outcome {
    Ran {
        exit: Exit,
        status: BTreeMap<ReqId, Status>,
        diag: BTreeMap<ReqId, String>,
        metrics: BTreeMap<String, Finite>,
        unbound: u32,
    },
    NotRun(NotRun),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub at: u64,
    pub turn: u32,
    pub kind: RunKind,
    pub duration: Duration,
    pub tail: String,
    pub outcome: Outcome,
    /// Check literals found verbatim in an artifact: typed, not computed (D170).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub literal_overlaps: Vec<String>,
}

impl Run {
    #[must_use]
    pub fn status(&self, id: &ReqId) -> Status {
        match &self.outcome {
            Outcome::Ran { status, .. } => status.get(id).copied().unwrap_or(Status::Missing),
            Outcome::NotRun(_) => Status::Missing,
        }
    }

    #[must_use]
    pub fn diag(&self, id: &ReqId) -> Option<&str> {
        match &self.outcome {
            Outcome::Ran { diag, .. } => diag.get(id).map(String::as_str),
            Outcome::NotRun(_) => None,
        }
    }

    pub fn ran(&self) -> bool {
        matches!(self.outcome, Outcome::Ran { .. })
    }
}

/// What the refute proved against; a different key reruns it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefuteKey {
    pub command: String,
    pub check_hash: Option<String>,
    pub artifacts: Vec<Artifact>,
}

/// The `f2p` ids that stayed green under each mutant; both empty is a pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefuteRecord {
    pub at: u64,
    pub key: RefuteKey,
    pub baseline_green: Vec<ReqId>,
    pub empty_green: Vec<ReqId>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl RefuteRecord {
    pub fn passed(&self) -> bool {
        self.baseline_green.is_empty() && self.empty_green.is_empty()
    }
}

/// Every failing fact of a freeze, as values; the tool renders them at its boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "fact")]
pub enum FreezeFact {
    GreenAtBaseline { ids: Vec<ReqId> },
    RedAtBaseline { ids: Vec<ReqId> },
    Uncovered { ids: Vec<ReqId> },
    NoF2p,
    MetricWithoutValue { ids: Vec<ReqId> },
    Fewer { held: usize, enumerated: usize },
    DidNotRun { cause: NotRun },
    Artifact { raw: String, why: ArtifactRefused },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreezeRefusal {
    pub facts: Vec<FreezeFact>,
}

/// The `spec` session entry: the whole document after every change, newest wins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpecRecord {
    pub at: u64,
    pub op: String,
    pub spec: Spec,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug)]
#[non_exhaustive]
pub enum SpecError {
    BadId(String),
    BadKind(String),
    EmptyCheck,
    NotFrozen,
    AlreadyFrozen,
    UnknownId(ReqId),
    ChildCannotFreeze,
    NotFinite(f64),
    Poisoned,
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl std::fmt::Display for SpecError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadId(text) => {
                write!(
                    formatter,
                    "{text:?} is not a requirement id; ids are R1, R2, …"
                )
            }
            Self::BadKind(text) => {
                write!(
                    formatter,
                    "{text:?} is not a kind; kinds are f2p, p2p, metric"
                )
            }
            Self::EmptyCheck => formatter.write_str("the check command is empty"),
            Self::NotFrozen => formatter.write_str("no spec is frozen; `spec freeze` first"),
            Self::AlreadyFrozen => formatter
                .write_str("the spec is already frozen; `spec add` grows it, `spec run` runs it"),
            Self::UnknownId(id) => write!(formatter, "no requirement {id}"),
            Self::ChildCannotFreeze => {
                formatter.write_str("a child cannot freeze; the root session owns the spec")
            }
            Self::NotFinite(value) => write!(formatter, "{value} is not finite"),
            Self::Poisoned => formatter.write_str("the spec store is poisoned"),
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
        }
    }
}

impl std::error::Error for SpecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::BadId(_)
            | Self::BadKind(_)
            | Self::EmptyCheck
            | Self::NotFrozen
            | Self::AlreadyFrozen
            | Self::UnknownId(_)
            | Self::ChildCannotFreeze
            | Self::NotFinite(_)
            | Self::Poisoned => None,
        }
    }
}
