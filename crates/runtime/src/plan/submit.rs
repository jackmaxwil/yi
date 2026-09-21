//! The running child's `submit` (plan section 6.6 rows one to three): the lane settled as the
//! candidate, verified on its own commit, then integrated and verified by `acceptance.rs`.

use std::marker::PhantomData;
use std::sync::{Arc, PoisonError};

use serde_json::json;
use yi_types::plan::contract::{Outcome as ContractOutcome, Verdict, VerificationToken};
use yi_types::plan::doc::{PlanId, TodoLabel, TodoState, TouchCount};
use yi_types::plan::ledger::{EffectId, RequestId};

use std::path::Path;

use super::acceptance::{
    Call, Candidate, Contracted, KIND_CANDIDATE_SUBMITTED, KIND_CANDIDATE_VERIFIED, Submitted,
    Verified, last_generation, verification,
};
use super::done::{Flight, Prepared};
use super::ops::{Actor, Op, Outcome, PlanEngine, PlanOpError};
use super::output::OutputResolve;
use super::state::{Decided, root_of};
use super::table::{OpKind, check_plan_state};
use super::verify::Snapshot;
use crate::lane::Sha;
use crate::lane::settle::generation_of;

/// What step 1 of a `submit` established under the lease.
struct SubmitRun {
    id: PlanId,
    root: PlanId,
    candidate: Candidate<Submitted>,
    contracted: Contracted,
    token: VerificationToken,
    effect: EffectId,
    flight: Arc<Flight>,
    /// The ordinal of the integration this submit prepares: one past the attempt's last.
    generation: u64,
}

/// A `local://` output read inside the candidate's own checkout, never the parent's, so the
/// token digests the candidate's bytes (section 6.6); every other scheme goes to the resolver.
struct LaneResolver<'a> {
    root: &'a Path,
    fallback: Option<&'a dyn OutputResolve>,
}

impl OutputResolve for LaneResolver<'_> {
    fn resolve(&self, url: &yi_types::url::Url) -> Result<Option<String>, String> {
        if *url.scheme() != yi_types::url::Scheme::Local {
            return self
                .fallback
                .map_or(Ok(None), |fallback| fallback.resolve(url));
        }
        let relative = Path::new(url.path().trim_start_matches('/'));
        if relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err(format!("{url} leaves the candidate checkout"));
        }
        std::fs::read_to_string(self.root.join(relative))
            .map(Some)
            .map_err(|error| format!("{url} in the candidate checkout: {error}"))
    }
}

enum Submission {
    Replayed(Box<Outcome>),
    /// A verification another `submit` in this process is running for the same token.
    Joined(Arc<Flight>, PlanId),
    Run(Box<SubmitRun>),
}

impl PlanEngine {
    /// Section 6.3 step 4 against a checkout of `commit` from the pool: the tree the token
    /// names, never the parent's, and never the child's lane.
    fn verify_commit(
        &self,
        id: &PlanId,
        label: &TodoLabel,
        token: &VerificationToken,
        contracted: &Contracted,
        commit: &Sha,
    ) -> Result<Verdict, PlanOpError> {
        let contract = &contracted.contract;
        let product = contracted.product.as_deref();
        let pool = self.pool(label)?;
        // The reserve, not the worker share (section 7.6): a full reserve waits its bounded
        // turn and then refuses as infrastructure, never as a verdict on the product.
        let _verification_slot = self.reserve_verification(label)?;
        let session = format!("verify-{}", self.store.nonce());
        let lane = match pool.claim(
            &session,
            crate::lane::ClaimBase::Commit(commit.as_str().to_owned()),
        ) {
            Ok(lane) => lane,
            Err(error) => {
                return Ok(self.verifier.abstained(
                    token,
                    contract,
                    &format!("no lane to verify in: {error}"),
                ));
            }
        };
        let artifacts = self.store.artifacts(id);
        let snapshot = Snapshot {
            id: &token.snapshot,
            root: lane.path(),
            output: product.map(str::as_bytes),
            artifacts: &artifacts,
            jury: None,
        };
        let verdict = self.verifier.run(token, contract, &snapshot);
        lane.discard()
            .map_err(|error| verification(label, format!("verification lane: {error}")))?;
        Ok(verdict)
    }

