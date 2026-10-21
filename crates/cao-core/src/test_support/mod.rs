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
//!
//! [`ScriptedToolset`] scripts a run's Asset attempt results, and
//! [`ToolsetConfiguration`] returns a fresh Run Toolset over it to each run,
//! so a scenario drives the real Run Executor through its Toolset path
//! (#550). The Toolset is built through the production constructor, so the
//! run does core's real extraction and Archive Finalization over the
//! scripted fake reader, packer and probes (#581).
//! [`ToolsetConfiguration::sse`] provides the all-supported [`sse_profile`]
//! most scenarios configure, and [`execute_run`] runs a request through the
//! real Run Executor with the services a scenario supplies.

mod archives;
mod backend;
mod cleanup;
mod configuration;
mod finalization;
mod observation;
mod scheduling;
mod toolset;

pub use archives::{
    AfterWrite, BeforeList, ExtractHook, FakeArchivePacker, FakeArchiveReader, FakeCapacity,
    FakeEntry, FakeVolumes, FakeWrite, RootProbe, entry, fake_dummy_plugin, write_archive,
};
pub use backend::{FakeBackend, FakeBackendFactory, FinishedAttempt};
pub use cleanup::{CountingCleanup, ScriptedCleanup};
pub use configuration::{
    CallbackConfiguration, FixedConfiguration, Loader, sse_capabilities, sse_profile,
};
pub use finalization::ArchiveFinalizationEvidence;
pub use observation::{EventLog, ObservationHook, Observed, RecordingSink};
pub use scheduling::{
    CountingInlineScheduler, DeferredScheduler, ExhaustedScheduler, GatedScheduler, WorkerHook,
};
pub use toolset::{
    AssetAdapterMaker, AssetAttemptScript, AssetAttempts, ScriptedToolset, ToolsetConfiguration,
    execute_run,
};
