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
//! [`ScriptedToolset`] scripts a run's Asset attempt results, and
//! [`ToolsetConfiguration`] returns a fresh Run Toolset over it to each run,
//! so a scenario drives the real Run Executor through its Toolset path
//! (#550). The Toolset is built through the production constructor, so the
//! run does core's real extraction and Archive Finalization over the
//! scripted fake reader, packer and probes (#581).
//! [`ToolsetConfiguration::sse`] provides the all-supported [`sse_profile`]
//! most scenarios configure, and [`execute_run`] runs a request through the
//! real Run Executor with the services a scenario supplies.
//!
//! [`ArchiveRunRequest`] builds an Apply request over one Mod Root that
//! requests Archive creation, with the profile and Archive work settings its
//! configuration supplies, so a scenario drives Archive extraction and
//! Archive Finalization through a whole Optimization Run (#596). It can
//! also select several Mod Roots, request other work, or give the profile
//! another Archive extension, and executes its run (#597).
//! [`SeamsConfiguration`] builds each run's Toolset over the Archive seams a
//! scenario's builder makes, so `cao-optimizers` can run the same way over
//! its production reader, packer and volume probes.

mod archive_run;
mod archives;
mod backend;
mod cleanup;
mod configuration;
mod observation;
mod scheduling;
mod toolset;

pub use archive_run::ArchiveRunRequest;
pub use archives::{
    AfterWrite, BeforeList, ExtractHook, FakeArchivePacker, FakeArchiveReader, FakeEntry,
    FakeVolumeProbe, FakeWrite, RootProbe, entry, fake_dummy_plugin, write_archive,
};
pub use backend::{FakeBackend, FakeBackendFactory, FinishedAttempt};
pub use cleanup::{CountingCleanup, ScriptedCleanup};
pub use configuration::{
    CallbackConfiguration, FixedConfiguration, Loader, sse_capabilities, sse_profile,
};
pub use observation::{EventLog, ObservationHook, Observed, RecordingSink};
pub use scheduling::{
    CountingInlineScheduler, DeferredScheduler, ExhaustedScheduler, GatedScheduler, WorkerHook,
};
pub use toolset::{
    AssetAdapterMaker, AssetAttemptScript, AssetAttempts, ProviderLoads, ScriptedToolset,
    SeamsBuilder, SeamsConfiguration, ToolsetConfiguration, execute_run,
};
