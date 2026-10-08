//! Exhaustive, hand-written mapping tables from the oracle's integers and names
//! to the harness's enums (#472).
//!
//! The integers are C++ enum positions (`static_cast<int>` in `src/CliRun.cpp`);
//! the names are the labels `phaseName`, `outcomeName` and `mutationName` render.
//! Each table returns `None` for a value it does not know, and the parser turns
//! that into a harness error, so a new C++ value can never be read as a
//! plausible but wrong fact. Keep each table in the C++ declaration order.

use crate::facts::{
    MutationKind, PhaseSkipReason, RunFailureCode, RunOutcome, RunPhase, SkipReason, StartError,
};

/// `cao::run::RunFailureCode`, 0–18 (`src/Run/RunLifecycle.h`).
pub fn run_failure_code(code: i64) -> Option<RunFailureCode> {
    use RunFailureCode::*;
    Some(match code {
        0 => SchedulingFailed,
        1 => RequestedWorkUnavailable,
        2 => PolicyConflict,
        3 => ConfigurationLoadingFailed,
        4 => ModSelectionResolutionFailed,
        5 => ConflictingModRoots,
        6 => TemporaryArtifactCleanupFailed,
        7 => SafetyCleanupServiceFailed,
        8 => StagingOwnershipUnverified,
        9 => StagingActive,
        10 => StagingRecoveryFailed,
        11 => ArchiveOrderMissing,
        12 => ArchiveOrderExtra,
        13 => ArchiveOrderDuplicate,
        14 => ArchiveOrderOutsideRoot,
        15 => ArchiveUnreadable,
        16 => ArchiveEntryInvalid,
        17 => ArchiveInsufficientCapacity,
        18 => WorkServiceFailed,
        _ => return None,
    })
}

/// `cao::routing::SkipReason`, 0–2 (`src/AssetRouting/AssetRouter.h`).
pub fn skip_reason(code: i64) -> Option<SkipReason> {
    use SkipReason::*;
    Some(match code {
        0 => DisabledPhase,
        1 => DisabledAssetKind,
        2 => ExcludedAssetVariant,
        _ => return None,
    })
}

/// `cao::run::StartError`, 0–2 (`src/Run/OptimizationRunService.h`).
pub fn start_error(code: i64) -> Option<StartError> {
    use StartError::*;
    Some(match code {
        0 => MissingProfileIdentity,
        1 => MissingModSelectionDirectory,
        2 => ActiveRun,
        _ => return None,
    })
}

/// The Run Phase labels of `phaseName`.
pub fn run_phase(name: &str) -> Option<RunPhase> {
    use RunPhase::*;
    Some(match name {
        "Preparing" => Preparing,
        "Discovering Archives" => DiscoveringArchives,
        "Extracting Archives" => ExtractingArchives,
        "Building the Effective Asset Tree" => BuildingEffectiveAssetTree,
        "Processing Assets" => ProcessingAssets,
        "Archive Finalization" => ArchiveFinalization,
        "Safety Cleanup" => SafetyCleanup,
        _ => return None,
    })
}

/// The Run Outcome labels of `outcomeName`.
pub fn run_outcome(name: &str) -> Option<RunOutcome> {
    use RunOutcome::*;
    Some(match name {
        "Succeeded" => Succeeded,
        "Completed With Failures" => CompletedWithFailures,
        "Cancelled" => Cancelled,
        "Failed" => Failed,
        _ => return None,
    })
}

/// The Phase Skip Reason labels `renderEvent` writes after `Skipped`.
pub fn phase_skip_reason(name: &str) -> Option<PhaseSkipReason> {
    use PhaseSkipReason::*;
    Some(match name {
        "No Requested Work" => NoRequestedWork,
        "Dry Run" => DryRun,
        _ => return None,
    })
}

