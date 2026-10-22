//! The Attempt Outcome shared by every Asset, Archive extraction and Archive
//! Finalization output attempt (#600).
//!
//! An [`AttemptOutcome`] is what one completed attempt left behind: its
//! mutation state, its [`OperationFailure`] if it failed, and whether the
//! Optimization Run can safely continue. The single safe-to-continue rule
//! lives here and nowhere else: a succeeded outcome is always safe to
//! continue, and a failed one is safe only when its producer claimed so and
//! its mutation is not partial or unknown.
//!
//! The module sits at the crate root beside Asset execution, not under the run
//! module, because Asset execution sits at the root and the run module depends
//! on it. The three-state [`MutationState`] stays in Asset execution.

use crate::execution::MutationState;

/// The mutation a succeeded attempt can leave behind: none, or a Committed
/// Mutation. Partial or unknown mutation has no variant here, so a partial or
/// unknown success cannot be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CompletedMutation {
    /// The attempt changed no durable Asset.
    #[default]
    None,
    /// The attempt completed a Committed Mutation.
    Committed,
}

impl From<CompletedMutation> for MutationState {
    fn from(mutation: CompletedMutation) -> Self {
        match mutation {
            CompletedMutation::None => MutationState::None,
            CompletedMutation::Committed => MutationState::Committed,
        }
    }
}

/// Why one Asset or Archive attempt did not complete: a stable category of the
/// attempt kind's own failure enum, with its presentable detail.
///
/// Its mutation state and continuation belong to the enclosing
/// [`AttemptOutcome`], since a failed attempt may still have committed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OperationFailure<F> {
    category: F,
    detail: String,
}

impl<F> OperationFailure<F> {
    /// An Operation Failure of `category`, with the presentable `detail` that
    /// explains it.
    pub fn new(category: F, detail: impl Into<String>) -> Self {
        Self {
            category,
            detail: detail.into(),
        }
    }

    /// The presentable detail of this Operation Failure.
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl<F: Copy> OperationFailure<F> {
    /// The stable failure category, never parsed from the detail.
    pub fn category(&self) -> F {
        self.category
    }
}

/// What one completed Asset, Archive extraction or Archive Finalization output
/// attempt left behind, generic over the attempt kind's failure category `F`.
///
/// A success can only carry a [`CompletedMutation`], and only a failure
/// carries an [`OperationFailure`], so "succeeded but partial" and "succeeded
/// but unsafe" are unrepresentable. Read the verdict through
/// [`safe_to_continue`](Self::safe_to_continue), never through the producer's
/// raw claim.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AttemptOutcome<F> {
    /// The attempt completed; it is always safe to continue.
    Succeeded {
        /// No mutation, or a Committed Mutation.
        mutation: CompletedMutation,
    },
    /// The attempt is an Operation Failure.
    Failed {
        /// Why the attempt did not complete.
        failure: OperationFailure<F>,
        /// The durable mutation known when the attempt ended, which may be
        /// committed or partial or unknown.
        mutation: MutationState,
        /// The producer's own continuation claim. Partial or unknown mutation
        /// overrides it; see [`AttemptOutcome::safe_to_continue`].
        claimed_safe_to_continue: bool,
    },
}

impl<F> AttemptOutcome<F> {
    /// A succeeded Attempt Outcome that left `mutation` behind.
    pub fn success(mutation: CompletedMutation) -> Self {
        Self::Succeeded { mutation }
    }

    /// A failed Attempt Outcome: `failure` explains it, `mutation` is the
    /// durable mutation known when the attempt ended, and
    /// `claimed_safe_to_continue` is the producer's own continuation claim,
    /// kept apart from the verdict.
    pub fn failed(
        failure: OperationFailure<F>,
        mutation: MutationState,
        claimed_safe_to_continue: bool,
    ) -> Self {
        Self::Failed {
            failure,
            mutation,
            claimed_safe_to_continue,
        }
    }

    /// Whether the attempt completed.
    pub fn succeeded(&self) -> bool {
        matches!(self, Self::Succeeded { .. })
    }

    /// Whether the Optimization Run can safely continue after this attempt.
    ///
    /// This is the only place the rule is written: a succeeded outcome is
    /// always safe, and a failed one is safe only when its producer claimed so
    /// and its mutation is not partial or unknown.
    pub fn safe_to_continue(&self) -> bool {
        match self {
            Self::Succeeded { .. } => true,
            Self::Failed {
                mutation,
                claimed_safe_to_continue,
                ..
            } => *claimed_safe_to_continue && *mutation != MutationState::PartialOrUnknown,
        }
    }

    /// The Operation Failure of a failed outcome, or `None` after success.
    pub fn failure(&self) -> Option<&OperationFailure<F>> {
        match self {
            Self::Succeeded { .. } => None,
            Self::Failed { failure, .. } => Some(failure),
        }
    }

    /// The durable mutation the attempt left behind, as the three-state
    /// [`MutationState`].
    pub fn mutation(&self) -> MutationState {
        match self {
            Self::Succeeded { mutation } => (*mutation).into(),
            Self::Failed { mutation, .. } => *mutation,
        }
    }

    /// The facts of this outcome that do not depend on the failure category,
    /// so attempts of different kinds can be read together.
    pub fn facts(&self) -> AttemptFacts {
        AttemptFacts {
            mutation: self.mutation(),
            succeeded: self.succeeded(),
            safe_to_continue: self.safe_to_continue(),
        }
    }
}

/// A copyable view of one [`AttemptOutcome`] that does not depend on its
/// failure category: its mutation state, whether it succeeded, and whether
/// the Optimization Run can safely continue after it.
///
/// It is only built from an Attempt Outcome, so it always agrees with the
/// safe-to-continue rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AttemptFacts {
    mutation: MutationState,
    succeeded: bool,
    safe_to_continue: bool,
}

impl AttemptFacts {
    /// The durable mutation the attempt left behind.
    pub fn mutation(&self) -> MutationState {
        self.mutation
    }

    /// Whether the attempt completed.
    pub fn succeeded(&self) -> bool {
        self.succeeded
    }

    /// The Attempt Outcome's safe-to-continue verdict.
    pub fn safe_to_continue(&self) -> bool {
        self.safe_to_continue
    }
}
