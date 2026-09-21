//! The verifier (plan section 6.3, step 4): a frozen contract run against one snapshot, item by
//! item, aggregated by section 6.2; the done path (`done.rs`) owns the token and the journal.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use yi_types::plan::canonical::Digest;
pub use yi_types::plan::contract::{Case, CheckerManifest, Cwd, MANIFEST_FORMAT};
use yi_types::plan::contract::{
    Contract, ContractItem, Decider, ItemLine, ItemVerdict, JUDGE_CAP_PER_TODO, JurorLine, Verdict,
    VerificationToken, aggregate,
};

use super::artifact::Artifacts;
pub use super::snapshot::{Snapshotter, TreeHash};
use crate::goal::run_check_in;

/// Retries of an item the host failed to spawn, backing off this many ms per attempt.
const ABSTAIN_RETRIES: u32 = 2;
const ABSTAIN_BACKOFF_MS: u64 = 100;

/// The judge tier (`judge.rs`). A verifier built without one abstains every judge item.
pub trait Judge: Send + Sync {
    /// The item's verdict and one line per juror seated, decided before `until`.
    fn judge(
        &self,
        item: &ContractItem,
        snapshot: &Snapshot<'_>,
        until: Instant,
    ) -> (ItemVerdict, Vec<JurorLine>);
}

/// What the done path establishes under the lease before a jury may sit (plan section 6.4).
pub struct Seat<'a> {
    /// Invariant: jurors sit above the worker cap only under the verification reserve, and only
    /// Rust mints a permit, so no spawn payload can claim the seat.
    pub permit: &'a super::capacity::Permit,
    /// Verifications requested on this todo at this plan version, this one included.
    pub juries: u32,
    /// The selector the todo's child was spawned with, when the plan names one.
    pub owner_model: Option<&'a str>,
}

/// The workspace the snapshot names, the attempt's output bytes, and the criteria store.
pub struct Snapshot<'a> {
    pub id: &'a str,
    pub root: &'a Path,
    pub output: Option<&'a [u8]>,
    pub artifacts: &'a Artifacts,
    /// None on a path that seats no jury: a judge item abstains there.
    pub jury: Option<Seat<'a>>,
}

/// The digests a `start` freezes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frozen {
    pub contract_digest: Digest,
    pub criteria_digest: Digest,
}

/// # Errors
/// The contract fails validation or names a criterion the store cannot serve as declared.
pub fn freeze(artifacts: &Artifacts, contract: &Contract) -> Result<Frozen, String> {
    contract.validate().map_err(|error| error.to_string())?;
    for item in &contract.items {
        for artifact in item.decider.criteria() {
            let bytes = artifacts
                .get(&artifact.digest)
                .map_err(|error| format!("item {}: {error}", item.id))?;
            if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != artifact.length {
                return Err(format!(
                    "item {}: artifact {} is {} bytes, not the declared {}",
                    item.id,
                    artifact.digest,
                    bytes.len(),
                    artifact.length
                ));
            }
        }
        match &item.decider {
            Decider::Cmd { checker, .. } => {
                CheckerManifest::parse(&artifacts.get(&checker.digest).map_err(|e| e.to_string())?)
                    .map_err(|error| format!("item {}: {error}", item.id))?;
            }
            Decider::Example { cases, runner, .. } => {
                CheckerManifest::parse(&artifacts.get(&runner.digest).map_err(|e| e.to_string())?)
                    .map_err(|error| format!("item {}: runner {error}", item.id))?;
                read_cases(artifacts, cases)
                    .map_err(|error| format!("item {}: {error}", item.id))?;
            }
            Decider::Schema { schema } => {
                read_schema(artifacts, schema)
                    .map_err(|error| format!("item {}: {error}", item.id))?;
            }
            Decider::Judge { .. } => {}
        }
    }
    Ok(Frozen {
        contract_digest: contract.digest().map_err(|error| error.to_string())?,
        criteria_digest: contract
            .criteria_digest()
            .map_err(|error| error.to_string())?,
    })
}

fn read_cases(
    artifacts: &Artifacts,
    cases: &yi_types::plan::canonical::ArtifactRef,
) -> Result<Vec<Case>, String> {
    let bytes = artifacts.get(&cases.digest).map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| format!("cases: {error}"))
}