/// The mutation kind labels of `mutationName`.
pub fn mutation_kind(name: &str) -> Option<MutationKind> {
    use MutationKind::*;
    Some(match name {
        "Archive Extraction" => ArchiveExtraction,
        "Asset Processing" => AssetProcessing,
        "Archive Finalization" => ArchiveFinalization,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    //! Each table is checked against an inverse written as an exhaustive
    //! `match`, so a variant added to a harness enum without a table entry
    //! fails to compile here, and every oracle value outside a table's range
    //! maps to nothing. The captured transcripts in `tests/oracle_parser.rs`
    //! pin the tables against the oracle's real output.

    use super::*;

    fn failure_code_of(code: RunFailureCode) -> i64 {
        use RunFailureCode::*;
        match code {
            SchedulingFailed => 0,
            RequestedWorkUnavailable => 1,
            PolicyConflict => 2,
            ConfigurationLoadingFailed => 3,
            ModSelectionResolutionFailed => 4,
            ConflictingModRoots => 5,
            TemporaryArtifactCleanupFailed => 6,
            SafetyCleanupServiceFailed => 7,
            StagingOwnershipUnverified => 8,
            StagingActive => 9,
            StagingRecoveryFailed => 10,
            ArchiveOrderMissing => 11,
            ArchiveOrderExtra => 12,
            ArchiveOrderDuplicate => 13,
            ArchiveOrderOutsideRoot => 14,
            ArchiveUnreadable => 15,
            ArchiveEntryInvalid => 16,
            ArchiveInsufficientCapacity => 17,
            WorkServiceFailed => 18,
        }
    }

    fn skip_reason_of(reason: SkipReason) -> i64 {
        match reason {
            SkipReason::DisabledPhase => 0,
            SkipReason::DisabledAssetKind => 1,
            SkipReason::ExcludedAssetVariant => 2,
        }
    }

    fn start_error_of(error: StartError) -> i64 {
        match error {
            StartError::MissingProfileIdentity => 0,
            StartError::MissingModSelectionDirectory => 1,
            StartError::ActiveRun => 2,
        }
    }

    fn phase_name_of(phase: RunPhase) -> &'static str {
        match phase {
            RunPhase::Preparing => "Preparing",
            RunPhase::DiscoveringArchives => "Discovering Archives",
            RunPhase::ExtractingArchives => "Extracting Archives",
            RunPhase::BuildingEffectiveAssetTree => "Building the Effective Asset Tree",
            RunPhase::ProcessingAssets => "Processing Assets",
            RunPhase::ArchiveFinalization => "Archive Finalization",
            RunPhase::SafetyCleanup => "Safety Cleanup",
        }
    }

    fn outcome_name_of(outcome: RunOutcome) -> &'static str {
        match outcome {
            RunOutcome::Succeeded => "Succeeded",
            RunOutcome::CompletedWithFailures => "Completed With Failures",
            RunOutcome::Cancelled => "Cancelled",
            RunOutcome::Failed => "Failed",
        }
    }

    fn skip_reason_name_of(reason: PhaseSkipReason) -> &'static str {
        match reason {
            PhaseSkipReason::NoRequestedWork => "No Requested Work",
            PhaseSkipReason::DryRun => "Dry Run",
        }
    }

    fn mutation_name_of(kind: MutationKind) -> &'static str {
        match kind {
            MutationKind::ArchiveExtraction => "Archive Extraction",
            MutationKind::AssetProcessing => "Asset Processing",
            MutationKind::ArchiveFinalization => "Archive Finalization",
        }
    }

    /// Checks a code table is a bijection onto exactly `0..count`.
    fn assert_code_table<T: Copy>(table: fn(i64) -> Option<T>, inverse: fn(T) -> i64, count: i64) {
        for code in 0..count {
            let value = table(code).unwrap_or_else(|| panic!("code {code} is unmapped"));
            assert_eq!(inverse(value), code);
        }
        for code in [-1, count, count + 1, i64::MAX] {
            assert!(table(code).is_none(), "code {code} must be unknown");
        }
    }

    #[test]
    fn code_tables_cover_exactly_the_oracle_ranges() {
        assert_code_table(run_failure_code, failure_code_of, 19);
        assert_code_table(skip_reason, skip_reason_of, 3);
        assert_code_table(start_error, start_error_of, 3);
    }

    /// Checks each name maps back to itself and near-miss spellings are unknown.
    fn assert_name_table<T: Copy>(
        table: fn(&str) -> Option<T>,
        inverse: fn(T) -> &'static str,
        names: &[&str],
    ) {
        for name in names {
            let value = table(name).unwrap_or_else(|| panic!("`{name}` is unmapped"));
            assert_eq!(inverse(value), *name);
            assert!(
                table(&name.to_lowercase()).is_none(),
                "names are case-sensitive"
            );
            assert!(table(&format!("{name} ")).is_none(), "names are exact");
        }
    }

    #[test]
    fn name_tables_cover_every_oracle_label() {
        assert_name_table(
            run_phase,
            phase_name_of,
            &[
                "Preparing",
                "Discovering Archives",
                "Extracting Archives",
                "Building the Effective Asset Tree",
                "Processing Assets",
                "Archive Finalization",
                "Safety Cleanup",
            ],
        );
        assert_name_table(
            run_outcome,
            outcome_name_of,
            &[
                "Succeeded",
                "Completed With Failures",
                "Cancelled",
                "Failed",
            ],
        );
        assert_name_table(
            phase_skip_reason,
            skip_reason_name_of,
            &["No Requested Work", "Dry Run"],
        );
        assert_name_table(
            mutation_kind,
            mutation_name_of,
            &[
                "Archive Extraction",
                "Asset Processing",
                "Archive Finalization",
            ],
        );
        // The C++ renderers' fallbacks for out-of-range values.
        assert!(run_phase("Unknown Phase").is_none());
        assert!(mutation_kind("Unknown Mutation").is_none());
    }
}
