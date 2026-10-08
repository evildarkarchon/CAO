//! The one fact comparator, serving both sides (#467, #472).
//!
//! It compares two [`NormalisedFacts`] under the run-fact rules:
//!
//! - **in order:** Run Outcome, final Run Phase, Cancellation Observed, the
//!   phase sequence with Phase Skip Reasons, and each phase's final progress
//!   tuple;
//! - **as multisets:** Mod Roots, Run Failures, Asset Failures, Archive
//!   Failures, Archive Collisions, Skip Reason counts, Committed Mutations
//!   Retained, cleanup failures and Run Diagnostics.
//!
//! Message text is never compared. Each multiset item is reduced to a key that
//! leaves it out, and the keys are what the diff shows. A phase-level
//! Finalization Failure is not a rule of its own: its effect shows in the Run
//! Outcome, and it only separates Identical from Equivalent.

use std::fmt;

use crate::normalise::{NormalisedFacts, NormalisedRun, PhaseFact};

/// A comparison's verdict.
///
/// `Identical` means the two sides agree on everything, including what the
/// rules ignore. `Equivalent` means they pass every rule but differ in
/// something a rule ignores, such as message text or list order. Both are
/// recorded, so drift towards "only equivalent" stays visible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict<D> {
    Identical,
    Equivalent,
    /// Fails the case, carrying every broken rule's difference.
    Different(Vec<D>),
}

impl<D> Verdict<D> {
    /// Whether the verdict passes the case.
    pub fn passed(&self) -> bool {
        !matches!(self, Verdict::Different(_))
    }

    /// The verdict's name, as reports and summaries print it.
    pub fn name(&self) -> &'static str {
        match self {
            Verdict::Identical => "Identical",
            Verdict::Equivalent => "Equivalent",
            Verdict::Different(_) => "Different",
        }
    }
}

/// A run-fact comparison rule, named in every difference it reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactRule {
    /// One side refused the request, or both did with different Start Errors.
    StartError,
    RunOutcome,
    FinalRunPhase,
    CancellationObserved,
    PhaseSequence,
    FinalProgress,
    ModRoots,
    RunFailures,
    AssetFailures,
    ArchiveFailures,
    ArchiveCollisions,
    SkipReasonCounts,
    CommittedMutationsRetained,
    CleanupFailures,
    RunDiagnostics,
}

impl fmt::Display for FactRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            FactRule::StartError => "Start Error",
            FactRule::RunOutcome => "Run Outcome",
            FactRule::FinalRunPhase => "Final Run Phase",
            FactRule::CancellationObserved => "Cancellation Observed",
            FactRule::PhaseSequence => "Phase Sequence",
            FactRule::FinalProgress => "Final Progress",
            FactRule::ModRoots => "Mod Roots",
            FactRule::RunFailures => "Run Failures",
            FactRule::AssetFailures => "Asset Failures",
            FactRule::ArchiveFailures => "Archive Failures",
            FactRule::ArchiveCollisions => "Archive Collisions",
            FactRule::SkipReasonCounts => "Skip Reason Counts",
            FactRule::CommittedMutationsRetained => "Committed Mutations Retained",
            FactRule::CleanupFailures => "Cleanup Failures",
            FactRule::RunDiagnostics => "Run Diagnostics",
        })
    }
}

/// One broken rule and the facts that broke it.
///
/// For an ordered rule, `oracle` and `rust` hold each side's whole value. For
/// a multiset rule, they hold only the items the other side lacks, so a
/// matching item never appears.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactDifference {
    pub rule: FactRule,
    pub oracle: Vec<String>,
    pub rust: Vec<String>,
}

impl fmt::Display for FactDifference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.rule)?;
        for (side, items) in [("oracle", &self.oracle), ("rust", &self.rust)] {
            if items.is_empty() {
                writeln!(f, "  {side}: (none)")?;
            }
            for item in items {
                writeln!(f, "  {side}: {item}")?;
            }
        }
        Ok(())
    }
}

/// Compares the oracle's normalised facts with the Rust side's.
pub fn compare_facts(oracle: &NormalisedFacts, rust: &NormalisedFacts) -> Verdict<FactDifference> {
    let differences = match (oracle, rust) {
        (NormalisedFacts::Started(oracle), NormalisedFacts::Started(rust)) => {
            compare_runs(oracle, rust)
        }
        _ => {
            let describe = |facts: &NormalisedFacts| match facts {
                NormalisedFacts::StartError(error) => format!("{error:?}"),
                NormalisedFacts::Started(run) => format!("started, {:?}", run.outcome),
            };
            let (oracle_text, rust_text) = (describe(oracle), describe(rust));
            if oracle_text == rust_text {
                Vec::new()
            } else {
                vec![FactDifference {
                    rule: FactRule::StartError,
                    oracle: vec![oracle_text],
                    rust: vec![rust_text],
                }]
            }
        }
    };
    if !differences.is_empty() {
        Verdict::Different(differences)
    } else if oracle == rust {
        Verdict::Identical
    } else {
        Verdict::Equivalent
    }
}

