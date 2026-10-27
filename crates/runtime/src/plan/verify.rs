//! The verifier (plan section 6.3, step 4): a frozen contract run against one snapshot, item by
//! item, aggregated by section 6.2; the done path (`done.rs`) owns the token and the journal.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use yi_types::plan::canonical::Digest;
pub use yi_types::plan::contract::{Case, CheckerManifest, Cwd, MANIFEST_FORMAT};
use yi_types::plan::contract::{
    Contract, ContractItem, Decider, ItemLine, ItemVerdict, Verdict, VerificationToken, aggregate,
};

use super::artifact::Artifacts;
pub use super::snapshot::{Snapshotter, TreeHash};
use crate::goal::run_check_in;

/// Retries of an item the host failed to spawn, backing off this many ms per attempt.
const ABSTAIN_RETRIES: u32 = 2;
const ABSTAIN_BACKOFF_MS: u64 = 100;
const EVIDENCE_CHARS: usize = 300;
const PAST_DEADLINE: &str = "the verification deadline passed before this item ran";

/// The workspace the snapshot names, the attempt's output bytes, and the criteria store.
pub struct Snapshot<'a> {
    pub id: &'a str,
    pub root: &'a Path,
    pub output: Option<&'a [u8]>,
    pub artifacts: &'a Artifacts,
}

/// The digests a `start` freezes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frozen {
    pub contract_digest: Digest,
    pub criteria_digest: Digest,
}

pub fn session_has(name: &str) -> bool {
    std::env::var_os(name).is_some()
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
                CheckerManifest::parse(
                    &artifacts.get(&checker.digest).map_err(|e| e.to_string())?,
                    session_has,
                )
                .map_err(|error| format!("item {}: {error}", item.id))?;
            }
            Decider::Example { cases, runner, .. } => {
                CheckerManifest::parse(
                    &artifacts.get(&runner.digest).map_err(|e| e.to_string())?,
                    session_has,
                )
                .map_err(|error| format!("item {}: runner {error}", item.id))?;
                read_cases(artifacts, cases)
                    .map_err(|error| format!("item {}: {error}", item.id))?;
            }
            Decider::Schema { schema } => {
                read_schema(artifacts, schema)
                    .map_err(|error| format!("item {}: {error}", item.id))?;
            }
            Decider::Judge { .. } => {
                return Err(format!(
                    "item {}: no judge runs a `judge` item; decide it with `cmd`, `schema` or `example`",
                    item.id
                ));
            }
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
    /// The session's own clock (D177): a verification never outlives the run it belongs to.
    deadline: Option<Instant>,
    hooks: Option<Arc<crate::claude_hooks::Hooks>>,
}

fn deadline_after(from: Instant, millis: u64) -> Instant {
    from.checked_add(Duration::from_millis(millis))
        .unwrap_or(from)
}