fn read_schema(
    artifacts: &Artifacts,
    schema: &yi_types::plan::canonical::ArtifactRef,
) -> Result<crate::schema::Schema, String> {
    let bytes = artifacts.get(&schema.digest).map_err(|e| e.to_string())?;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|error| format!("schema: {error}"))?;
    // A criterion refuses what the subset does not implement; freeze is where that lands.
    crate::schema::Schema::criterion(value).map_err(|error| format!("schema: {error}"))
}

pub struct Verifier {
    timeout_ms: u64,
    judge: Option<Arc<dyn Judge>>,
    /// The session's own clock (D177): a verification never outlives the run it belongs to.
    deadline: Option<Instant>,
}

fn deadline_after(from: Instant, millis: u64) -> Instant {
    from.checked_add(Duration::from_millis(millis))
        .unwrap_or(from)
}

fn abstain(reason: impl Into<String>) -> ItemVerdict {
    let reason = reason.into();
    ItemVerdict::Abstain { reason }
}

/// The jury cap (plan section 6.4): past it no jury sits, and the item is the user's question.
fn escalated(item: &ContractItem) -> ItemVerdict {
    ItemVerdict::Escalate {
        question: format!(
            "{JUDGE_CAP_PER_TODO} juries sat on this todo without settling item {}; accept it, fail it or change its contract",
            item.id
        ),
    }
}

fn retryable(verdict: &ItemVerdict) -> bool {
    matches!(verdict, ItemVerdict::Abstain { reason } if reason.starts_with("failed to spawn"))
}

/// One complete JSON value on stdout, or the distinct reason it is not one.
pub fn one_json_value(stdout: &str) -> Result<Value, &'static str> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Err("empty_output");
    }
    let mut stream = serde_json::Deserializer::from_str(trimmed).into_iter::<Value>();
    let first = match stream.next() {
        None => return Err("empty_output"),
        Some(Err(_)) => {
            let non_finite = trimmed
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
                .any(|token| matches!(token, "NaN" | "Infinity" | "-Infinity"));
            return Err(if non_finite { "non_finite" } else { "not_json" });
        }
        Some(Ok(value)) => value,
    };
    let rest = trimmed.get(stream.byte_offset()..).unwrap_or("").trim();
    if rest.is_empty() {
        return Ok(first);
    }
    match stream.next() {
        Some(Ok(_)) => Err("several_values"),
        Some(Err(_)) | None => Err("trailing_output"),
    }
}

impl Verifier {
    pub fn new(timeout_ms: u64) -> Self {
        Self {
            timeout_ms,
            judge: None,
            deadline: None,
        }
    }

    pub fn with_deadline(self, at: Instant) -> Self {
        Self {
            deadline: Some(at),
            ..self
        }
    }

