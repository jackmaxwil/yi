//! Worktree acceptance (plan section 6.6): candidate verified, integration verified, published
//! under the generation check; phases are typestate read from the journal, never from a lane.

use std::marker::PhantomData;
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Value, json};
use yi_types::plan::canonical::Digest;
use yi_types::plan::contract::{
    Contract, Outcome as ContractOutcome, Resolution, Verdict, VerificationToken,
};
use yi_types::plan::doc::{AgentId, Isolation, PlanId, Todo, TodoLabel, TodoState, TouchCount};
use yi_types::plan::ledger::{AttemptId, EffectId, JournalRecord, RequestId};
use yi_types::plan::op::Choice;
use yi_types::url::Url;

pub use yi_types::plan::acceptance::{Conflict, Disposition, Published, Quiescence};

use super::capacity::{Capacity, Permit, Purpose, RESERVE_WAIT};
use super::done::Prepared;
use super::ops::{Actor, Delta, Op, Outcome, PlanEngine, PlanOpError, Txn};
use super::output::check_output;
use super::state::{Claim, Decided, KIND_VERIFICATION_REQUESTED, KIND_VERIFICATION_STALE, root_of};
use super::store::draft;
use super::table::{OpKind, check_plan_state, check_terminal, locate_step, op_name};
use super::verify::Snapshot;
use crate::lane::settle::{Prepared as Merge, Publish, generation_of, prepare, publish, slot_url};
use crate::lane::{Pool, Sha};

pub const KIND_CANDIDATE_SUBMITTED: &str = "candidate_submitted";
pub const KIND_CANDIDATE_VERIFIED: &str = "candidate_verified";
pub const KIND_INTEGRATION_PREPARED: &str = "integration_prepared";
pub const KIND_INTEGRATION_VERIFIED: &str = "integration_verified";
/// A publication refused because the parent moved past the prepared generation.
pub const KIND_INTEGRATION_STALE: &str = "integration_stale";
pub const KIND_ACCEPTED: &str = "accepted";
pub const KIND_DISPOSITION: &str = Disposition::KIND;

/// Re-preparations after a stale publication before `done` gives up on this call.
pub const REPREPARE_CAP: u32 = 2;

/// ponytail: one process-wide integration lock; the generation check at publication is the
/// cross-process guard, so a second process prepares in vain rather than publishes wrong.
static INTEGRATION: Mutex<()> = Mutex::new(());

/// Where a worktree todo's attempt stands, read from its records and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Unsubmitted,
    Submitted,
    CandidateVerified,
    IntegrationPrepared,
    IntegrationVerified,
    /// A publication the parent moved past, not yet prepared again: `done` prepares it.
    IntegrationStale,
    Accepted,
    /// The candidate conflicted with the parent: the branch is retained and the worker keeps
    /// its checkout; the way on is a new `submit` of the resolved candidate.
    MergeFailed,
    Disposed,
}

impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Self::Unsubmitted => "unsubmitted",
            Self::Submitted => KIND_CANDIDATE_SUBMITTED,
            Self::CandidateVerified => KIND_CANDIDATE_VERIFIED,
            Self::IntegrationPrepared => KIND_INTEGRATION_PREPARED,
            Self::IntegrationVerified => KIND_INTEGRATION_VERIFIED,
            Self::IntegrationStale => KIND_INTEGRATION_STALE,
            Self::Accepted => KIND_ACCEPTED,
            Self::MergeFailed => "merge_failed",
            Self::Disposed => KIND_DISPOSITION,
        }
    }

    /// The record `done` is still missing from this phase; `None` once accept is legal.
    pub fn missing(self) -> Option<&'static str> {
        match self {
            Self::Unsubmitted => Some(KIND_CANDIDATE_SUBMITTED),
            Self::Submitted => Some(KIND_CANDIDATE_VERIFIED),
            // A verified candidate whose integration never landed, or landed unverified, is
            // prepared again by `done`; nothing is missing from it (section 6.6 row three).
            Self::CandidateVerified
            | Self::IntegrationPrepared
            | Self::IntegrationVerified
            | Self::IntegrationStale => None,
            Self::Accepted => Some("a new attempt: this one is accepted"),
            Self::MergeFailed => {
                Some("a submit of the resolved candidate: the last one conflicted")
            }
            Self::Disposed => Some("a new attempt: this one was disposed"),
        }
    }
}

/// The `done` an `accepted` record reduces as: the todo it names and the output it carries.
pub(super) fn accepted_op(record: &JournalRecord) -> Result<Op, String> {
    let label = record
        .record
        .todo
        .clone()
        .ok_or_else(|| "accepted names no todo".to_owned())?;
    let output = match record.args.get("output") {
        None | Some(Value::Null) => None,
        Some(value) => {
            Some(serde_json::from_value::<Url>(value.clone()).map_err(|e| e.to_string())?)
        }
    };
    Ok(Op::Done { label, output })
}

fn own(record: &JournalRecord, plan: &PlanId, label: &TodoLabel, attempt: AttemptId) -> bool {
    &record.record.plan == plan
        && record.record.todo.as_ref() == Some(label)
        && record.attempt == Some(attempt)
}

