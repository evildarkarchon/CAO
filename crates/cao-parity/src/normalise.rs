//! The one normaliser both sides' raw facts go through (#472).
//!
//! Neither side normalises on its own: the oracle parser and the Rust driver
//! both emit raw [`RunFacts`], and this module removes everything that may
//! legitimately differ between two builds of one case. It drops the Run ID and
//! sequence numbers, makes paths relative to each side's case root, replaces
//! staging Run IDs and nonces with placeholders, and reduces the phase records
//! to one entry per traversed phase with its final progress tuple.
//!
//! Message text and list order are kept. The comparator ignores them, but
//! they decide whether a passing comparison is Identical or only Equivalent,
//! and they make a report readable.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::HarnessError;
use crate::facts::{
    ArchiveCollision, ArchiveFailure, AssetFailure, CommittedMutations, DetailedPath,
    PhaseSkipReason, PhaseStatus, Progress, RunEventPayload, RunFacts, RunFailureCode, RunOutcome,
    RunPhase, SkippedAssets, StartError,
};

/// One side's facts after normalisation.
// As with `RunFacts`, one value exists per side, so the size gap is harmless.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NormalisedFacts {
    StartError(StartError),
    Started(NormalisedRun),
}

/// A started run's facts after normalisation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalisedRun {
    pub outcome: RunOutcome,
    pub final_phase: RunPhase,
    pub cancellation_observed: bool,
    pub phases: Vec<PhaseFact>,
    pub mod_roots: Vec<String>,
    pub run_failures: Vec<RunFailureFact>,
    pub asset_failures: Vec<AssetFailure>,
    pub archive_failures: Vec<ArchiveFailure>,
    pub finalization_failure: Option<String>,
    pub archive_collisions: Vec<ArchiveCollision>,
    pub skipped_assets: Vec<SkippedAssets>,
    pub committed_mutations: Vec<CommittedMutations>,
    pub cleanup_failures: Vec<DetailedPath>,
    pub diagnostics: Vec<DiagnosticFact>,
}

/// One traversed Run Phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhaseFact {
    pub phase: RunPhase,
    pub skip_reason: Option<PhaseSkipReason>,
    pub final_progress: Option<Progress>,
}

/// A Run Failure with its published phase and code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunFailureFact {
    pub phase: RunPhase,
    pub code: RunFailureCode,
    pub detail: String,
    pub path: String,
}

/// A Run Diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticFact {
    pub phase: RunPhase,
    pub detail: String,
    pub path: String,
}