fn abstain(reason: impl Into<String>) -> ItemVerdict {
    let reason = reason.into();
    ItemVerdict::Abstain { reason }
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
            deadline: None,
            hooks: None,
        }
    }

    pub fn with_deadline(self, at: Instant) -> Self {
        Self {
            deadline: Some(at),
            ..self
        }
    }

    pub fn with_claude_hooks(mut self, hooks: Option<Arc<crate::claude_hooks::Hooks>>) -> Self {
        self.hooks = hooks;
        self
    }

    pub(super) fn hook_config(&self) -> Option<&crate::claude_hooks::Hooks> {
        self.hooks.as_deref()
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
            if Instant::now() >= whole {
                lines.push(ItemLine {
                    id: item.id.clone(),
                    verdict: abstain(PAST_DEADLINE),
                    jurors: Vec::new(),
                    evidence: None,
                });
                continue;
            }
            let mut evidence = None;
            let mut run = |evidence: &mut Option<String>| {
                self.run_item(item, snapshot, whole, (&mut reproducible, evidence))
            };
            let mut verdict = run(&mut evidence);
            let mut retries = 0;
            while retryable(&verdict) && retries < ABSTAIN_RETRIES && Instant::now() < whole {
                retries = retries.saturating_add(1);
                std::thread::sleep(Duration::from_millis(
                    ABSTAIN_BACKOFF_MS.saturating_mul(u64::from(retries)),
                ));
                verdict = run(&mut evidence);
            }
            let evidence = evidence.filter(|_| verdict == ItemVerdict::Pass);
            lines.push(ItemLine {
                id: item.id.clone(),
                verdict,
                jurors: Vec::new(),
                evidence,
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
                evidence: None,
            })
            .collect();
        aggregated(token, contract, lines, true, Instant::now())
    }

    fn run_item(
        &self,
        item: &ContractItem,
        snapshot: &Snapshot<'_>,
        whole: Instant,
        (reproducible, evidence): (&mut bool, &mut Option<String>),
    ) -> ItemVerdict {
        match &item.decider {
            Decider::Cmd {
                checker,
                timeout_ms,
            } => {
                let manifest = match snapshot
                    .artifacts
                    .get(&checker.digest)
                    .map_err(|e| e.to_string())
                    .and_then(|bytes| CheckerManifest::parse(&bytes, session_has))
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
                    let (verdict, said) = run_cmd_said(
                        &manifest,
                        snapshot.root,
                        snapshot.artifacts,
                        deadline,
                        self.hooks.as_deref(),
                    );
                    *evidence = said;
                    verdict
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
                    .and_then(|bytes| CheckerManifest::parse(&bytes, session_has))
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
                    run_examples(
                        &manifest,
                        &cases,
                        snapshot.root,
                        item_deadline,
                        self.hooks.as_deref(),
                    )
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

/// Protected paths digested under the root; a missing one is `None`, so appearing counts too.
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

/// A check that changed a protected path fails its item: paths are digested before and after.
pub(super) fn protecting(
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

fn run_cmd_said(
    manifest: &CheckerManifest,
    root: &Path,
    artifacts: &Artifacts,
    deadline: Instant,
    hooks: Option<&crate::claude_hooks::Hooks>,
) -> (ItemVerdict, Option<String>) {
    let workdir = manifest.workdir(root);
    let capture = run_check_in(
        &workdir,
        &manifest.command,
        &manifest.env,
        None,
        deadline,
        None,
        hooks,
    );
    let passed =
        |capture: &&yi_tools::CommandCapture| capture.exit_code == Some(0) && !capture.cancelled;
    let said = capture.as_ref().ok().filter(passed).map(|capture| {
        let output = format!(
            "{}\n{}",
            capture.stdout.trim_end(),
            capture.stderr.trim_end()
        );
        let output = output.trim();
        let total = capture.stdout.trim().chars().count() + capture.stderr.trim().chars().count();
        let skip = output.chars().count().saturating_sub(EVIDENCE_CHARS);
        let kept: String = output.chars().skip(skip).collect();
        let cut = if total > EVIDENCE_CHARS {
            format!("[… last {EVIDENCE_CHARS} of {total} chars; the rest was not kept]\n")
        } else {
            String::new()
        };
        format!("`{}` exit 0\n{cut}{kept}", manifest.command)
    });
    (verdict_of(capture, manifest, &workdir, artifacts), said)
}

pub(super) fn run_cmd(
    manifest: &CheckerManifest,
    root: &Path,
    artifacts: &Artifacts,
    deadline: Instant,
    stop: Option<&yi_tools::CancelFlag>,
    hooks: Option<&crate::claude_hooks::Hooks>,
) -> ItemVerdict {
    let workdir = manifest.workdir(root);
    let capture = run_check_in(
        &workdir,
        &manifest.command,
        &manifest.env,
        None,
        deadline,
        stop,
        hooks,
    );
    verdict_of(capture, manifest, &workdir, artifacts)
}

fn verdict_of(
    capture: Result<yi_tools::CommandCapture, String>,
    manifest: &CheckerManifest,
    workdir: &Path,
    artifacts: &Artifacts,
) -> ItemVerdict {
    let capture = match capture {
        Ok(capture) => capture,
        Err(reason) => return abstain(reason),
    };
    if capture.cancelled {
        return abstain(format!(
            "checker timed out (deadline {} ms); run it by hand, then accepted_by_user with its exit line as note",
            manifest.timeout_ms
        ));
    }
    match capture.exit_code {
        Some(0) => ItemVerdict::Pass,
        code => ItemVerdict::Fail {
            detail: format!(
                "exit {}: {}",
                code.map_or_else(|| "signal".to_owned(), |code| code.to_string()),
                crate::goal::check_view(
                    &manifest.command,
                    workdir,
                    Some(&artifacts.spill_dir()),
                    &capture
                )
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
    hooks: Option<&crate::claude_hooks::Hooks>,
) -> ItemVerdict {
    let mut failed: Vec<(usize, &'static str)> = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        match run_case(manifest, case, root, item_deadline, hooks) {
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
    hooks: Option<&crate::claude_hooks::Hooks>,
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
        None,
        hooks,
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
