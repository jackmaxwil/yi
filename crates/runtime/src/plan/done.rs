//! The done path (plan section 6.3): freeze, commit the effect, release the lease, verify,
//! re-acquire, compare the whole token, commit the verdict or `done_refused`.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};
use yi_types::plan::canonical::Digest;
use yi_types::plan::contract::{
    Decider, ItemVerdict, Outcome as ContractOutcome, Resolution, Verdict, VerificationToken, Vote,
};
use yi_types::plan::doc::{BlockedOn, Plan, PlanId, TodoLabel, TodoState, TouchCount};
use yi_types::plan::ledger::{AttemptId, EffectId, JournalRecord, RequestId};

use super::journal::JournalError;
use super::ops::{Actor, Op, Outcome, PlanEngine, PlanOpError, Txn};
use super::output::check_output;
use super::state::{
    Claim, Decided, KIND_DONE_REFUSED, KIND_VERIFICATION_REQUESTED, KIND_VERIFICATION_STALE,
    Verification, root_of,
};
use super::store::draft;
use super::table::{OpKind, check_plan_state, locate_step};
use super::verify::Snapshot;

/// One verification running in this process, keyed by its token digest: a second `done` for
/// the same token waits here instead of running the checks again.
pub(super) struct Flight {
    settled: Mutex<Option<Result<Verdict, String>>>,
    ready: Condvar,
}

pub(super) type InFlight = HashMap<Digest, Arc<Flight>>;

impl Flight {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            settled: Mutex::new(None),
            ready: Condvar::new(),
        })
    }

    pub(super) fn publish(&self, result: Result<Verdict, String>) {
        let mut slot = self.settled.lock().unwrap_or_else(PoisonError::into_inner);
        *slot = Some(result);
        self.ready.notify_all();
    }

    fn wait(&self, timeout: Duration) -> Option<Result<Verdict, String>> {
        let guard = self.settled.lock().unwrap_or_else(PoisonError::into_inner);
        let (guard, _) = self
            .ready
            .wait_timeout_while(guard, timeout, |slot| slot.is_none())
            .unwrap_or_else(PoisonError::into_inner);
        guard.clone()
    }
}

/// How long a `done` waits for a lease another `done` in this process holds around its own
/// step 1 or 5 before giving up; every other op keeps the store's refuse-at-once rule.
const LEASE_WAIT: Duration = Duration::from_secs(10);
const LEASE_POLL: Duration = Duration::from_millis(20);
/// The grace past the verifier's own deadline before another process's claim counts as dead.
const CLAIM_GRACE_MS: u64 = 5_000;

/// What step 1 established under the lease and step 4 runs against.
pub(super) struct Prepared {
    pub(super) id: PlanId,
    pub(super) root: PlanId,
    pub(super) label: TodoLabel,
    pub(super) token: VerificationToken,
    pub(super) contract: yi_types::plan::contract::Contract,
    pub(super) product: Option<String>,
    pub(super) effect: EffectId,
    /// Juries this todo has convened at this plan version, and the model its child was given.
    pub(super) jury: (u32, Option<String>),
}

enum Phase1 {
    /// The plain path: no contract, or the user's own acceptance.
    Plain(Box<Outcome>),
    /// A verification this call runs, registered in this process under the lease.
    Run(Box<Prepared>, Arc<Flight>),
    /// A verification another call in this process is running.
    Join(Arc<Flight>, PlanId, TodoLabel),
}