/// Normalises one side's raw facts against that side's case root.
///
/// `case_root` is the side's own copy of the case tree (`oracle/` or `rust/`).
/// Every reported path must lie inside it. Paths become `/`-separated and
/// relative to it, the root itself becomes `.`, and an empty path (a fact
/// about no path) stays empty. A path outside the root is a
/// [`HarnessError::PathOutsideCaseRoot`], because no relative spelling could
/// be compared with the other side's.
///
/// Also checks the side agrees with itself: the published `Failure` events
/// must be exactly the terminal result's Run Failures.
pub fn normalise(facts: &RunFacts, case_root: &Path) -> Result<NormalisedFacts, HarnessError> {
    let run = match facts {
        RunFacts::StartError(error) => return Ok(NormalisedFacts::StartError(*error)),
        RunFacts::Started(run) => run,
    };
    if run.run_id.is_empty() {
        return Err(HarnessError::InconsistentFacts(
            "the run has an empty Run ID".into(),
        ));
    }
    let root = CaseRoot::new(case_root)?;
    let path = |path: &str| -> Result<String, HarnessError> {
        Ok(staging_placeholders(&root.relative(path)?, &run.run_id))
    };
    let paths = |paths: &[String]| {
        paths
            .iter()
            .map(|each| path(each))
            .collect::<Result<Vec<_>, _>>()
    };

    let mut phases: Vec<PhaseFact> = Vec::new();
    let mut run_failures = Vec::new();
    let mut diagnostics = Vec::new();
    for event in &run.events {
        match &event.payload {
            RunEventPayload::Phase { phase, status } => {
                // Consecutive records of one phase describe one traversal of it.
                let fact = match phases.last_mut() {
                    Some(last) if last.phase == *phase => last,
                    _ => {
                        phases.push(PhaseFact {
                            phase: *phase,
                            skip_reason: None,
                            final_progress: None,
                        });
                        phases.last_mut().expect("just pushed")
                    }
                };
                match status {
                    PhaseStatus::Indeterminate => {}
                    PhaseStatus::Skipped(reason) => fact.skip_reason = Some(*reason),
                    PhaseStatus::Progress(progress) => fact.final_progress = Some(*progress),
                }
            }
            RunEventPayload::Diagnostic {
                phase,
                detail,
                path: at,
            } => diagnostics.push(DiagnosticFact {
                phase: *phase,
                detail: detail.clone(),
                path: path(at)?,
            }),
            RunEventPayload::Failure {
                phase,
                code,
                detail,
                path: at,
            } => run_failures.push(RunFailureFact {
                phase: *phase,
                code: *code,
                detail: detail.clone(),
                path: path(at)?,
            }),
        }
    }

    let terminal = &run.terminal;
    check_failures_agree(&run_failures, &terminal.run_failures, &path)?;

    Ok(NormalisedFacts::Started(NormalisedRun {
        outcome: terminal.outcome,
        final_phase: terminal.final_phase,
        cancellation_observed: terminal.cancellation_observed,
        phases,
        mod_roots: paths(&terminal.mod_roots)?,
        run_failures,
        asset_failures: terminal
            .asset_failures
            .iter()
            .map(|failure| {
                Ok(AssetFailure {
                    path: path(&failure.path)?,
                    affected_path: path(&failure.affected_path)?,
                    ..failure.clone()
                })
            })
            .collect::<Result<_, HarnessError>>()?,
        archive_failures: terminal
            .archive_failures
            .iter()
            .map(|failure| {
                Ok(ArchiveFailure {
                    archive_path: path(&failure.archive_path)?,
                    detail: failure.detail.clone(),
                })
            })
            .collect::<Result<_, HarnessError>>()?,
        finalization_failure: terminal.finalization_failure.clone(),
        archive_collisions: terminal
            .archive_collisions
            .iter()
            .map(|collision| {
                Ok(ArchiveCollision {
                    mod_root: path(&collision.mod_root)?,
                    // Already relative to the Mod Root; only its separators are normalised.
                    game_path: collision.game_path.replace('\\', "/"),
                    winning_archive: path(&collision.winning_archive)?,
                    loose_asset_wins: collision.loose_asset_wins,
                    shadowed_archives: paths(&collision.shadowed_archives)?,
                })
            })
            .collect::<Result<_, HarnessError>>()?,
        // The oracle omits zero counts; drop any the other side reports so both agree.
        skipped_assets: terminal
            .skipped_assets
            .iter()
            .filter(|skipped| skipped.count != 0)
            .cloned()
            .collect(),
        committed_mutations: terminal
            .committed_mutations
            .iter()
            .map(|mutation| {
                Ok(CommittedMutations {
                    mod_root: path(&mutation.mod_root)?,
                    ..mutation.clone()
                })
            })
            .collect::<Result<_, HarnessError>>()?,
        cleanup_failures: terminal
            .cleanup_failures
            .iter()
            .map(|failure| {
                Ok(DetailedPath {
                    detail: failure.detail.clone(),
                    path: path(&failure.path)?,
                })
            })
            .collect::<Result<_, HarnessError>>()?,
        diagnostics,
    }))
}

/// Checks the published Run Failures and the terminal ones are one multiset.
///
/// Both come from the same retained failure in C++ (`RunEvidence` publishes
/// each failure as it records it), so any disagreement means the side's
/// reporting is broken, and comparing either list alone could hide that.
fn check_failures_agree(
    published: &[RunFailureFact],
    terminal: &[DetailedPath],
    path: &dyn Fn(&str) -> Result<String, HarnessError>,
) -> Result<(), HarnessError> {
    let mut published: Vec<(&str, &str)> = published
        .iter()
        .map(|failure| (failure.detail.as_str(), failure.path.as_str()))
        .collect();
    let terminal_paths = terminal
        .iter()
        .map(|failure| Ok((failure.detail.as_str(), path(&failure.path)?)))
        .collect::<Result<Vec<_>, HarnessError>>()?;
    let mut terminal: Vec<(&str, &str)> = terminal_paths
        .iter()
        .map(|(detail, path)| (*detail, path.as_str()))
        .collect();
    published.sort_unstable();
    terminal.sort_unstable();
    if published == terminal {
        Ok(())
    } else {
        Err(HarnessError::InconsistentFacts(format!(
            "published Run Failures {published:?} differ from the terminal result's {terminal:?}"
        )))
    }
}

