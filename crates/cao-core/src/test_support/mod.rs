//! Fakes of `cao-core`'s seams shared by the run scenarios of `cao-core`,
//! `cao-optimizers` and `cao-gui` (#547).
//!
//! Compiled only with the `test-support` feature, which consumers enable
//! from `[dev-dependencies]` alone, so no fake ships in a release build.
//!
//! The scenarios port the C++ suites' intent, not their fixtures: each fake
//! here stands in for one ported seam trait, and fixture trees are real
//! directories the scenarios create themselves.
//!
//! [`ArchiveFinalizationEvidence`] is not a fake: it opens the real
//! finalization recorder over fresh evidence already in the Archive
//! Finalization phase, so that phase is tested at its own interface (#548).

mod archives;
mod backend;
mod cleanup;
mod configuration;
mod finalization;
mod observation;
mod scheduling;

pub use archives::{
    AfterWrite, ExtractHook, FakeArchivePacker, FakeArchiveReader, FakeCapacity, FakeEntry,
    FakeVolumes, FakeWrite, RootProbe, entry, fake_dummy_plugin, write_archive,
};
pub use backend::FakeBackend;
pub use cleanup::{CountingCleanup, ScriptedCleanup};
pub use configuration::{CallbackConfiguration, FixedConfiguration, Loader};
pub use finalization::ArchiveFinalizationEvidence;
pub use observation::{EventLog, ObservationHook, Observed, RecordingSink};
pub use scheduling::{
    CountingInlineScheduler, DeferredScheduler, ExhaustedScheduler, GatedScheduler, WorkerHook,
};