    pub fn with_judge(self, judge: Arc<dyn Judge>) -> Self {
        Self {
            judge: Some(judge),
            ..self
        }
    }

    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }

    /// The session deadline the verifier was built with, so a settle can read the same clock.
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// Run every item under one whole-verification deadline and each item's own, then
    /// aggregate. Never fails: what cannot be decided abstains with its reason.
    pub fn run(
        &self,
        token: &VerificationToken,
        contract: &Contract,
        snapshot: &Snapshot<'_>,
    ) -> Verdict {
        let started = Instant::now();
        let whole = self.deadline.map_or_else(
            || deadline_after(started, self.timeout_ms),
            |at| deadline_after(started, self.timeout_ms).min(at),
        );
        let mut lines = Vec::with_capacity(contract.items.len());
        let mut reproducible = true;
        for item in &contract.items {
            if let (Decider::Judge { .. }, Some(judge)) = (&item.decider, &self.judge) {
                let (verdict, jurors) = match &snapshot.jury {
                    Some(seat) if seat.juries > JUDGE_CAP_PER_TODO => (escalated(item), Vec::new()),
                    _ => judge.judge(item, snapshot, whole),
                };
                let id = item.id.clone();
                lines.push(ItemLine {
                    id,
                    verdict,
                    jurors,
                });
                continue;
            }
            let mut verdict = self.run_item(item, snapshot, whole, &mut reproducible);
            let mut retries = 0;
            while retryable(&verdict) && retries < ABSTAIN_RETRIES && Instant::now() < whole {
                retries = retries.saturating_add(1);
                std::thread::sleep(Duration::from_millis(
                    ABSTAIN_BACKOFF_MS.saturating_mul(u64::from(retries)),
                ));
                verdict = self.run_item(item, snapshot, whole, &mut reproducible);
            }
            lines.push(ItemLine {
                id: item.id.clone(),
                verdict,
                jurors: Vec::new(),
            });
        }
        aggregated(token, contract, lines, reproducible, started)
    }

    /// Every item abstained for one reason the verifier never got to run against: the
    /// snapshot could not be materialized. Infrastructure, so it charges nothing.
    pub fn abstained(
        &self,
        token: &VerificationToken,
        contract: &Contract,
        reason: &str,
    ) -> Verdict {
        let lines = contract
            .items
            .iter()
            .map(|item| ItemLine {
                id: item.id.clone(),
                verdict: abstain(reason),
                jurors: Vec::new(),
            })
            .collect();
        aggregated(token, contract, lines, true, Instant::now())
    }

    fn run_item(
        &self,
        item: &ContractItem,
        snapshot: &Snapshot<'_>,
        whole: Instant,
        reproducible: &mut bool,
    ) -> ItemVerdict {
        if Instant::now() >= whole {
            return abstain("the verification deadline passed before this item ran");
        }
        match &item.decider {
            Decider::Cmd {
                checker,
                timeout_ms,
            } => {
                let manifest = match snapshot
                    .artifacts
                    .get(&checker.digest)
                    .map_err(|e| e.to_string())
                    .and_then(|bytes| CheckerManifest::parse(&bytes))
                {
                    Ok(manifest) => manifest,
                    Err(reason) => return abstain(reason),
                };
                if manifest.reads_outside_snapshot {
                    *reproducible = false;
                }
                let deadline =
                    deadline_after(Instant::now(), (*timeout_ms).min(manifest.timeout_ms))
                        .min(whole);
                protecting(&manifest, snapshot.root, || {
                    run_cmd(&manifest, snapshot.root, deadline)
                })
            }
            Decider::Schema { schema } => run_schema(snapshot, schema),
            Decider::Example {
                cases,
                runner,
                timeout_ms,
            } => {
                let manifest = match snapshot
                    .artifacts
                    .get(&runner.digest)
                    .map_err(|e| e.to_string())
                    .and_then(|bytes| CheckerManifest::parse(&bytes))
                {
                    Ok(manifest) => manifest,
                    Err(reason) => return abstain(reason),
                };
                let cases = match read_cases(snapshot.artifacts, cases) {
                    Ok(cases) => cases,
                    Err(reason) => return abstain(reason),
                };
                if manifest.reads_outside_snapshot {
                    *reproducible = false;
                }
                let item_deadline = deadline_after(Instant::now(), *timeout_ms).min(whole);
                protecting(&manifest, snapshot.root, || {
                    run_examples(&manifest, &cases, snapshot.root, item_deadline)
                })
            }
            Decider::Judge { .. } => abstain("no judge"),
        }
    }
}

fn aggregated(
    token: &VerificationToken,
    contract: &Contract,
    lines: Vec<ItemLine>,
    reproducible: bool,
    started: Instant,
) -> Verdict {
    let paired: Vec<_> = contract
        .items
        .iter()
        .cloned()
        .zip(lines.iter().map(|line| line.verdict.clone()))
        .collect();
    let result = aggregate(&paired, contract.threshold, contract.min_coverage);
    Verdict {
        token: token.clone(),
        outcome: result.outcome,
        score: result.score,
        coverage: result.coverage,
        items: lines,
        reproducible,
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        at: yi_session::now_ms(),
    }
}

/// The manifest's protected paths as they stand, relative to the snapshot root; a missing
/// path digests as `None`, so appearing or vanishing is a change too.
fn protected_digests(manifest: &CheckerManifest, root: &Path) -> Vec<Option<Digest>> {
    manifest
        .protected
        .iter()
        .map(|path| {
            std::fs::read(root.join(path))
                .ok()
                .map(|bytes| Digest::of(&bytes))
        })
        .collect()
}