impl PlanEngine {
    /// `done` on every surface (plan section 6.5): the todo's contract decides whether a
    /// verification runs, and nothing here authors `Done` without a resolution.
    pub(super) fn done(
        &self,
        plan: Option<PlanId>,
        actor: &Actor,
        op: Op,
        request: RequestId,
        expected: Option<TouchCount>,
    ) -> Result<Outcome, PlanOpError> {
        let (prepared, flight) = match self.prepare(plan, actor, &op, request.clone(), expected)? {
            Phase1::Plain(outcome) => return Ok(*outcome),
            Phase1::Join(flight, id, label) => {
                return self.joined(&flight, id, label);
            }
            Phase1::Run(prepared, flight) => (*prepared, flight),
        };
        // Step 3: the lease is gone; a checker may run for minutes.
        if let Some(hook) = &self.verify_hook {
            hook();
        }
        let verdict = self.run_verifier(&prepared);
        let settled =
            self.settle_verdict(&prepared, actor, &op, request, expected, verdict.clone());
        let published = match &settled {
            Ok(_) | Err(PlanOpError::Refused { .. }) => Ok(verdict),
            Err(error) => Err(error.to_string()),
        };
        flight.publish(published);
        self.unflight(&prepared.token);
        settled
    }

    /// The flight is over: a later call for the token runs or replays, never joins.
    pub(super) fn unflight(&self, token: &VerificationToken) {
        if let Ok(mut flights) = self.in_flight.lock()
            && let Ok(digest) = token.digest()
        {
            flights.remove(&digest);
        }
    }

    /// Steps 1 and 2, under the lease.
    fn prepare(
        &self,
        plan: Option<PlanId>,
        actor: &Actor,
        op: &Op,
        request: RequestId,
        expected: Option<TouchCount>,
    ) -> Result<Phase1, PlanOpError> {
        let Op::Done { label, output } = op else {
            return Err(PlanOpError::NotJournaled { op: op.kind() });
        };
        let _lease = self.lease_waiting()?;
        let id = self.resolve(plan)?;
        let root = root_of(&id)?;
        let mut txn = self.begin(&root, actor, request, expected, false)?;
        if let Some(replayed) = self.replay(&txn, op)? {
            return Ok(Phase1::Plain(Box::new(replayed)));
        }
        self.check_revision(&txn, &id, expected)?;
        let current = txn.state.plan(&id)?;
        let todo = current
            .todo(label)
            .ok_or_else(|| PlanOpError::UnknownLabel {
                plan: id.clone(),
                label: label.clone(),
            })?;
        // Invariant: a worktree todo reaching the plain path is refused on the journal-backed
        // state, so no routing slip completes it without an `accepted` record (section 6.6).
        if super::acceptance::is_worktree(todo) {
            return Err(PlanOpError::AcceptanceUnavailable {
                label: label.clone(),
            });
        }
        let Some(contract) = todo.contract.clone() else {
            return Ok(Phase1::Plain(Box::new(
                self.settle(&mut txn, &id, &root, op)?,
            )));
        };
        let todos = u32::try_from(current.todos.len()).unwrap_or(u32::MAX);
        let attempt = todo.attempt;
        let owner_model = todo
            .delegation
            .as_ref()
            .and_then(|delegation| delegation.spec.model.clone());
        let convened = juries(&txn.records, label, current.version);
        match self.evidence(&txn, &id, label, output.as_ref(), &contract) {
            Ok((token, product)) => {
                if let Some(settled) = txn.state.settled_verification(&token)
                    && let Some(verdict) = &settled.verdict
                    && matches!(
                        verdict.outcome,
                        ContractOutcome::Fail | ContractOutcome::Escalate
                    )
                {
                    // A third done replays the committed verdict, charging nothing; an abstention
                    // decided nothing, so the same token runs the checks again (section 6.3).
                    return Err(PlanOpError::Refused {
                        label: label.clone(),
                        verdict: Box::new(verdict.clone()),
                    });
                }
                let pending = txn
                    .state
                    .pending_verification(&id, label, &token)
                    .map(|(effect, pending)| (effect.clone(), pending.clone()));
                let prepared = |effect, juries| {
                    Box::new(Prepared {
                        id: id.clone(),
                        root: root.clone(),
                        label: label.clone(),
                        token: token.clone(),
                        contract: contract.clone(),
                        product: product.clone(),
                        effect,
                        jury: (juries, owner_model.clone()),
                    })
                };
                if let Some((effect, pending)) = pending {
                    let digest = token.digest()?;
                    if let Some(flight) = self
                        .in_flight
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .get(&digest)
                    {
                        return Ok(Phase1::Join(Arc::clone(flight), id, label.clone()));
                    }
                    self.refuse_live_claim(label, &pending)?;
                    // Requested by a process that died or overran: this call adopts the effect.
                    let flight = self.flight_for(&token)?;
                    return Ok(Phase1::Run(prepared(effect, convened), flight));
                }
                let effect = EffectId::new(format!("e-{}", self.store.request_nonce())).map_err(
                    |error| PlanOpError::Verification {
                        label: label.clone(),
                        reason: error.to_string(),
                    },
                )?;
                let claim = Claim {
                    pid: std::process::id(),
                    at: self.store.now_ms(),
                };
                let mut record = draft(
                    &id,
                    KIND_VERIFICATION_REQUESTED,
                    txn.actor.clone(),
                    self.store.now_ms(),
                    todos,
                    json!({"label": label, "token": token, "effect_id": effect,
                           "claim": claim.to_value()}),
                    txn.derived("verify")?,
                    txn.expected,
                    Some(attempt),
                );
                record.record.todo = Some(label.clone());
                let committed = self.commit(&mut txn, record)?;
                self.store.checkpoint_family(&txn.state)?;
                self.emit(&committed);
                // Invariant: the flight is registered before the lease drops, so no caller can
                // see the committed effect with nothing in this process behind it.
                let flight = self.flight_for(&token)?;
                Ok(Phase1::Run(
                    prepared(effect, convened.saturating_add(1)),
                    flight,
                ))
            }
            Err(error) => {
                if error.is_recordable() {
                    self.record_refusal(&mut txn, &id, op, &error);
                }
                Err(error)
            }
        }
    }