/// The phase the attempt's records prove. A refused verification leaves the phase where it
/// was; a stale publication sends the attempt back to its candidate, prepared again by `done`.
pub fn phase_of(
    records: &[JournalRecord],
    plan: &PlanId,
    label: &TodoLabel,
    attempt: AttemptId,
) -> Phase {
    let mut phase = Phase::Unsubmitted;
    for record in records
        .iter()
        .filter(|record| own(record, plan, label, attempt))
    {
        phase = match record.record.op.as_str() {
            KIND_CANDIDATE_SUBMITTED => Phase::Submitted,
            KIND_CANDIDATE_VERIFIED => Phase::CandidateVerified,
            KIND_INTEGRATION_PREPARED => Phase::IntegrationPrepared,
            KIND_INTEGRATION_VERIFIED => Phase::IntegrationVerified,
            KIND_INTEGRATION_STALE => Phase::IntegrationStale,
            KIND_ACCEPTED => Phase::Accepted,
            KIND_DISPOSITION => match record.args.get("disposition") {
                Some(Value::Object(disposition)) if disposition.contains_key("merge_failed") => {
                    Phase::MergeFailed
                }
                _ => Phase::Disposed,
            },
            _ => phase,
        };
    }
    phase
}

/// The ordinal of the attempt's last preparation; zero before the first.
pub(super) fn last_generation(
    records: &[JournalRecord],
    plan: &PlanId,
    label: &TodoLabel,
    attempt: AttemptId,
) -> u64 {
    records
        .iter()
        .filter(|record| own(record, plan, label, attempt))
        .filter(|record| record.record.op == KIND_INTEGRATION_PREPARED)
        .filter_map(|record| record.args.get("generation").and_then(Value::as_u64))
        .max()
        .unwrap_or(0)
}

/// Invariant: only an `accepted` record completes a worktree todo (section 6.6), so the reducer
/// refuses a plain `done` on one whatever resolution it carries.
pub(super) fn refuse_plain_done(
    state: &super::state::RootState,
    id: &PlanId,
    op: &Op,
) -> Result<(), PlanOpError> {
    if let Op::Done { label, .. } = op
        && state
            .plan(id)
            .ok()
            .and_then(|plan| plan.todo(label))
            .is_some_and(is_worktree)
    {
        return Err(PlanOpError::AcceptanceUnavailable {
            label: label.clone(),
        });
    }
    Ok(())
}

pub fn is_worktree(todo: &Todo) -> bool {
    todo.delegation
        .as_ref()
        .is_some_and(|delegation| delegation.spec.isolation == Some(Isolation::Worktree))
}

/// Typestate markers: what the journal proves about the candidate or the integration.
#[derive(Debug, Clone, Copy)]
pub struct Submitted;
#[derive(Debug, Clone, Copy)]
pub struct Verified;

/// A candidate: the child's commit on its branch, named by attempt and artifact ids.
#[derive(Debug, Clone)]
pub struct Candidate<S> {
    pub label: TodoLabel,
    pub attempt: AttemptId,
    pub branch: String,
    pub commit: Sha,
    pub parent_base: Sha,
    pub outputs: Vec<Url>,
    /// The candidate token, once verified.
    pub token: Option<VerificationToken>,
    pub(super) _phase: PhantomData<fn() -> S>,
}

impl Candidate<Submitted> {
    pub(super) fn verified(self, token: VerificationToken) -> Candidate<Verified> {
        Candidate {
            label: self.label,
            attempt: self.attempt,
            branch: self.branch,
            commit: self.commit,
            parent_base: self.parent_base,
            outputs: self.outputs,
            token: Some(token),
            _phase: PhantomData,
        }
    }
}

impl Candidate<Verified> {
    /// The verified candidate the records prove: its submission and a passing verdict.
    pub fn from_records(
        records: &[JournalRecord],
        plan: &PlanId,
        label: &TodoLabel,
        attempt: AttemptId,
    ) -> Option<Self> {
        let mine: Vec<&JournalRecord> = records
            .iter()
            .filter(|record| own(record, plan, label, attempt))
            .collect();
        // The verdict must follow the submission it verified: a re-submit after a verified
        // candidate is a new commit with no verdict yet, never the old verdict on the new tree.
        let at = mine
            .iter()
            .rposition(|record| record.record.op == KIND_CANDIDATE_SUBMITTED)?;
        let submitted = mine.get(at)?;
        let verified = mine
            .get(at..)?
            .iter()
            .find(|record| record.record.op == KIND_CANDIDATE_VERIFIED)?;
        let args = &submitted.args;
        let text = |name: &str| args.get(name).and_then(Value::as_str);
        let outputs = args
            .get("outputs")
            .and_then(Value::as_array)
            .map(|urls| {
                urls.iter()
                    .filter_map(Value::as_str)
                    .filter_map(|url| url.parse().ok())
                    .collect()
            })
            .unwrap_or_default();
        let token: VerificationToken =
            serde_json::from_value(verified.args.get("token")?.clone()).ok()?;
        Some(Self {
            label: label.clone(),
            attempt,
            branch: text("branch")?.to_owned(),
            commit: Sha::parse(text("candidate")?)?,
            parent_base: Sha::parse(text("parent_base")?)?,
            outputs,
            token: Some(token),
            _phase: PhantomData,
        })
    }
}

