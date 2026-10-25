//! The class of each plan refusal, named in `details.errorKind` for the tool-failure census:
//! a misread, a stale view, a rule's verdict, a safety refusal, or Yi's own failure.

use yi_types::event::ToolErrorKind;

use super::ops::PlanOpError;
use super::store::StoreError;
use super::tool::{ArgError, PlanToolError};

impl PlanOpError {
    pub fn code(&self) -> &'static str {
        self.class().0
    }

    /// What the call met, for `details.errorKind`: misread (`InvalidArgs`), the model's view out of
    /// date (`Stale`), a rule's answer (`Verdict`), or Yi's own failure (`ToolError`).
    pub fn kind(&self) -> ToolErrorKind {
        self.class().1
    }

    /// `Refused` and `Stale` are recorded by the done path itself; an unserved product and a
    /// verification that could not run are the host's failures, so they charge no refusal.
    pub(super) fn is_recordable(&self) -> bool {
        self.class().2
    }

    fn class(&self) -> (&'static str, ToolErrorKind, bool) {
        use ToolErrorKind::{Denied, InvalidArgs, Stale, ToolError, Verdict};
        match self {
            Self::NoPlan => ("no_plan", Stale, true),
            Self::PlanExists { .. } => ("plan_exists", Stale, true),
            Self::UnknownLabel { .. } => ("unknown_label", Stale, true),
            Self::NotOwner { .. } => ("not_owner", Denied, true),
            Self::IllegalStep { .. } => ("illegal_step", Stale, true),
            Self::UnknownState { .. } => ("unknown_state", InvalidArgs, true),
            Self::LabelNotUnique { .. } => ("label_not_unique", Verdict, true),
            Self::Invalid { .. } => ("invalid", Verdict, true),
            Self::NotAPermutation { .. } => ("not_a_permutation", InvalidArgs, true),
            Self::DepthExhausted { .. } => ("depth_exhausted", Verdict, true),
            Self::SpawnCeilingExhausted { .. } => ("spawn_ceiling_exhausted", Verdict, true),
            Self::RetriesExhausted { .. } => ("retries_exhausted", Verdict, true),
            Self::AttemptsExhausted { .. } => ("attempts_exhausted", Verdict, true),
            Self::UnmetEdge { .. } => ("unmet_edge", Verdict, true),
            Self::Admission(_) => ("admission", Verdict, true),
            Self::NotActive { .. } => ("not_active", Stale, true),
            Self::SpawnFailed { .. } => ("spawn_failed", ToolError, true),
            Self::ReapFailed { .. } => ("reap_failed", ToolError, true),
            Self::EphemeralTerminal { .. } => ("ephemeral_terminal", Verdict, true),
            Self::MissingDeclaredOutput { .. } => ("missing_declared_output", Verdict, true),
            Self::UnresolvedOutput { .. } => ("unresolved_output", ToolError, true),
            Self::UnusableSchema { .. } => ("unusable_schema", Verdict, true),
            Self::OutputMismatch { .. } => ("output_mismatch", Verdict, true),
            Self::StaleRevision { .. } => ("stale_revision", Stale, true),
            Self::RequestIdReused { .. } => ("request_id_reused", ToolError, false),
            Self::RecordedRefusal { .. } => ("recorded_refusal", ToolError, false),
            Self::NeedsReconciliation { .. } => ("needs_reconciliation", ToolError, true),
            Self::StartWithoutSpawn { .. } => ("start_without_spawn", ToolError, true),
            Self::SubplanUndecided { .. } => ("subplan_undecided", Verdict, true),
            Self::NotJournaled { .. } => ("not_journaled", ToolError, true),
            Self::NotReconcilable { .. } => ("not_reconcilable", ToolError, true),
            Self::Refused { .. } => ("refused", Verdict, false),
            Self::Stale { .. } => ("stale", Stale, false),
            Self::ContractDrift { .. } => ("contract_drift", Verdict, true),
            Self::AcceptanceUnavailable { .. } => ("acceptance_unavailable", Denied, true),
            Self::PhaseMissing { .. } => ("phase_missing", Verdict, true),
            Self::MergeFailed { .. } => ("merge_failed", ToolError, false),
            Self::NoVerifiedCompletion { .. } => ("no_verified_completion", Verdict, true),
            Self::UnservedOutput { .. } => ("unserved_output", ToolError, false),
            Self::UnservedSchema { .. } => ("unserved_schema", ToolError, false),
            Self::OutputRequired { .. } => ("output_required", Verdict, true),
            Self::Contract { .. } => ("contract", Verdict, true),
            Self::Verification { .. } => ("verification", ToolError, false),
            Self::WrongAttempt { .. } => ("wrong_attempt", Stale, true),
            Self::NotRunningBy { .. } => ("not_running_by", Stale, true),
            Self::Program { .. } => ("program", ToolError, true),
            Self::Import(_) => ("import", ToolError, true),
            Self::Serialize(_) | Self::Canonical(_) => ("serialize", ToolError, false),
            Self::Reduce(_) => ("reduce", ToolError, false),
            Self::Store(StoreError::Missing { .. }) => ("store", Stale, false),
            Self::Store(_) => ("store", ToolError, false),
            Self::Doc(_) => ("doc", InvalidArgs, true),
        }
    }
}

impl ArgError {
    pub(super) fn kind(&self) -> ToolErrorKind {
        match self {
            Self::LeftOut { ruled, .. } if *ruled => ToolErrorKind::Verdict,
            Self::LeftOut { .. } => ToolErrorKind::InvalidArgs,
            Self::ChildViews { .. } => ToolErrorKind::Denied,
            Self::LabelTooLong { .. } | Self::EmptyList | Self::Unverifiable { .. } => {
                ToolErrorKind::Verdict
            }
            Self::ActorArg
            | Self::NoOp
            | Self::UnknownOp { .. }
            | Self::Missing { .. }
            | Self::Malformed { .. }
            | Self::TodoShape { .. }
            | Self::Spec(_)
            | Self::Checklist { .. }
            | Self::TooDeep { .. }
            | Self::Declared(_)
            | Self::NoRows { .. }
            | Self::UnknownKey { .. } => ToolErrorKind::InvalidArgs,
        }
    }
}

impl PlanToolError {
    pub(super) fn kind(&self) -> ToolErrorKind {
        use super::authority::SubmitError;
        match self {
            Self::Arg(error) | Self::Submit(SubmitError::Arg(error)) => error.kind(),
            Self::Op(error) | Self::Submit(SubmitError::Op(error)) => error.kind(),
            Self::Submit(SubmitError::NoConfirmer { .. } | SubmitError::Declined { .. }) => {
                ToolErrorKind::Denied
            }
            Self::Submit(SubmitError::Citation(_)) => ToolErrorKind::ToolError,
        }
    }
}