    /// A pending effect another process claimed: refused while that process is alive and
    /// inside the verifier's deadline, adoptable after.
    pub(super) fn refuse_live_claim(
        &self,
        label: &TodoLabel,
        pending: &Verification,
    ) -> Result<(), PlanOpError> {
        let Some(claim) = pending.claim else {
            return Ok(());
        };
        let alive = yi_kernel::bootstrap::process_is_running(claim.pid).unwrap_or(true);
        let young = self.store.now_ms().saturating_sub(claim.at)
            <= self.verifier.timeout_ms().saturating_add(CLAIM_GRACE_MS);
        // ponytail: an adopted effect is not re-claimed, so two adopters of one dead claim can
        // both run the checker; step 5's replay guard still charges one refusal.
        if alive && young {
            return Err(PlanOpError::Verification {
                label: label.clone(),
                reason: format!(
                    "a verification of this token is in progress since {} by pid {}",
                    claim.at, claim.pid
                ),
            });
        }
        Ok(())
    }

    /// The token and the product bytes for the todo as it stands now (section 6.3, steps 1 and
    /// 5): the workspace is captured afresh each time, so one that moved reads as stale.
    fn evidence(
        &self,
        txn: &Txn,
        id: &PlanId,
        label: &TodoLabel,
        output: Option<&yi_types::url::Url>,
        contract: &yi_types::plan::contract::Contract,
    ) -> Result<(VerificationToken, Option<String>), PlanOpError> {
        let plan = txn.state.plan(id)?;
        check_plan_state(plan, OpKind::Done)?;
        locate_step(plan, label, OpKind::Done)?;
        let todo = plan.todo(label).ok_or_else(|| PlanOpError::UnknownLabel {
            plan: id.clone(),
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
        let product = check_output(self.output_resolve.as_deref(), plan, label, output)?;
        let needs_product = contract
            .items
            .iter()
            .any(|item| matches!(item.decider, Decider::Schema { .. }));
        if needs_product && product.is_none() {
            return Err(if output.is_none() {
                PlanOpError::OutputRequired {
                    label: label.clone(),
                }
            } else {
                PlanOpError::Verification {
                    label: label.clone(),
                    reason:
                        "no output resolver is attached; the schema item cannot read the product"
                            .to_owned(),
                }
            });
        }
        let snapshot =
            self.snapshotter
                .capture(&self.cwd)
                .map_err(|reason| PlanOpError::Verification {
                    label: label.clone(),
                    reason: format!("snapshot: {reason}"),
                })?;
        let token = VerificationToken {
            plan: id.clone(),
            version: plan.version,
            todo: label.clone(),
            attempt: todo.attempt,
            contract_digest: frozen,
            criteria_digest: contract.criteria_digest()?,
            output_digest: Digest::of(product.as_deref().unwrap_or("").as_bytes()),
            snapshot,
            integration: None,
        };
        Ok((token, product))
    }

    /// Two `done` calls may overlap by design (step 3 releases the lease), so the done path
    /// waits a bounded while for a live holder instead of refusing at once.
    pub(super) fn lease_waiting(&self) -> Result<super::store::Lease, PlanOpError> {
        let started = std::time::Instant::now();
        loop {
            match self.store.lease() {
                Ok(lease) => return Ok(lease),
                Err(super::store::StoreError::LeaseHeld { .. })
                    if started.elapsed() < LEASE_WAIT =>
                {
                    std::thread::sleep(LEASE_POLL);
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    pub(super) fn flight_for(&self, token: &VerificationToken) -> Result<Arc<Flight>, PlanOpError> {
        let digest = token.digest()?;
        let flight = Flight::new();
        self.in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(digest, Arc::clone(&flight));
        Ok(flight)
    }

    /// Step 4, no lease held: the verifier in `workspace_of(snapshot)`, the step 1 tree written
    /// under the temp dir, so the checker's writes never move the token the checkout is judged by.
    fn run_verifier(&self, prepared: &Prepared) -> Verdict {
        let artifacts = self.store.artifacts(&prepared.id);
        let workspace = Workspace(
            std::env::temp_dir().join(format!("yi-verify-{}", self.store.request_nonce())),
        );
        if let Err(reason) =
            self.snapshotter
                .materialize(&self.cwd, &prepared.token.snapshot, &workspace.0)
        {
            return self.verifier.abstained(
                &prepared.token,
                &prepared.contract,
                &format!("snapshot could not be materialized: {reason}"),
            );
        }
        // A jury sits in the verification reserve, never the worker share (section 7.6); a full
        // reserve abstains as infrastructure, and only a judged contract asks for it.
        let judged = prepared
            .contract
            .items
            .iter()
            .any(|item| matches!(item.decider, Decider::Judge { .. }));
        let permit = judged
            .then(|| self.reserve_verification(&prepared.label).ok())
            .flatten();
        let snapshot = Snapshot {
            id: &prepared.token.snapshot,
            root: &workspace.0,
            output: prepared.product.as_deref().map(str::as_bytes),
            artifacts: &artifacts,
            jury: permit.as_ref().map(|permit| super::verify::Seat {
                permit,
                juries: prepared.jury.0,
                owner_model: prepared.jury.1.as_deref(),
            }),
        };
        self.verifier
            .run(&prepared.token, &prepared.contract, &snapshot)
    }

    /// Steps 5 and 6, under the lease again.
    fn settle_verdict(
        &self,
        prepared: &Prepared,
        actor: &Actor,
        op: &Op,
        request: RequestId,
        expected: Option<TouchCount>,
        verdict: Verdict,
    ) -> Result<Outcome, PlanOpError> {
        let _lease = self.lease_waiting()?;
        let mut txn = self.begin(&prepared.root, actor, request, expected, false)?;
        let label = &prepared.label;
        if let Some(settled) = txn
            .state
            .verifications
            .get(&prepared.effect)
            .filter(|open| open.settled)
        {
            // Another process settled the effect this call adopted: its verdict stands and this
            // call charges nothing.
            return match settled.verdict.clone() {
                Some(verdict) if verdict.outcome == ContractOutcome::Pass => {
                    self.view(Some(prepared.id.clone()), true)
                }
                Some(verdict) => Err(PlanOpError::Refused {
                    label: label.clone(),
                    verdict: Box::new(verdict),
                }),
                None => Err(PlanOpError::Stale {
                    label: label.clone(),
                    token: Box::new(prepared.token.clone()),
                }),
            };
        }
        txn.effect = Some(prepared.effect.clone());
        let Op::Done { output, .. } = op else {
            return Err(PlanOpError::NotJournaled { op: op.kind() });
        };
        let now = self
            .evidence(
                &txn,
                &prepared.id,
                label,
                output.as_ref(),
                &prepared.contract,
            )
            .map(|(token, _)| token);
        if now.as_ref().ok() != Some(&prepared.token) {
            let detail = match &now {
                Ok(token) => stale_detail(&prepared.token, token),
                Err(error) => error.to_string(),
            };
            let mut record = draft(
                &prepared.id,
                KIND_VERIFICATION_STALE,
                txn.actor.clone(),
                self.store.now_ms(),
                u32::try_from(txn.state.plan(&prepared.id)?.todos.len()).unwrap_or(u32::MAX),
                json!({"label": label, "token": prepared.token, "effect_id": prepared.effect}),
                txn.derived("stale")?,
                txn.expected,
                Some(prepared.token.attempt),
            );
            record.record.todo = Some(label.clone());
            record.record.extra.insert(
                "refusal".to_owned(),
                json!({"code": "stale", "detail": detail}),
            );
            let committed = self.commit(&mut txn, record)?;
            self.store.checkpoint_family(&txn.state)?;
            self.emit(&committed);
            return Err(PlanOpError::Stale {
                label: label.clone(),
                token: Box::new(prepared.token.clone()),
            });
        }
        if verdict.outcome == ContractOutcome::Pass {
            txn.resolution = Some(Resolution::VerifiedDone);
            txn.verdict = Some(verdict);
            return self.settle(&mut txn, &prepared.id, &prepared.root, op);
        }
        self.refuse(&mut txn, prepared, op, verdict)
    }

    /// Step 6, the refusal: `done_refused` with the verdict, the counter bump the reducer
    /// applies from it, and the cap as its own committed `block`.
    pub(super) fn refuse(
        &self,
        txn: &mut Txn,
        prepared: &Prepared,
        op: &Op,
        verdict: Verdict,
    ) -> Result<Outcome, PlanOpError> {
        let label = &prepared.label;
        let error = PlanOpError::Refused {
            label: label.clone(),
            verdict: Box::new(verdict.clone()),
        };
        let mut record = draft(
            &prepared.id,
            KIND_DONE_REFUSED,
            txn.actor.clone(),
            self.store.now_ms(),
            u32::try_from(txn.state.plan(&prepared.id)?.todos.len()).unwrap_or(u32::MAX),
            op.args()?,
            txn.derived("refused")?,
            txn.expected,
            Some(prepared.token.attempt),
        );
        record.record.todo = Some(label.clone());
        // The header line only: the item lines are the verdict below, journaled once.
        let header = error
            .to_string()
            .lines()
            .next()
            .unwrap_or_default()
            .to_owned();
        record.record.extra.insert(
            "refusal".to_owned(),
            json!({"code": "refused", "detail": header}),
        );
        record.record.extra.insert(
            "effect_id".to_owned(),
            Value::String(prepared.effect.to_string()),
        );
        if let Some(votes) = juror_votes(&verdict) {
            record.record.extra.insert("jurors".to_owned(), votes);
        }
        let record = fitted(txn, record, &verdict)?;
        let committed = self.commit(txn, record)?;
        self.emit(&committed);
        // The cap counts refused verdicts on this attempt: a refused transition, a stale
        // token and an abstention are not verdicts on the product.
        let refused = refused_verdicts(&txn.records, label, prepared.token.attempt);
        // A jury cap reached is a question for the user at once, not after three more refusals.
        let escalated = verdict.outcome == ContractOutcome::Escalate;
        let refusal_cap = crate::levers::get().plan_done_refusal_cap;
        let capped = (escalated || refused >= refusal_cap)
            && txn
                .state
                .plan(&prepared.id)?
                .todo(label)
                .is_some_and(|todo| {
                    matches!(todo.state, TodoState::Running { .. } | TodoState::Pending)
                });
        if capped {
            txn.request = txn.derived("block")?;
            // An escalation arrives on its own refusal, so its note names the verdict that
            // asked for the user rather than a count of refusals that never happened.
            let note = if escalated {
                format!("a verdict escalated to you: {}", verdict.lines())
            } else {
                format!(
                    "{refusal_cap} refused verdicts; the last: {}",
                    verdict.lines()
                )
            };
            let block = Op::Block {
                label: label.clone(),
                on: BlockedOn::User,
                note,
            };
            self.transact(txn, &prepared.id, &prepared.root, &block)?;
        }
        self.store.checkpoint_family(&txn.state)?;
        Err(error)
    }

    /// A second `done` for a token this process is verifying: wait for that verdict and return
    /// it as if this call had run the checks.
    pub(super) fn joined(
        &self,
        flight: &Flight,
        id: PlanId,
        label: TodoLabel,
    ) -> Result<Outcome, PlanOpError> {
        let grace = Duration::from_millis(self.verifier.timeout_ms().saturating_add(5_000));
        match flight.wait(grace) {
            Some(Ok(verdict)) if verdict.outcome == ContractOutcome::Pass => {
                self.view(Some(id), true)
            }
            Some(Ok(verdict)) => Err(PlanOpError::Refused {
                label,
                verdict: Box::new(verdict),
            }),
            Some(Err(reason)) => Err(PlanOpError::Verification { label, reason }),
            None => Err(PlanOpError::Verification {
                label,
                reason: "the shared verification did not settle in time".to_owned(),
            }),
        }
    }
}

/// The materialized tree a verification ran in, removed with the run whatever it decided.
struct Workspace(std::path::PathBuf);

impl Drop for Workspace {
    fn drop(&mut self) {
        let _gone_or_never_made = std::fs::remove_dir_all(&self.0);
    }
}

/// Juror votes by outcome on the record a session sees; the lines stay in the journal's verdict.
pub(super) fn juror_votes(verdict: &Verdict) -> Option<Value> {
    let lines: Vec<_> = verdict.items.iter().flat_map(|item| &item.jurors).collect();
    let count = |vote| lines.iter().filter(|line| line.vote == vote).count();
    let unbacked = lines.iter().filter(|line| line.unbacked);
    (!lines.is_empty()).then(|| {
        json!({"pass": count(Vote::Pass), "fail": count(Vote::Fail),
               "abstain": count(Vote::Abstain), "unbacked": unbacked.count()})
    })
}

/// Verifications already requested on this todo at this plan version. Each one that reaches a
/// judge item seats a jury, so this is what the jury cap counts (plan section 6.4).
fn juries(
    records: &[JournalRecord],
    label: &TodoLabel,
    version: yi_types::plan::PlanVersion,
) -> u32 {
    let version = json!(version);
    let count = records
        .iter()
        .filter(|record| {
            record.record.op == KIND_VERIFICATION_REQUESTED
                && record.record.todo.as_ref() == Some(label)
                && record.args.pointer("/token/version") == Some(&version)
        })
        .count();
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// The `done_refused` records on this todo's attempt whose verdict decided something about the
/// product: an abstention is not one.
fn refused_verdicts(records: &[JournalRecord], label: &TodoLabel, attempt: AttemptId) -> u32 {
    let count = records
        .iter()
        .filter(|record| {
            record.record.op == KIND_DONE_REFUSED
                && record.record.todo.as_ref() == Some(label)
                && record.attempt == Some(attempt)
                && record
                    .verdict
                    .as_ref()
                    .and_then(|verdict| verdict.get("outcome"))
                    .and_then(Value::as_str)
                    != Some("abstain")
        })
        .count();
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// The administrative completions (plan sections 3.6 and 5.6), checked before any effect: a
/// `submit` is the running agent's own, and an `accept` lands as `AcceptedByUser`.
pub(super) fn admit(
    txn: &Txn,
    plan: &Plan,
    op: &Op,
    decided: &mut Decided,
) -> Result<(), PlanOpError> {
    match op {
        Op::Submit { label, .. } => {
            if let Some(TodoState::Running { by }) = plan.todo(label).map(|todo| &todo.state)
                && by.as_str() != txn.actor
            {
                return Err(PlanOpError::NotRunningBy {
                    label: label.clone(),
                    agent: txn.actor.clone(),
                });
            }
        }
        Op::Accept { .. } => decided.resolution = Some(Resolution::AcceptedByUser),
        _ => {}
    }
    Ok(())
}

/// Item details a `done_refused` record keeps once the full verdict does not fit the cap.
const DETAIL_CLIP_CHARS: usize = 256;

/// The record with its verdict rehearsed under the record cap: a checker's tail is attacker
/// text, so item details are clipped, then dropped, until the line fits and the refusal lands.
fn fitted(
    txn: &Txn,
    mut record: JournalRecord,
    verdict: &Verdict,
) -> Result<JournalRecord, PlanOpError> {
    for budget in [usize::MAX, DETAIL_CLIP_CHARS, 0] {
        record.verdict = Some(serde_json::to_value(clipped(verdict, budget))?);
        match txn.journal.rehearse(record.clone(), txn.last(), 0) {
            Ok(()) => return Ok(record),
            Err(JournalError::RecordOverCap { .. }) if budget > 0 => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(record)
}

fn clipped(verdict: &Verdict, budget: usize) -> Verdict {
    let clip = |text: &str| -> String {
        if text.chars().count() <= budget {
            return text.to_owned();
        }
        let kept: String = text.chars().take(budget).collect();
        format!("{kept} [clipped]")
    };
    let mut out = verdict.clone();
    for line in &mut out.items {
        line.verdict = match &line.verdict {
            ItemVerdict::Fail { detail } => ItemVerdict::Fail {
                detail: clip(detail),
            },
            ItemVerdict::Abstain { reason } => ItemVerdict::Abstain {
                reason: clip(reason),
            },
            ItemVerdict::Escalate { question } => ItemVerdict::Escalate {
                question: clip(question),
            },
            ItemVerdict::Pass => ItemVerdict::Pass,
        };
    }
    out
}

pub(super) fn stale_detail(was: &VerificationToken, now: &VerificationToken) -> String {
    let mut moved = Vec::new();
    if was.attempt != now.attempt {
        moved.push(format!(
            "the token names attempt {}; the todo is on attempt {}",
            was.attempt, now.attempt
        ));
    }
    if was.version != now.version {
        moved.push(format!(
            "the token names version {}; the plan is at {}",
            was.version.0, now.version.0
        ));
    }
    if was.contract_digest != now.contract_digest || was.criteria_digest != now.criteria_digest {
        moved.push("the contract or its criteria changed".to_owned());
    }
    if was.output_digest != now.output_digest {
        moved.push("the output changed".to_owned());
    }
    if was.snapshot != now.snapshot {
        moved.push("the workspace changed".to_owned());
    }
    if moved.is_empty() {
        moved.push("the token no longer matches".to_owned());
    }
    moved.join("; ")
}