/// An integration of a verified candidate onto one parent generation, prepared in a staging
/// lane and, once `Verified`, publishable while that generation still holds.
#[derive(Debug, Clone)]
pub struct Integration<S> {
    pub candidate: Candidate<Verified>,
    /// The ordinal of this preparation on the attempt; the token's `integration`.
    pub generation: u64,
    pub parent_base: Sha,
    /// The branch the parent's HEAD named when the merge was prepared; a switch to another
    /// branch at the same commit is a moved parent too.
    pub parent_branch: Option<String>,
    pub integrated: Sha,
    pub conflict_provenance: Vec<Conflict>,
    pub token: Option<VerificationToken>,
    _phase: PhantomData<fn() -> S>,
}

impl Integration<Verified> {
    /// The verified integration the records prove, after the last preparation.
    pub fn from_records(
        records: &[JournalRecord],
        plan: &PlanId,
        label: &TodoLabel,
        attempt: AttemptId,
    ) -> Option<Self> {
        let candidate = Candidate::<Verified>::from_records(records, plan, label, attempt)?;
        let mine: Vec<&JournalRecord> = records
            .iter()
            .filter(|record| own(record, plan, label, attempt))
            .collect();
        let at = mine
            .iter()
            .rposition(|record| record.record.op == KIND_INTEGRATION_PREPARED)?;
        let prepared = mine.get(at)?;
        let verified = mine
            .get(at..)?
            .iter()
            .find(|record| record.record.op == KIND_INTEGRATION_VERIFIED)?;
        let args = &prepared.args;
        let text = |name: &str| args.get(name).and_then(Value::as_str);
        let token: VerificationToken =
            serde_json::from_value(verified.args.get("token")?.clone()).ok()?;
        Some(Self {
            candidate,
            generation: args.get("generation").and_then(Value::as_u64)?,
            parent_base: Sha::parse(text("parent_base")?)?,
            parent_branch: text("parent_branch").map(str::to_owned),
            integrated: Sha::parse(text("integrated")?)?,
            conflict_provenance: args
                .get("conflict_provenance")
                .cloned()
                .and_then(|value| serde_json::from_value(value).ok())
                .unwrap_or_default(),
            token: Some(token),
            _phase: PhantomData,
        })
    }
}

/// A publication the parent moved past: what preparing the candidate again needs.
struct Stale {
    id: PlanId,
    root: PlanId,
    candidate: Candidate<Verified>,
    generation: u64,
}

/// The frozen contract, its digest and the product bytes one verification runs against.
pub(super) struct Contracted {
    pub(super) contract: Contract,
    pub(super) frozen: Digest,
    pub(super) product: Option<String>,
}

/// One request's identity, carried through the phases that take and release the lease.
pub(super) struct Call<'a> {
    pub(super) actor: &'a Actor,
    pub(super) op: &'a Op,
    pub(super) request: RequestId,
    pub(super) expected: Option<TouchCount>,
}

pub(super) fn verification(label: &TodoLabel, reason: impl Into<String>) -> PlanOpError {
    PlanOpError::Verification {
        label: label.clone(),
        reason: reason.into(),
    }
}

fn effect_id(nonce: &str, label: &TodoLabel) -> Result<EffectId, PlanOpError> {
    EffectId::new(format!("e-{nonce}")).map_err(|error| verification(label, error.to_string()))
}

impl PlanEngine {
    /// The lane pool the staging and verification checkouts come from, and the split of it
    /// that keeps a slot for them (section 7.6).
    pub fn with_lanes(self, lanes: Pool) -> Self {
        let capacity = Capacity::for_slots(lanes.slots());
        Self {
            lanes: Some(lanes),
            capacity,
            ..self
        }
    }

    /// The split shared with the host whose workers draw on the same pool, so its worker
    /// permits and this engine's verification reserve are counted over one object.
    pub fn with_capacity(self, capacity: Arc<Capacity>) -> Self {
        Self { capacity, ..self }
    }

    /// Until when a reservation waits for the reserve: the verifier's deadline, or the bounded
    /// backoff, whichever comes first.
    fn reserve_until(&self) -> std::time::Instant {
        let backoff = std::time::Instant::now()
            .checked_add(RESERVE_WAIT)
            .unwrap_or_else(std::time::Instant::now);
        self.verifier
            .deadline()
            .map_or(backoff, |ends| ends.min(backoff))
    }

    /// The lane share this engine's verification and staging checkouts draw on.
    pub fn capacity(&self) -> &Arc<Capacity> {
        &self.capacity
    }

    /// A verification refusal naming the counter that ran out, never a product refusal.
    pub(super) fn reserve_verification(&self, label: &TodoLabel) -> Result<Permit, PlanOpError> {
        self.capacity
            .reserve_within(Purpose::Verification, self.reserve_until())
            .map_err(|error| verification(label, error.to_string()))
    }

    pub(super) fn pool(&self, label: &TodoLabel) -> Result<&Pool, PlanOpError> {
        self.lanes
            .as_ref()
            .ok_or_else(|| verification(label, "no lane pool is attached to the engine"))
    }