    /// Section 6.6 rows one to three on the running agent's `submit`: settled quiescent,
    /// verified on its own commit, integrated in a staging lane and verified there.
    pub(super) fn submit_candidate(
        &self,
        plan: Option<PlanId>,
        actor: &Actor,
        op: Op,
        request: RequestId,
        expected: Option<TouchCount>,
    ) -> Result<Outcome, PlanOpError> {
        let Op::Submit { label, .. } = &op else {
            return Err(PlanOpError::NotJournaled { op: op.kind() });
        };
        let run = match self.submit_prepare(plan, actor, &op, request.clone(), expected)? {
            Submission::Replayed(outcome) => return Ok(*outcome),
            Submission::Joined(flight, id) => return self.joined(&flight, id, label.clone()),
            Submission::Run(run) => *run,
        };
        let verdict = match self.verify_commit(
            &run.id,
            label,
            &run.token,
            &run.contracted,
            &run.candidate.commit,
        ) {
            Ok(verdict) => verdict,
            Err(error) => {
                run.flight.publish(Err(error.to_string()));
                self.unflight(&run.token);
                return Err(error);
            }
        };
        let verified = self.settle_candidate(&run, actor, &op, &request, expected, &verdict);
        run.flight.publish(match &verified {
            Ok(_) | Err(PlanOpError::Refused { .. }) => Ok(verdict),
            Err(error) => Err(error.to_string()),
        });
        self.unflight(&run.token);
        let verified = verified?;
        let call = Call {
            actor,
            op: &op,
            request,
            expected,
        };
        self.integrate(&run.id, &run.root, &call, verified, run.generation)?;
        self.view(Some(run.id), true)
    }