/// Applies every rule to two started runs, in the documented order.
fn compare_runs(oracle: &NormalisedRun, rust: &NormalisedRun) -> Vec<FactDifference> {
    let mut differences = Vec::new();
    // Each rule reduces both runs to keys with `key`; an ordered rule compares
    // the key lists as they are, a multiset rule ignores their order.
    let mut ordered = |rule, key: &dyn Fn(&NormalisedRun) -> Vec<String>| {
        let (oracle_keys, rust_keys) = (key(oracle), key(rust));
        if oracle_keys != rust_keys {
            differences.push(FactDifference {
                rule,
                oracle: oracle_keys,
                rust: rust_keys,
            });
        }
    };
    ordered(FactRule::RunOutcome, &|run| {
        vec![format!("{:?}", run.outcome)]
    });
    ordered(FactRule::FinalRunPhase, &|run| {
        vec![format!("{:?}", run.final_phase)]
    });
    ordered(FactRule::CancellationObserved, &|run| {
        vec![yes_no(run.cancellation_observed).into()]
    });
    ordered(FactRule::PhaseSequence, &|run| {
        run.phases.iter().map(phase_key).collect()
    });
    ordered(FactRule::FinalProgress, &|run| {
        run.phases.iter().map(progress_key).collect()
    });

    let mut multiset = |rule, key: &dyn Fn(&NormalisedRun) -> Vec<String>| {
        let (only_oracle, only_rust) = multiset_difference(key(oracle), key(rust));
        if !only_oracle.is_empty() || !only_rust.is_empty() {
            differences.push(FactDifference {
                rule,
                oracle: only_oracle,
                rust: only_rust,
            });
        }
    };
    multiset(FactRule::ModRoots, &|run| run.mod_roots.clone());
    multiset(FactRule::RunFailures, &|run| {
        let key = |f: &crate::normalise::RunFailureFact| {
            format!("phase={:?} code={:?} path={}", f.phase, f.code, f.path)
        };
        run.run_failures.iter().map(key).collect()
    });
    multiset(FactRule::AssetFailures, &|run| {
        let key = |f: &crate::facts::AssetFailure| {
            format!(
                "path={} operation={} affected={}",
                f.path, f.operation, f.affected_path
            )
        };
        run.asset_failures.iter().map(key).collect()
    });
    multiset(FactRule::ArchiveFailures, &|run| {
        let key = |f: &crate::facts::ArchiveFailure| format!("archive={}", f.archive_path);
        run.archive_failures.iter().map(key).collect()
    });
    multiset(FactRule::ArchiveCollisions, &|run| {
        let key = |c: &crate::facts::ArchiveCollision| {
            format!(
                "root={} game={} winner={} loose-asset-wins={} shadowed=[{}]",
                c.mod_root,
                c.game_path,
                c.winning_archive,
                yes_no(c.loose_asset_wins),
                c.shadowed_archives.join(", ")
            )
        };
        run.archive_collisions.iter().map(key).collect()
    });
    multiset(FactRule::SkipReasonCounts, &|run| {
        let key = |s: &crate::facts::SkippedAssets| format!("{:?}={}", s.reason, s.count);
        run.skipped_assets.iter().map(key).collect()
    });
    multiset(FactRule::CommittedMutationsRetained, &|run| {
        let key = |m: &crate::facts::CommittedMutations| {
            format!(
                "root={} kind={:?} committed={} partial-or-unknown={}",
                m.mod_root, m.kind, m.committed, m.partial_or_unknown
            )
        };
        run.committed_mutations.iter().map(key).collect()
    });
    multiset(FactRule::CleanupFailures, &|run| {
        run.cleanup_failures
            .iter()
            .map(|f| format!("path={}", f.path))
            .collect()
    });
    multiset(FactRule::RunDiagnostics, &|run| {
        let key =
            |d: &crate::normalise::DiagnosticFact| format!("phase={:?} path={}", d.phase, d.path);
        run.diagnostics.iter().map(key).collect()
    });
    differences
}

fn phase_key(phase: &PhaseFact) -> String {
    match phase.skip_reason {
        Some(reason) => format!("{:?} skipped={reason:?}", phase.phase),
        None => format!("{:?}", phase.phase),
    }
}

fn progress_key(phase: &PhaseFact) -> String {
    match phase.final_progress {
        Some(p) => format!(
            "{:?} {}/{} succeeded={} failed={}",
            phase.phase, p.completed, p.total, p.succeeded, p.failed
        ),
        None => format!("{:?} no progress", phase.phase),
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

/// Splits two multisets into the items only the first holds and the items
/// only the second holds, counting duplicates. Both results are sorted.
fn multiset_difference(mut a: Vec<String>, mut b: Vec<String>) -> (Vec<String>, Vec<String>) {
    a.sort_unstable();
    b.sort_unstable();
    let (mut only_a, mut only_b) = (Vec::new(), Vec::new());
    let (mut a, mut b) = (a.into_iter().peekable(), b.into_iter().peekable());
    loop {
        match (a.peek(), b.peek()) {
            (Some(x), Some(y)) if x == y => {
                a.next();
                b.next();
            }
            (Some(x), Some(y)) if x < y => only_a.push(a.next().expect("peeked")),
            (Some(_), Some(_)) => only_b.push(b.next().expect("peeked")),
            (Some(_), None) => only_a.push(a.next().expect("peeked")),
            (None, Some(_)) => only_b.push(b.next().expect("peeked")),
            (None, None) => return (only_a, only_b),
        }
    }
}