    /// Invariant: a read that fails is an error, never `false`, so a worktree todo never takes
    /// the plain path because its checkpoint could not be read (sections 3.6 and 6.6).
    pub(super) fn is_worktree_todo(
        &self,
        plan: Option<&PlanId>,
        label: Option<&TodoLabel>,
    ) -> Result<bool, PlanOpError> {
        let Some(label) = label else {
            return Ok(false);
        };
        let id = self.resolve(plan.cloned())?;
        let plan = self.store.read(&id)?;
        Ok(plan.todo(label).is_some_and(is_worktree))
    }

    /// The frozen contract and the product bytes for the attempt, as `evidence` reads them.
    pub(super) fn contracted(
        &self,
        txn: &Txn,
        id: &PlanId,
        label: &TodoLabel,
        output: Option<&Url>,
    ) -> Result<Contracted, PlanOpError> {
        self.contracted_via(txn, id, label, output, self.output_resolve.as_deref())
    }

    /// `contracted` over the resolver the caller names: a submit reads its output in the
    /// candidate's checkout, an acceptance in the parent's.
    pub(super) fn contracted_via(
        &self,
        txn: &Txn,
        id: &PlanId,
        label: &TodoLabel,
        output: Option<&Url>,
        resolve: Option<&dyn super::output::OutputResolve>,
    ) -> Result<Contracted, PlanOpError> {
        let plan = txn.state.plan(id)?;
        let todo = plan.todo(label).ok_or_else(|| PlanOpError::UnknownLabel {
            plan: id.clone(),
            label: label.clone(),
        })?;
        let contract = todo
            .contract
            .clone()
            .ok_or_else(|| PlanOpError::AcceptanceUnavailable {
                label: label.clone(),
            })?;
        let frozen = todo.contract_hash.ok_or_else(|| PlanOpError::Contract {
            label: label.clone(),
            detail: "the contract was never frozen; start records it".to_owned(),
        })?;
        if contract.digest()? != frozen {
            return Err(PlanOpError::ContractDrift {
                label: label.clone(),
            });
        }
        let product = check_output(resolve, plan, label, output)?;
        Ok(Contracted {
            contract,
            frozen,
            product,
        })
    }

    pub(super) fn token(
        &self,
        txn: &Txn,
        id: &PlanId,
        label: &TodoLabel,
        contracted: &Contracted,
        snapshot: &Sha,
        integration: Option<u64>,
    ) -> Result<VerificationToken, PlanOpError> {
        let plan = txn.state.plan(id)?;
        let todo = plan.todo(label).ok_or_else(|| PlanOpError::UnknownLabel {
            plan: id.clone(),
            label: label.clone(),
        })?;
        Ok(VerificationToken {
            plan: id.clone(),
            version: plan.version,
            todo: label.clone(),
            attempt: todo.attempt,
            contract_digest: contracted.frozen,
            criteria_digest: contracted.contract.criteria_digest()?,
            output_digest: Digest::of(contracted.product.as_deref().unwrap_or("").as_bytes()),
            snapshot: snapshot.as_str().to_owned(),
            integration,
        })
    }

    /// Section 6.3 step 2 for a candidate or an integration tree: the effect committed under
    /// the lease, so a dropped request leaves an intent recovery can see.
    pub(super) fn request_verification(
        &self,
        txn: &mut Txn,
        id: &PlanId,
        label: &TodoLabel,
        token: &VerificationToken,
    ) -> Result<EffectId, PlanOpError> {
        let effect = effect_id(&self.store.request_nonce(), label)?;
        let claim = Claim {
            pid: std::process::id(),
            at: self.store.now_ms(),
        };
        let mut record = draft(
            id,
            KIND_VERIFICATION_REQUESTED,
            txn.actor.clone(),
            self.store.now_ms(),
            self.todos(txn, id)?,
            json!({"label": label, "token": token, "effect_id": effect, "claim": claim.to_value()}),
            txn.derived(&format!("verify-{}", effect))?,
            txn.expected,
            Some(token.attempt),
        );
        record.record.todo = Some(label.clone());
        let committed = self.commit(txn, record)?;
        self.emit(&committed);
        Ok(effect)
    }

    fn todos(&self, txn: &Txn, id: &PlanId) -> Result<u32, PlanOpError> {
        Ok(u32::try_from(txn.state.plan(id)?.todos.len()).unwrap_or(u32::MAX))
    }

    /// One journal record of the acceptance kinds, named by the todo and its attempt; a
    /// verified kind carries the verdict and the effect it settles.
    pub(super) fn record(
        &self,
        txn: &mut Txn,
        id: &PlanId,
        who: (&TodoLabel, AttemptId),
        kind: &str,
        args: Value,
        evidence: Option<(&Verdict, &EffectId)>,
    ) -> Result<(), PlanOpError> {
        let (label, attempt) = who;
        let mut record = draft(
            id,
            kind,
            txn.actor.clone(),
            self.store.now_ms(),
            self.todos(txn, id)?,
            args,
            txn.derived(&format!("{kind}-{}", self.store.nonce()))?,
            txn.expected,
            Some(attempt),
        );
        record.record.todo = Some(label.clone());
        if let Some((verdict, effect)) = evidence {
            record.verdict = Some(serde_json::to_value(verdict)?);
            record
                .record
                .extra
                .insert("effect_id".to_owned(), Value::String(effect.to_string()));
        }
        let committed = self.commit(txn, record)?;
        self.store.checkpoint_family(&txn.state)?;
        self.emit(&committed);
        Ok(())
    }

