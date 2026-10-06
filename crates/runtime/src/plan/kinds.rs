//! The class of each plan refusal, named in `details.errorKind` for the tool-failure census:
//! a misread, a stale view, a rule's verdict, a safety refusal, or Yi's own failure.

use yi_types::event::ToolErrorKind;

use super::ops::PlanOpError;
use super::store::StoreError;
use super::tool::{ArgError, PlanToolError};

impl PlanOpError {
    /// What the call met, for `details.errorKind`: misread (`InvalidArgs`), the model's view out of
    /// date (`Stale`), a rule's answer (`Verdict`), or Yi's own failure (`ToolError`).
    pub fn kind(&self) -> ToolErrorKind {
        match self {
            Self::NoPlan
            | Self::PlanExists { .. }
            | Self::UnknownLabel { .. }
            | Self::IllegalStep { .. }
            | Self::NotAPermutation { .. }
            | Self::UnmetEdge { .. }
            | Self::NotActive { .. }
            | Self::StaleRevision { .. }
            | Self::Stale { .. }
            | Self::WrongAttempt { .. }
            | Self::NotRunningBy { .. }
            | Self::Store(StoreError::Missing { .. }) => ToolErrorKind::Stale,
            Self::LabelNotUnique { .. }
            | Self::Invalid { .. }
            | Self::DepthExhausted { .. }
            | Self::SpawnCeilingExhausted { .. }
            | Self::RetriesExhausted { .. }
            | Self::AttemptsExhausted { .. }
            | Self::Admission(_)
            | Self::EphemeralTerminal { .. }
            | Self::MissingDeclaredOutput { .. }
            | Self::UnusableSchema { .. }
            | Self::OutputMismatch { .. }
            | Self::SubplanUndecided { .. }
            | Self::Refused { .. }
            | Self::ContractDrift { .. }
            | Self::PhaseMissing { .. }
            | Self::NoVerifiedCompletion { .. }
            | Self::OutputRequired { .. }
            | Self::Contract { .. } => ToolErrorKind::Verdict,
            Self::NotOwner { .. } | Self::AcceptanceUnavailable { .. } => ToolErrorKind::Denied,
            Self::UnknownState { .. } | Self::Doc(_) => ToolErrorKind::InvalidArgs,
            Self::SpawnFailed { .. }
            | Self::ReapFailed { .. }
            | Self::UnresolvedOutput { .. }
            | Self::RequestIdReused { .. }
            | Self::RecordedRefusal { .. }
            | Self::NeedsReconciliation { .. }
            | Self::StartWithoutSpawn { .. }
            | Self::NotJournaled { .. }
            | Self::NotReconcilable { .. }
            | Self::MergeFailed { .. }
            | Self::UnservedOutput { .. }
            | Self::UnservedSchema { .. }
            | Self::Verification { .. }
            | Self::Program { .. }
            | Self::Import(_)
            | Self::Serialize(_)
            | Self::Canonical(_)
            | Self::Reduce(_)
            | Self::Store(_) => ToolErrorKind::ToolError,
        }
    }
}

impl ArgError {
    pub(super) fn kind(&self) -> ToolErrorKind {
        match self {
            Self::LabelTooLong { .. } => ToolErrorKind::Verdict,
            Self::ActorArg | Self::ChildViews { .. } => ToolErrorKind::Denied,
            Self::NoOp
            | Self::UnknownOp { .. }
            | Self::Missing { .. }
            | Self::Malformed { .. }
            | Self::TodoShape { .. }
            | Self::Spec(_)
            | Self::Checklist { .. }
            | Self::EmptyList
            | Self::TooDeep { .. }
            | Self::Declared(_)
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
            Self::Submit(
                SubmitError::NoConfirmer { .. }
                | SubmitError::Declined { .. }
                | SubmitError::Expired { .. },
            ) => ToolErrorKind::Denied,
            Self::Submit(SubmitError::Citation(_)) => ToolErrorKind::ToolError,
        }
    }
}