/// A check that changed a protected path fails the item rather than passing it
/// (`fixtures/plans/contracts/contracts.md`): the paths are digested before and after.
fn protecting(
    manifest: &CheckerManifest,
    root: &Path,
    check: impl FnOnce() -> ItemVerdict,
) -> ItemVerdict {
    let before = protected_digests(manifest, root);
    let verdict = check();
    let after = protected_digests(manifest, root);
    let changed = before
        .iter()
        .zip(&after)
        .position(|(was, now)| was != now)
        .and_then(|index| manifest.protected.get(index));
    match changed {
        Some(path) => ItemVerdict::Fail {
            detail: format!("protected path {path} changed during the check"),
        },
        None => verdict,
    }
}

fn run_cmd(manifest: &CheckerManifest, root: &Path, deadline: Instant) -> ItemVerdict {
    let capture = match run_check_in(
        &manifest.workdir(root),
        &manifest.command,
        &manifest.env,
        None,
        deadline,
    ) {
        Ok(capture) => capture,
        Err(reason) => return abstain(reason),
    };
    if capture.cancelled {
        return abstain(format!(
            "checker timed out (deadline {} ms)",
            manifest.timeout_ms
        ));
    }
    match capture.exit_code {
        Some(0) => ItemVerdict::Pass,
        code => ItemVerdict::Fail {
            detail: format!(
                "exit {}: {}",
                code.map_or_else(|| "signal".to_owned(), |code| code.to_string()),
                crate::goal::output_tail(&capture)
            ),
        },
    }
}

fn run_schema(
    snapshot: &Snapshot<'_>,
    schema: &yi_types::plan::canonical::ArtifactRef,
) -> ItemVerdict {
    let Some(output) = snapshot.output else {
        return abstain("no output artifact to validate");
    };
    let schema = match read_schema(snapshot.artifacts, schema) {
        Ok(schema) => schema,
        Err(reason) => return abstain(reason),
    };
    let product = String::from_utf8_lossy(output);
    let value = match crate::schema::extract(&product) {
        Ok(value) => value,
        Err(detail) => return ItemVerdict::Fail { detail },
    };
    match schema.validate(&value) {
        Ok(()) => ItemVerdict::Pass,
        Err(detail) => ItemVerdict::Fail { detail },
    }
}

/// Why one case did not pass: the runner's answer, or a host that could not run it at all.
enum CaseError {
    Case(&'static str),
    /// The shell did not spawn: infrastructure, abstained and retried, never the product's.
    Host(String),
}

fn run_examples(
    manifest: &CheckerManifest,
    cases: &[Case],
    root: &Path,
    item_deadline: Instant,
) -> ItemVerdict {
    let mut failed: Vec<(usize, &'static str)> = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        match run_case(manifest, case, root, item_deadline) {
            Ok(()) => {}
            Err(CaseError::Case(why)) => failed.push((index, why)),
            Err(CaseError::Host(reason)) => return abstain(reason),
        }
    }
    if failed.is_empty() {
        return ItemVerdict::Pass;
    }
    let indexes: Vec<String> = failed.iter().map(|(index, _)| index.to_string()).collect();
    let reasons: Vec<String> = failed
        .iter()
        .map(|(index, why)| format!("{index} {why}"))
        .collect();
    ItemVerdict::Fail {
        detail: format!(
            "cases {} failed: {}",
            indexes.join(", "),
            reasons.join(", ")
        ),
    }
}

fn run_case(
    manifest: &CheckerManifest,
    case: &Case,
    root: &Path,
    item_deadline: Instant,
) -> Result<(), CaseError> {
    if Instant::now() >= item_deadline {
        return Err(CaseError::Case("timeout"));
    }
    let input = serde_json::to_vec(&case.input).map_err(|_| CaseError::Case("not_json"))?;
    let deadline = deadline_after(Instant::now(), manifest.timeout_ms).min(item_deadline);
    let capture = run_check_in(
        &manifest.workdir(root),
        &manifest.command,
        &manifest.env,
        Some(input),
        deadline,
    )
    .map_err(CaseError::Host)?;
    if capture.cancelled {
        return Err(CaseError::Case("timeout"));
    }
    if capture.truncated {
        return Err(CaseError::Case("output_overflow"));
    }
    if capture.exit_code != Some(0) {
        return Err(CaseError::Case("nonzero_exit"));
    }
    let value = one_json_value(&capture.stdout).map_err(CaseError::Case)?;
    // Numbers compare by their JSON representation (`contracts.md`): `1` and `1.0` differ.
    if value == case.expected {
        Ok(())
    } else {
        Err(CaseError::Case("wrong_answer"))
    }
}