    /// Section 6.6 row three: prepared onto the parent's generation under the integration lock,
    /// checked on that exact tree outside it, the staging branch kept as the pin until `done`.
    pub(super) fn integrate(
        &self,
        id: &PlanId,
        root: &PlanId,
        call: &Call<'_>,
        candidate: Candidate<Verified>,
        generation: u64,
    ) -> Result<Integration<Verified>, PlanOpError> {
        let label = candidate.label.clone();
        let attempt = candidate.attempt;
        let deadline = self.verifier.deadline();
        // The same reserve the candidate check used and gave back, held until the staging lane
        // is released or its branch kept below: one candidate at a time (section 6.6).
        let _staging_slot = self.reserve_verification(&label)?;
        let (contracted, token, effect, staging, parent_branch) = {
            let _integration = INTEGRATION.lock().unwrap_or_else(PoisonError::into_inner);
            let _lease = self.lease_waiting()?;
            let mut txn =
                self.begin(root, call.actor, call.request.clone(), call.expected, false)?;
            let output = candidate.outputs.first();
            let contracted = self.contracted(&txn, id, &label, output)?;
            let parent = generation_of(&self.cwd, deadline)
                .map_err(|error| verification(&label, format!("parent checkout: {error}")))?;
            let pool = self.pool(&label)?;
            let session = format!("stage-{}", self.store.nonce());
            let merged = prepare(pool, &session, &parent, &candidate.commit, deadline)
                .map_err(|error| verification(&label, format!("prepare: {error}")))?;
            let staging = match merged {
                Merge::Conflict { base, conflicts } => {
                    let disposition = Disposition::MergeFailed {
                        branch: candidate.branch.clone(),
                        candidate: candidate.commit.as_str().to_owned(),
                        parent_base: base.as_str().to_owned(),
                        generation,
                        conflict_provenance: conflicts.clone(),
                    };
                    self.record_disposition(&mut txn, id, &label, attempt, &disposition)?;
                    return Err(PlanOpError::MergeFailed {
                        label,
                        paths: conflicts
                            .iter()
                            .map(|conflict| conflict.path.clone())
                            .collect(),
                    });
                }
                Merge::Merged(staging) => staging,
            };
            self.record(
                &mut txn,
                id,
                (&label, attempt),
                KIND_INTEGRATION_PREPARED,
                json!({
                    "label": label,
                    "candidate": candidate.commit.as_str(),
                    "parent_base": parent.base.as_str(),
                    "parent_branch": parent.head.branch().map(crate::lane::BranchName::as_str),
                    "generation": generation,
                    "integrated": staging.integrated.as_str(),
                    "staging": staging.slot().map(slot_url),
                    "conflict_provenance": Vec::<Conflict>::new(),
                }),
                None,
            )?;
            let token = self.token(
                &txn,
                id,
                &label,
                &contracted,
                &staging.integrated,
                Some(generation),
            )?;
            let effect = self.request_verification(&mut txn, id, &label, &token)?;
            self.store.checkpoint_family(&txn.state)?;
            let parent_branch = parent
                .head
                .branch()
                .map(|branch| branch.as_str().to_owned());
            (contracted, token, effect, staging, parent_branch)
        };
        let artifacts = self.store.artifacts(id);
        let verdict = match staging.path() {
            Some(root_path) => self.verifier.run(
                &token,
                &contracted.contract,
                &Snapshot {
                    id: &token.snapshot,
                    root: root_path,
                    output: contracted.product.as_deref().map(str::as_bytes),
                    artifacts: &artifacts,
                    jury: None,
                },
            ),
            None => self.verifier.abstained(
                &token,
                &contracted.contract,
                "the staging lane was released early",
            ),
        };
        let _lease = self.lease_waiting()?;
        let mut txn = self.begin(root, call.actor, call.request.clone(), call.expected, false)?;
        if verdict.outcome != ContractOutcome::Pass {
            staging
                .release()
                .map_err(|error| verification(&label, format!("staging lane: {error}")))?;
            let prepared = Prepared {
                id: id.clone(),
                root: root.clone(),
                label: label.clone(),
                token,
                contract: contracted.contract,
                product: contracted.product,
                effect,
                jury: (0, None),
            };
            return match self.refuse(&mut txn, &prepared, call.op, verdict) {
                Ok(_) => Err(verification(&label, "a refusal returned an outcome")),
                Err(error) => Err(error),
            };
        }
        self.record(
            &mut txn,
            id,
            (&label, attempt),
            KIND_INTEGRATION_VERIFIED,
            json!({"label": label, "effect_id": effect, "token": token}),
            Some((&verdict, &effect)),
        )?;
        let integrated = staging.integrated.clone();
        let parent_base = staging.base.clone();
        // The staging branch is the integration's pin until it is published: the slot goes
        // back to the pool, the branch stays.
        staging
            .keep()
            .map_err(|error| verification(&label, format!("staging lane: {error}")))?;
        Ok(Integration {
            candidate,
            generation,
            parent_base,
            parent_branch,
            integrated,
            conflict_provenance: Vec::new(),
            token: Some(token),
            _phase: PhantomData,
        })
    }

