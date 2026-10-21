//! The Optimization Run core of Cathedral Assets Optimizer.
//!
//! This crate ports the C++ `AssetRouting`, `AssetExecution` and `Run` modules.
//! Its seams are object-safe traits (#468): the Run Scheduler, Safety Cleanup
//! Service, Run Configuration Provider and Run Observation Sink, the Run
//! Toolset's seams (the Asset attempt port, the archive reader and packer,
//! and the volume probe, over which core assembles
//! Archive extraction and Archive Finalization), and one Asset Execution
//! Backend. A filesystem abstraction, a clock and the staging nonce are not
//! seams; tests reach faults through the seams plus real temporary
//! directories.
//!
//! The crate needs no C++ toolchain, logs through the `log` facade (a run that
//! aborts for destroying its own worker also writes that diagnostic straight
//! to stderr, which no log sink would flush in time), and keeps its domain
//! records (`StartError`, `RunFailure`, `OperationFailure` state,
//! `RunOutcome`) as plain data. [`Error`] is the crate's error type for the
//! places where C++ threw.
//!
//! The `test-support` feature adds `test_support`, the fakes of these seams
//! that the run scenarios of this crate, `cao-optimizers` and `cao-gui`
//! share. Enable it only from `[dev-dependencies]`.

mod error;
pub mod execution;
pub mod routing;
pub mod run;
#[cfg(feature = "test-support")]
pub mod test_support;

pub use error::Error;
