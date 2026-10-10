//! One planned output's atomic attempt.
//!
//! Ported from `src/Run/ArchiveFinalizationAttempt.cpp`: capacity recheck,
//! Archive write and no-replace publication, its Loading Plugin, then packed
//! source cleanup, with no cancellation inside.

use std::io::Read as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use super::planning::{FinalizationPlan, PlanningStop, estimate_packed_capacity};
use super::plugins::ensure_output_loading_plugin;
use super::{
    ArchiveFinalizationAttempt, ArchiveFinalizationFailure, ArchiveFinalizationMutation, Seams,
    capacity_shortfall,
};
use crate::execution::MutationState;
use crate::run::source_pin::{LoadingPluginPin, SourceDirectoryPins, SourceFilePin};
use crate::run::{
    PublicationPolicy, PublicationState, TemporaryArtifactRegistry, take_panic_message,
};

/// How far one attempt got, for classifying its failure.
struct AttemptProgress {
    /// The failure an error now reports.
    boundary: ArchiveFinalizationFailure,
    /// How many packed sources are already deleted.
    removed_sources: usize,
}

/// Runs output `index`'s atomic attempt, never failing outright: every
/// failure is reported in the returned attempt.
///
/// A capacity rejection returns `InsufficientCapacity` before any mutation.
/// `dummy_reserve` is the Loading Plugin allowance still needed on this
/// output's volume. The Archive's mutation and continuation verdict come from
/// its publication. After a commit, a later failure stays safe to continue
/// only while the Archive reads back and every source not yet deleted is
/// still readable recovery material; a Loading Plugin failure never is.
/// Plugin publication facts are appended to `mutations` as they happen, so
/// the caller keeps them even if the attempt then fails. A panic is an
/// unsafe `UnexpectedException` that keeps any committed mutation.
pub(super) fn attempt_output(
    plan: &FinalizationPlan,
    index: usize,
    artifacts: &mut TemporaryArtifactRegistry,
    seams: Seams<'_>,
    dummy_reserve: u64,
    mutations: &mut Vec<ArchiveFinalizationMutation>,
) -> ArchiveFinalizationAttempt {
    let output = &plan.outputs[index];
    let mut attempt =
        ArchiveFinalizationAttempt::new(output.archive_path.clone(), output.mod_root.clone());
    let mut progress = AttemptProgress {
        boundary: ArchiveFinalizationFailure::WriteFailed,
        removed_sources: 0,
    };
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        try_attempt(
            plan,
            index,
            artifacts,
            seams,
            dummy_reserve,
            mutations,
            &mut attempt,
            &mut progress,
        )
    }));
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(detail)) => {
            attempt.failure = Some(progress.boundary);
            attempt.detail = detail;
            // Publication or plugin errors leave sources intact. Once
            // committed, only a usable Archive and usable surviving sources
            // justify continuing, and verification can only confirm a
            // continuation the publication itself allowed.
            if attempt.mutation == MutationState::Committed && attempt.safe_to_continue {
                let verified = catch_unwind(AssertUnwindSafe(|| {
                    progress.boundary != ArchiveFinalizationFailure::PluginCreationFailed
                        && archive_reads_back(seams, &output.archive_path)
                        && output.archive.files[progress.removed_sources..]
                            .iter()
                            .all(|source| readable_packed_source(&source.path))
                }));
                // A failed verification keeps the original error and stops
                // later attempts.
                attempt.safe_to_continue = verified.unwrap_or_else(|payload| {
                    take_panic_message(payload);
                    false
                });
            }
        }
        Err(payload) => {
            attempt.failure = Some(ArchiveFinalizationFailure::UnexpectedException);
            attempt.detail = take_panic_message(payload);
            attempt.safe_to_continue = false;
        }
    }
    attempt
}