    /// Section 6.6 row four on `done`: legal once the records prove a verified integration;
    /// published while the generation still matches, else prepared again, bounded.
    pub(super) fn accept(
        &self,
        plan: Option<PlanId>,
        actor: &Actor,
        op: Op,
        request: RequestId,
        expected: Option<TouchCount>,
    ) -> Result<Outcome, PlanOpError> {
        let Op::Done { label, output } = &op else {
            return Err(PlanOpError::NotJournaled { op: op.kind() });
        };
        let mut reprepared = 0_u32;
        loop {
            let moved = {
                let _integration = INTEGRATION.lock().unwrap_or_else(PoisonError::into_inner);
                let _lease = self.lease_waiting()?;
                let id = self.resolve(plan.clone())?;
                let root = root_of(&id)?;
                let mut txn = self.begin(&root, actor, request.clone(), expected, false)?;
                if let Some(replayed) = self.replay(&txn, &op)? {
                    return Ok(replayed);
                }
                self.check_revision(&txn, &id, expected)?;
                match self.try_publish(&mut txn, &id, &root, label, output.as_ref()) {
                    Ok(Ok(outcome)) => return Ok(outcome),
                    Ok(Err(stale)) => stale,
                    Err(error) => {
                        if error.is_recordable() {
                            self.record_refusal(&mut txn, &id, &op, &error);
                        }
                        return Err(error);
                    }
                }
            };
            let Stale {
                id,
                root,
                candidate,
                generation,
            } = moved;
            reprepared = reprepared.saturating_add(1);
            if reprepared > REPREPARE_CAP {
                return Err(verification(
                    label,
                    format!(
                        "the parent moved {REPREPARE_CAP} times while the integration was prepared; run done again"
                    ),
                ));
            }
            let call = Call {
                actor,
                op: &op,
                request: request.clone(),
                expected,
            };
            self.integrate(&id, &root, &call, candidate, generation.saturating_add(1))?;
        }
    }

    /// Under the lease and the integration lock: the phase check, the publication, and the
    /// acceptance record with the transition. `Err(stale)` inside `Ok` is a moved parent.
    fn try_publish(
        &self,
        txn: &mut Txn,
        id: &PlanId,
        root: &PlanId,
        label: &TodoLabel,
        output: Option<&Url>,
    ) -> Result<Result<Outcome, Stale>, PlanOpError> {
        let plan = txn.state.plan(id)?;
        check_plan_state(plan, OpKind::Done)?;
        locate_step(plan, label, OpKind::Done)?;
        let todo = plan.todo(label).ok_or_else(|| PlanOpError::UnknownLabel {
            plan: id.clone(),
            label: label.clone(),
        })?;
        if todo.contract.is_none() {
            return Err(PlanOpError::AcceptanceUnavailable {
                label: label.clone(),
            });
        }
        let attempt = todo.attempt;
        let by = match &todo.state {
            TodoState::Running { by } => Some(by.clone()),
            _ => None,
        };
        let phase = phase_of(&txn.records, id, label, attempt);
        if let Some(missing) = phase.missing() {
            return Err(PlanOpError::PhaseMissing {
                label: label.clone(),
                phase: phase.name(),
                missing,
            });
        }
        if matches!(
            phase,
            Phase::CandidateVerified | Phase::IntegrationPrepared | Phase::IntegrationStale
        ) {
            // A verified candidate whose integration never landed, landed unverified, or was
            // published past is prepared again here, so a sound candidate is never stranded.
            let candidate =
                Candidate::<Verified>::from_records(&txn.records, id, label, attempt)
                    .ok_or_else(|| verification(label, "the candidate records do not reduce"))?;
            return Ok(Err(Stale {
                id: id.clone(),
                root: root.clone(),
                candidate,
                generation: last_generation(&txn.records, id, label, attempt),
            }));
        }
        let integration =
            Integration::<Verified>::from_records(&txn.records, id, label, attempt)
                .ok_or_else(|| verification(label, "the integration records do not reduce"))?;
        check_terminal(label, output)?;
        // Invariant: the child's quiescence is read before the parent's ref moves, so a busy
        // child never turns a published integration into a `done` with no acceptance record.
        if let Some(by) = &by
            && let Some(held) = self
                .delegate
                .candidate(by)
                .map_err(|reason| verification(label, reason))?
            && !held.quiescence.is_quiet()
        {
            return Err(verification(
                label,
                format!(
                    "the child {by} still runs {} command(s) in its lane; done waits for it",
                    held.quiescence.running_commands()
                ),
            ));
        }
        // Section 6.3 step 5 at the accept phase: the token for the output this call names, or
        // the submitted one, must be the token the integration verified; else stale, uncharged.
        let named = output.or(integration.candidate.outputs.first());
        let contracted = self.contracted(txn, id, label, named)?;
        let now = self.token(
            txn,
            id,
            label,
            &contracted,
            &integration.integrated,
            Some(integration.generation),
        )?;
        if let Some(was) = integration.token.as_ref().filter(|was| **was != now) {
            self.record_stale(txn, id, (label, attempt), was, &now)?;
            return Err(PlanOpError::Stale {
                label: label.clone(),
                token: Box::new(now),
            });
        }
        let deadline = self.verifier.deadline();
        let published = publish(
            &self.cwd,
            &integration.parent_base,
            integration.parent_branch.as_deref(),
            &integration.integrated,
            deadline,
        )
        .map_err(|error| verification(label, format!("publish: {error}")))?;
        let (at, how) = match published {
            Publish::Published { at, how } => (at, how),
            Publish::Moved { expected, found } => {
                // The pin goes with the stale integration; a deletion that fails is written
                // into the record, never dropped, so a leaked `yi/stage-*` branch is visible.
                let pin = self.drop_pin(&integration);
                self.record(
                    txn,
                    id,
                    (label, attempt),
                    KIND_INTEGRATION_STALE,
                    json!({"label": label, "expected": expected.as_str(), "found": found.as_str(),
                           "integrated": integration.integrated.as_str(), "pin": pin_of(&pin)}),
                    None,
                )?;
                return Ok(Err(Stale {
                    id: id.clone(),
                    root: root.clone(),
                    candidate: integration.candidate,
                    generation: integration.generation,
                }));
            }
        };
        let verdicts = json!([
            {"phase": "candidate", "outcome": "pass", "token": integration.candidate.token},
            {"phase": "integration", "outcome": "pass", "token": integration.token},
        ]);
        // The parent's ref holds `integrated` now, so the staging pin can go; its fate rides
        // the acceptance record, a failed deletion naming the leaked branch.
        let pin = self.drop_pin(&integration);
        txn.resolution = Some(Resolution::VerifiedDone);
        txn.verdict = self.last_verdict(&txn.records, id, label, attempt);
        // The acceptance record's body: what was published, from what, with which verdicts.
        txn.accepted = Some(json!({
            "candidate": integration.candidate.commit.as_str(),
            "parent_base": integration.parent_base.as_str(),
            "generation": integration.generation,
            "integrated": integration.integrated.as_str(),
            "published": at.as_str(),
            "how": how,
            "conflict_provenance": integration.conflict_provenance,
            "verdicts": verdicts,
            "pin": pin_of(&pin),
        }));
        // The accepted output is the verified one (section 3.6): a `done` naming none settles
        // with the output the candidate submitted, and that is what its record carries.
        let op = &Op::Done {
            label: label.clone(),
            output: named.cloned(),
        };
        let outcome = self.settle(txn, id, root, op)?;
        Ok(Ok(outcome))
    }