    /// Steps 1 and 2 of section 6.3 for the candidate, under the lease: committed, recorded,
    /// and one verification effect for its token shared with every `submit` of that token.
    fn submit_prepare(
        &self,
        plan: Option<PlanId>,
        actor: &Actor,
        op: &Op,
        request: RequestId,
        expected: Option<TouchCount>,
    ) -> Result<Submission, PlanOpError> {
        let Op::Submit {
            label,
            attempt,
            output,
        } = op
        else {
            return Err(PlanOpError::NotJournaled { op: op.kind() });
        };
        let _lease = self.lease_waiting()?;
        let id = self.resolve(plan)?;
        let root = root_of(&id)?;
        let mut txn = self.begin(&root, actor, request, expected, false)?;
        if let Some(replayed) = self.replay(&txn, op)? {
            return Ok(Submission::Replayed(Box::new(replayed)));
        }
        self.check_revision(&txn, &id, expected)?;
        let current = txn.state.plan(&id)?;
        check_plan_state(current, OpKind::Submit)?;
        let todo = current
            .todo(label)
            .ok_or_else(|| PlanOpError::UnknownLabel {
                plan: id.clone(),
                label: label.clone(),
            })?;
        if todo.attempt != *attempt {
            return Err(PlanOpError::WrongAttempt {
                label: label.clone(),
                named: *attempt,
                current: todo.attempt,
            });
        }
        let TodoState::Running { by } = &todo.state else {
            return Err(PlanOpError::IllegalStep {
                label: label.clone(),
                from: yi_types::plan::doc::TodoStateName::of(&todo.state),
                op: OpKind::Submit,
            });
        };
        let by = by.clone();
        super::done::admit(&txn, current, op, &mut Decided::default())?;
        // The lane read first: the output is resolved in the candidate's checkout, and nothing
        // is marked on the host by a submit (the disposition is `done` or `fail`'s to journal).
        let held = self
            .delegate
            .candidate(&by)
            .map_err(|reason| verification(label, reason))?
            .ok_or_else(|| verification(label, format!("the host holds no lane for {by}")))?;
        let resolver = LaneResolver {
            root: &held.path,
            fallback: self.output_resolve.as_deref(),
        };
        let contracted = self.contracted_via(&txn, &id, label, Some(output), Some(&resolver))?;
        let parent_base = generation_of(&self.cwd, self.verifier.deadline())
            .map_err(|error| verification(label, format!("parent checkout: {error}")))?
            .base;
        let (settled, quiescence) = (held.candidate, held.quiescence);
        let generation = last_generation(&txn.records, &id, label, *attempt).saturating_add(1);
        let candidate = Candidate::<Submitted> {
            label: label.clone(),
            attempt: *attempt,
            branch: settled.branch.as_str().to_owned(),
            commit: settled.commit.clone(),
            parent_base,
            outputs: vec![output.clone()],
            token: None,
            _phase: PhantomData,
        };
        let token = self.token(&txn, &id, label, &contracted, &candidate.commit, None)?;
        // A second submit of the token returns or awaits the same verification: a settled
        // refusal replays uncharged, a flight is joined, a dead claim is adopted (6.3 step 2).
        if let Some(verdict) = txn
            .state
            .settled_verification(&token)
            .and_then(|settled| settled.verdict.clone())
            .filter(|verdict| {
                matches!(
                    verdict.outcome,
                    ContractOutcome::Fail | ContractOutcome::Escalate
                )
            })
        {
            return Err(PlanOpError::Refused {
                label: label.clone(),
                verdict: Box::new(verdict),
            });
        }
        let pending = txn
            .state
            .pending_verification(&id, label, &token)
            .map(|(effect, pending)| (effect.clone(), pending.clone()));
        let effect = match pending {
            Some((effect, pending)) => {
                if let Some(flight) = self
                    .in_flight
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(&token.digest()?)
                {
                    return Ok(Submission::Joined(Arc::clone(flight), id));
                }
                self.refuse_live_claim(label, &pending)?;
                effect
            }
            None => {
                // The op record first: the submitted url stands on the todo whatever follows.
                let _submitted_url_stands_on_the_todo = self.transact(&mut txn, &id, &root, op)?;
                self.record(
                    &mut txn,
                    &id,
                    (label, *attempt),
                    KIND_CANDIDATE_SUBMITTED,
                    json!({
                        "label": label,
                        "branch": candidate.branch,
                        "candidate": candidate.commit.as_str(),
                        "parent_base": candidate.parent_base.as_str(),
                        "outputs": candidate.outputs,
                        "quiescent": {"at": quiescence.at, "running_commands": quiescence.running_commands()},
                    }),
                    None,
                )?;
                let effect = self.request_verification(&mut txn, &id, label, &token)?;
                self.store.checkpoint_family(&txn.state)?;
                effect
            }
        };
        // Invariant: the flight is registered before the lease drops, so no caller can see
        // the committed effect with nothing in this process behind it.
        let flight = self.flight_for(&token)?;
        Ok(Submission::Run(Box::new(SubmitRun {
            id,
            root,
            candidate,
            contracted,
            token,
            effect,
            flight,
            generation,
        })))
    }

    /// Steps 5 and 6 for the candidate, under the lease again: the verdict recorded as
    /// `candidate_verified`, or the refusal charged.
    fn settle_candidate(
        &self,
        run: &SubmitRun,
        actor: &Actor,
        op: &Op,
        request: &RequestId,
        expected: Option<TouchCount>,
        verdict: &Verdict,
    ) -> Result<Candidate<Verified>, PlanOpError> {
        let label = &run.candidate.label;
        let _lease = self.lease_waiting()?;
        let mut txn = self.begin(&run.root, actor, request.clone(), expected, false)?;
        if verdict.outcome != ContractOutcome::Pass {
            let prepared = Prepared {
                id: run.id.clone(),
                root: run.root.clone(),
                label: label.clone(),
                token: run.token.clone(),
                contract: run.contracted.contract.clone(),
                product: run.contracted.product.clone(),
                effect: run.effect.clone(),
                jury: (0, None),
            };
            return match self.refuse(&mut txn, &prepared, op, verdict.clone()) {
                Ok(_) => Err(verification(label, "a refusal returned an outcome")),
                Err(error) => Err(error),
            };
        }
        self.record(
            &mut txn,
            &run.id,
            (label, run.candidate.attempt),
            KIND_CANDIDATE_VERIFIED,
            json!({"label": label, "effect_id": run.effect, "token": run.token}),
            Some((verdict, &run.effect)),
        )?;
        Ok(run.candidate.clone().verified(run.token.clone()))
    }
}
