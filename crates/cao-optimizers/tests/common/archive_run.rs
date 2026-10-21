//! What the whole-run Archive scenarios share (#595): the production Archive
//! seams each run's Toolset is built over, and the reading of a run's one
//! Archive extraction attempt.

use std::sync::Arc;

use cao_archive::{Game, Settings};
use cao_core::run::{ArchiveExtractionResult, ArchiveSeams, OptimizationRunResult, VolumeProbe};
use cao_core::test_support::SeamsBuilder;
use cao_optimizers::archives::{ArchiveFileReader, GameArchivePacker, VolumeProbes};

/// Builds each run's Archive seams over the production reader, the SSE
/// packer and the real volume probes, as production wires an SSE run.
pub fn production_seams() -> SeamsBuilder {
    production_seams_for(Game::Sse, Arc::new(VolumeProbes))
}

/// Builds each run's Archive seams over the production reader and `game`'s
/// packer, with `volume_probe` answering capacity and volume identity.
///
/// The reader and packer are made afresh for each run, as Preparing makes
/// them; `volume_probe` is shared by every run, so a scenario can keep a
/// clone to count or script its answers.
pub fn production_seams_for(game: Game, volume_probe: Arc<dyn VolumeProbe>) -> SeamsBuilder {
    Box::new(move || ArchiveSeams {
        reader: Arc::new(ArchiveFileReader::default()),
        volume_probe: Arc::clone(&volume_probe),
        packer: Arc::new(GameArchivePacker::new(Settings::get(game))),
    })
}

/// The run's one Archive extraction attempt.
///
/// # Panics
///
/// When the run made no attempt or several, such as when its preflight
/// failed.
pub fn only_attempt(result: &OptimizationRunResult) -> &ArchiveExtractionResult {
    let attempts = result.archive_extraction_attempts();
    assert_eq!(attempts.len(), 1, "{:?}", result.failures());
    &attempts[0]
}