    /// The refusal of step 5: the token the integration verified is not the one the plan
    /// computes now. It settles no effect and charges no refusal (section 6.3).
    fn record_stale(
        &self,
        txn: &mut Txn,
        id: &PlanId,
        who: (&TodoLabel, AttemptId),
        was: &VerificationToken,
        now: &VerificationToken,
    ) -> Result<(), PlanOpError> {
        let (label, attempt) = who;
        let mut record = draft(
            id,
            KIND_VERIFICATION_STALE,
            txn.actor.clone(),
            self.store.now_ms(),
            self.todos(txn, id)?,
            json!({"label": label, "token": was}),
            txn.derived("stale")?,
            txn.expected,
            Some(attempt),
        );
        record.record.todo = Some(label.clone());
        record.record.extra.insert(
            "refusal".to_owned(),
            json!({"code": "stale", "detail": super::done::stale_detail(was, now)}),
        );
        let committed = self.commit(txn, record)?;
        self.store.checkpoint_family(&txn.state)?;
        self.emit(&committed);
        Ok(())
    }

    /// The integration's verdict, the one the acceptance record carries.
    fn last_verdict(
        &self,
        records: &[JournalRecord],
        id: &PlanId,
        label: &TodoLabel,
        attempt: AttemptId,
    ) -> Option<Verdict> {
        records
            .iter()
            .rev()
            .filter(|record| own(record, id, label, attempt))
            .find(|record| record.record.op == KIND_INTEGRATION_VERIFIED)
            .and_then(|record| record.verdict.clone())
            .and_then(|value| serde_json::from_value(value).ok())
    }

    /// The staging branch that pinned the integrated tree, once published or superseded.
    fn drop_pin(&self, integration: &Integration<Verified>) -> Result<(), PlanOpError> {
        let pool = self.pool(&integration.candidate.label)?;
        crate::lane::settle::drop_integration_pin(pool, &integration.integrated)
            .map_err(|error| verification(&integration.candidate.label, error.to_string()))
    }

    /// The disposition record, committed before the lane and its slot are released.
    fn record_disposition(
        &self,
        txn: &mut Txn,
        id: &PlanId,
        label: &TodoLabel,
        attempt: AttemptId,
        disposition: &Disposition,
    ) -> Result<(), PlanOpError> {
        self.record(
            txn,
            id,
            (label, attempt),
            KIND_DISPOSITION,
            json!({"label": label, "disposition": disposition, "slot_released": disposition.releases_slot()}),
            None,
        )
    }