/// A side's case root in the generic spelling paths are matched against.
struct CaseRoot {
    generic: String,
}

impl CaseRoot {
    fn new(root: &Path) -> Result<Self, HarnessError> {
        let text = root
            .to_str()
            .ok_or_else(|| HarnessError::InvalidCase(format!("case root {root:?} is not UTF-8")))?;
        let mut generic = generic(text);
        while generic.ends_with('/') {
            generic.pop();
        }
        Ok(Self { generic })
    }

    /// Makes `path` relative to the root.
    ///
    /// The root prefix is matched ASCII case-insensitively, because Windows
    /// does and the two builds may spell a drive letter or folder differently.
    /// The rest of the path is kept exactly, so case differences inside the
    /// case tree stay visible to the comparator.
    fn relative(&self, path: &str) -> Result<String, HarnessError> {
        if path.is_empty() {
            return Ok(String::new());
        }
        let path = generic(path);
        let length = self.generic.len();
        let prefix_matches = path.is_char_boundary(length)
            && path.as_bytes()[..length].eq_ignore_ascii_case(self.generic.as_bytes());
        if prefix_matches {
            match &path[length..] {
                "" => return Ok(".".into()),
                rest if rest.starts_with('/') => return Ok(rest[1..].to_owned()),
                // A sibling such as `<root>-other`, not a child.
                _ => {}
            }
        }
        Err(HarnessError::PathOutsideCaseRoot {
            path,
            root: self.generic.clone(),
        })
    }
}

/// `/`-separated, without a `\\?\` verbatim prefix.
fn generic(path: &str) -> String {
    let path = path.replace('\\', "/");
    if let Some(unc) = path.strip_prefix("//?/UNC/") {
        format!("//{unc}")
    } else if let Some(local) = path.strip_prefix("//?/") {
        local.to_owned()
    } else {
        path
    }
}

/// Replaces a run's Run ID and staging nonces in a relative path with the
/// `{run-id}` and `{nonce}` placeholders, so two runs' staging names compare.
///
/// Only components shaped like CAO's own staging names are touched (see
/// `docs/architecture/staging-ownership.md`): the run child
/// `run-<Run ID>-<nonce>`, siblings named `.cao-staging…-<Run ID>-<nonce>…`,
/// and `archive-entry-<nonce>` files. A mod's own file names never change,
/// even if they happen to contain the Run ID.
pub fn staging_placeholders(relative: &str, run_id: &str) -> String {
    relative
        .split('/')
        .map(|component| staging_component(component, run_id))
        .collect::<Vec<_>>()
        .join("/")
}

const RUN_ID_PLACEHOLDER: &str = "{run-id}";
const NONCE_PLACEHOLDER: &str = "{nonce}";
const NONCE_LENGTH: usize = 32;

fn staging_component(component: &str, run_id: &str) -> String {
    if let Some(nonce) = component.strip_prefix("archive-entry-")
        && is_nonce(nonce)
    {
        return format!("archive-entry-{NONCE_PLACEHOLDER}");
    }
    let staging = component.len() >= ".cao-staging".len()
        && component.as_bytes()[..".cao-staging".len()].eq_ignore_ascii_case(b".cao-staging");
    if !staging && !component.starts_with("run-") {
        return component.to_owned();
    }
    let marked = format!("-{RUN_ID_PLACEHOLDER}-");
    let replaced = component.replace(&format!("-{run_id}-"), &marked);
    let Some(position) = replaced.find(&marked) else {
        return replaced;
    };
    let nonce_start = position + marked.len();
    let tail = &replaced[nonce_start..];
    // The nonce runs to the end of the name or to its extension.
    let nonce_end = tail.find('.').unwrap_or(tail.len());
    if is_nonce(&tail[..nonce_end]) {
        format!(
            "{}{NONCE_PLACEHOLDER}{}",
            &replaced[..nonce_start],
            &tail[nonce_end..]
        )
    } else {
        replaced
    }
}

/// 32 lowercase hexadecimal characters, the staging nonce's only spelling.
fn is_nonce(text: &str) -> bool {
    text.len() == NONCE_LENGTH
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}
