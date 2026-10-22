//! The Attempt Outcome rule, proved once over its public API (#600, #601).
//!
//! Every Asset, Archive extraction and Archive Finalization output attempt
//! embeds an [`AttemptOutcome`], so the rule is tested here instead of once
//! per attempt kind. Archive producers cannot, by design, produce a failed
//! partial outcome that claims safety, so the full table is not reachable
//! through a whole Optimization Run. The failure category is irrelevant to
//! the rule; the rows use an Asset category.

use cao_core::execution::{AssetExecutionFailure, MutationState};
use cao_core::{AttemptOutcome, CompletedMutation, OperationFailure};

/// One row: the outcome under test and the answers the rule must give.
struct Row {
    name: &'static str,
    outcome: AttemptOutcome<AssetExecutionFailure>,
    mutation: MutationState,
    succeeded: bool,
    safe_to_continue: bool,
}

/// A failed outcome with a fixed Operation Failure, so rows differ only in
/// mutation and the producer's continuation claim.
fn failed(mutation: MutationState, claimed_safe: bool) -> AttemptOutcome<AssetExecutionFailure> {
    AttemptOutcome::failed(
        OperationFailure::new(AssetExecutionFailure::SaveFailed, "save failed"),
        mutation,
        claimed_safe,
    )
}

#[test]
fn attempt_outcome_answers_follow_the_safe_to_continue_rule() {
    let rows = [
        Row {
            name: "succeeded with no mutation",
            outcome: AttemptOutcome::success(CompletedMutation::None),
            mutation: MutationState::None,
            succeeded: true,
            safe_to_continue: true,
        },
        Row {
            name: "succeeded with a Committed Mutation",
            outcome: AttemptOutcome::success(CompletedMutation::Committed),
            mutation: MutationState::Committed,
            succeeded: true,
            safe_to_continue: true,
        },
        Row {
            name: "failed, no mutation, claimed safe",
            outcome: failed(MutationState::None, true),
            mutation: MutationState::None,
            succeeded: false,
            safe_to_continue: true,
        },
        Row {
            name: "failed, no mutation, claimed unsafe",
            outcome: failed(MutationState::None, false),
            mutation: MutationState::None,
            succeeded: false,
            safe_to_continue: false,
        },
        Row {
            name: "failed, Committed Mutation, claimed safe",
            outcome: failed(MutationState::Committed, true),
            mutation: MutationState::Committed,
            succeeded: false,
            safe_to_continue: true,
        },
        Row {
            name: "failed, Committed Mutation, claimed unsafe",
            outcome: failed(MutationState::Committed, false),
            mutation: MutationState::Committed,
            succeeded: false,
            safe_to_continue: false,
        },
        Row {
            // The producer's claim cannot override partial or unknown mutation.
            name: "failed, partial or unknown mutation, claimed safe",
            outcome: failed(MutationState::PartialOrUnknown, true),
            mutation: MutationState::PartialOrUnknown,
            succeeded: false,
            safe_to_continue: false,
        },
        Row {
            name: "failed, partial or unknown mutation, claimed unsafe",
            outcome: failed(MutationState::PartialOrUnknown, false),
            mutation: MutationState::PartialOrUnknown,
            succeeded: false,
            safe_to_continue: false,
        },
    ];

    for row in rows {
        let outcome = &row.outcome;
        assert_eq!(
            outcome.succeeded(),
            row.succeeded,
            "{}: succeeded",
            row.name
        );
        assert_eq!(
            outcome.safe_to_continue(),
            row.safe_to_continue,
            "{}: safe to continue",
            row.name
        );
        assert_eq!(outcome.mutation(), row.mutation, "{}: mutation", row.name);

        let facts = outcome.facts();
        assert_eq!(
            facts.succeeded(),
            row.succeeded,
            "{}: facts succeeded",
            row.name
        );
        assert_eq!(
            facts.safe_to_continue(),
            row.safe_to_continue,
            "{}: facts safe to continue",
            row.name
        );
        assert_eq!(
            facts.mutation(),
            row.mutation,
            "{}: facts mutation",
            row.name
        );
    }
}

#[test]
fn only_a_failed_outcome_carries_its_operation_failure_and_detail() {
    let success: AttemptOutcome<AssetExecutionFailure> =
        AttemptOutcome::success(CompletedMutation::Committed);
    assert!(success.failure().is_none());

    let failure = failed(MutationState::Committed, true);
    let operation_failure = failure
        .failure()
        .expect("a failed outcome has an Operation Failure");
    assert_eq!(
        operation_failure.category(),
        AssetExecutionFailure::SaveFailed
    );
    assert_eq!(operation_failure.detail(), "save failed");
}