    /// Every child a transition takes out of `Running`, reaped; a worktree child's disposition
    /// is journaled first with the copy that outlives the slot, unless `done` is accepting it.
    pub(super) fn reap_leaving(
        &self,
        txn: &mut Txn,
        op: &Op,
        leaving: Vec<(PlanId, Todo)>,
        delta: &mut Delta,
        decided: &mut Decided,
    ) -> Result<(), PlanOpError> {
        let accepting = txn.accepted.is_some();
        for (plan_id, todo) in leaving {
            let TodoState::Running { by } = &todo.state else {
                continue;
            };
            if is_worktree(&todo) {
                if accepting {
                    // The acceptance record is this branch's journaled fate: the host is told
                    // the choice here too, so the reap settles the lane instead of refusing it.
                    self.delegate.mark(by, Choice::Retained).map_err(|reason| {
                        PlanOpError::ReapFailed {
                            agent: by.clone(),
                            reason,
                        }
                    })?;
                } else {
                    self.dispose(txn, &plan_id, &todo, by, op)?;
                }
            }
            let supplied = todo
                .delegation
                .as_ref()
                .map(|delegation| delegation.context.clone())
                .unwrap_or_default();
            let last =
                self.delegate
                    .reap(by, &supplied)
                    .map_err(|reason| PlanOpError::ReapFailed {
                        agent: by.clone(),
                        reason,
                    })?;
            check_terminal(&todo.label, last.as_ref())?;
            delta
                .reaped
                .push(super::ops::agent_url(&yi_types::plan::doc::TodoAddr {
                    plan: plan_id.clone(),
                    todo: todo.label.clone(),
                })?);
            decided.reaped.push(yi_types::plan::op::Reaped {
                plan: plan_id,
                todo: todo.label.clone(),
                agent: by.clone(),
                last,
            });
        }
        Ok(())
    }

    /// The disposition path for one worktree child: the choice rides the op (`fail`, `drop`),
    /// defaults to `Retained`, and lands in the journal before the reap releases anything.
    fn dispose(
        &self,
        txn: &mut Txn,
        plan_id: &PlanId,
        todo: &Todo,
        by: &AgentId,
        op: &Op,
    ) -> Result<(), PlanOpError> {
        let (choice, why) = match op {
            Op::Fail {
                cause, disposition, ..
            } => (disposition.unwrap_or(Choice::Retained), cause.clone()),
            Op::Drop { disposition, .. } => (
                disposition.unwrap_or(Choice::Retained),
                "dropped".to_owned(),
            ),
            Op::Supersede { reason, .. } => (Choice::Retained, format!("superseded: {reason}")),
            other => (Choice::Retained, op_name(other.kind()).to_owned()),
        };
        // A retry after a reap that failed finds its disposition already journaled: the record
        // stands, the host is marked again, and no second record is written.
        let already = phase_of(&txn.records, plan_id, &todo.label, todo.attempt) == Phase::Disposed;
        let refs = self
            .delegate
            .candidate(by)
            .map_err(|reason| PlanOpError::ReapFailed {
                agent: by.clone(),
                reason,
            })?
            .map(|held| held.candidate);
        let (branch, candidate) = match refs {
            Some(refs) => (
                Some(refs.branch.as_str().to_owned()),
                Some(refs.commit.as_str().to_owned()),
            ),
            None => submitted_refs(&txn.records, plan_id, &todo.label, todo.attempt),
        };
        let trace: Url =
            format!("history://{by}")
                .parse()
                .map_err(|error: yi_types::url::UrlError| {
                    verification(&todo.label, error.to_string())
                })?;
        let kept = vec![trace];
        let disposition = match choice {
            Choice::Retained => Disposition::Retained {
                branch,
                candidate,
                kept,
                reason: why,
            },
            Choice::Discarded => Disposition::Discarded {
                branch,
                candidate,
                kept,
                reason: why,
            },
        };
        if !already {
            self.record_disposition(txn, plan_id, &todo.label, todo.attempt, &disposition)?;
        }
        // The host learns the choice only once the journal carries it (section 6.6).
        self.delegate
            .mark(by, choice)
            .map_err(|reason| PlanOpError::ReapFailed {
                agent: by.clone(),
                reason,
            })
    }
}

/// What became of a staging pin, for the record that outlives it.
fn pin_of(dropped: &Result<(), PlanOpError>) -> Value {
    match dropped {
        Ok(()) => json!({"dropped": true}),
        Err(error) => json!({"dropped": false, "error": error.to_string()}),
    }
}

/// The branch and commit the attempt's `candidate_submitted` record named, for a child whose
/// lane the host no longer holds.
fn submitted_refs(
    records: &[JournalRecord],
    plan: &PlanId,
    label: &TodoLabel,
    attempt: AttemptId,
) -> (Option<String>, Option<String>) {
    records
        .iter()
        .rev()
        .filter(|record| own(record, plan, label, attempt))
        .find(|record| record.record.op == KIND_CANDIDATE_SUBMITTED)
        .map(|record| {
            let text = |name: &str| {
                record
                    .args
                    .get(name)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            };
            (text("branch"), text("candidate"))
        })
        .unwrap_or((None, None))
}