/// Performs the attempt, recording in `progress` which failure an `Err`
/// detail stands for. A capacity rejection is not an `Err`: it returns `Ok`
/// with the failure already set, since it is never a mutation.
#[expect(
    clippy::too_many_arguments,
    reason = "the attempt's inputs, plus the two records its caller classifies"
)]
fn try_attempt(
    plan: &FinalizationPlan,
    index: usize,
    artifacts: &mut TemporaryArtifactRegistry,
    seams: Seams<'_>,
    dummy_reserve: u64,
    mutations: &mut Vec<ArchiveFinalizationMutation>,
    attempt: &mut ArchiveFinalizationAttempt,
    progress: &mut AttemptProgress,
) -> Result<(), String> {
    let output = &plan.outputs[index];
    // Sources can grow after planning: re-stat them before any mutation,
    // keeping the frozen allowance if they shrank. Capacity is still only a
    // momentary sample.
    let current =
        estimate_packed_capacity(output, plan.rules.dummy_plugin.len() as u64, &|| Ok(()))
            .map_err(|stop| match stop {
                PlanningStop::Failed(detail) => detail,
                PlanningStop::Cancelled => unreachable!("the recheck never polls cancellation"),
            })?;
    let required = output
        .estimated_capacity_bytes
        .max(current)
        .saturating_add(dummy_reserve);
    if let Some(shortfall) = capacity_shortfall(seams.capacity, &output.mod_root, required) {
        attempt.failure = Some(ArchiveFinalizationFailure::InsufficientCapacity);
        attempt.detail = shortfall;
        return Ok(());
    }

    // The sources and their parents stay pinned from before the write until
    // each one's cleanup, so the Archive holds exactly the bytes deleted.
    let mut directory_pins = SourceDirectoryPins::default();
    let mut source_pins = Vec::new();
    if plan.delete_sources {
        source_pins.reserve(output.archive.files.len());
        for source in &output.archive.files {
            source_pins.push(
                SourceFilePin::with_shared_directories(
                    &source.path,
                    &output.mod_root,
                    &mut directory_pins,
                )
                .map_err(|error| error.to_string())?,
            );
        }
    }
    let receipt = artifacts
        .stage_archive_file_for_publication(&output.mod_root)
        .map_err(|error| error.to_string())?;
    let staged = receipt
        .path()
        .map_err(|error| error.to_string())?
        .to_path_buf();
    seams
        .packer
        .write(&output.archive, plan.compress, &output.mod_root, &staged)
        .map_err(|error| error.to_string())?;

    progress.boundary = ArchiveFinalizationFailure::CommitFailed;
    let publication = receipt.publish(&output.archive_path, PublicationPolicy::NoReplace);
    // Publication commits the Archive before its ownership is released.
    attempt.mutation = publication.mutation();
    if publication.state != PublicationState::PublishedAndReleased {
        // Publication's verdict stands; a later read-back cannot override a
        // release failure.
        attempt.safe_to_continue = publication.safe_to_continue();
        return Err(if publication.error_detail.is_empty() {
            "Archive publication did not complete.".to_owned()
        } else {
            publication.error_detail
        });
    }

    progress.boundary = ArchiveFinalizationFailure::PluginCreationFailed;
    // Held until every source is gone: the sources are deleted only because
    // the Archive loads through this plugin.
    let mut _loading_plugin_pin = None;
    if !output.loading_plugin_paths.is_empty() {
        let loading_plugin = ensure_output_loading_plugin(plan, output, artifacts, mutations)?;
        if plan.delete_sources {
            _loading_plugin_pin = Some(
                LoadingPluginPin::new(&loading_plugin, &output.mod_root)
                    .map_err(|error| error.to_string())?,
            );
        }
    }

    progress.boundary = ArchiveFinalizationFailure::SourceCleanupFailed;
    for pin in &mut source_pins {
        pin.release_for_cleanup();
        pin.remove_if_unchanged()
            .map_err(|error| error.to_string())?;
        progress.removed_sources += 1;
    }
    Ok(())
}

/// Whether the committed Archive still reads back through the reader, which
/// is then told to release it.
fn archive_reads_back(seams: Seams<'_>, archive: &Path) -> bool {
    let listed = seams.reader.list_entries(archive).is_ok();
    seams.reader.release();
    listed
}

/// Whether a retained source can still be read whole, without loading it all
/// into memory. A directory or a substituted link is not usable recovery
/// material, and neither is a file whose reads fail part way, as Windows
/// byte-range locks make them.
fn readable_packed_source(source: &Path) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(source) else {
        return false;
    };
    if !metadata.is_file() || metadata.is_symlink() {
        return false;
    }
    let Ok(mut file) = std::fs::File::open(source) else {
        return false;
    };
    let mut buffer = [0u8; 8192];
    let mut read = 0u64;
    loop {
        match file.read(&mut buffer) {
            Ok(0) => return read == metadata.len(),
            Ok(count) => read += count as u64,
            // An interrupted read read nothing; try the same read again.
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return false,
        }
    }
}
