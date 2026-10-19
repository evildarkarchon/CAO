//! The Optimization Run core of Cathedral Assets Optimizer.
//!
//! This crate ports the C++ `AssetRouting`, `AssetExecution` and `Run` modules.
//! Its seams are the C++ seams, ported one-to-one as object-safe traits (#468):
//! the Run Scheduler, Run Work Service, Safety Cleanup Service, Run
//! Configuration Provider, Run Observation Sink and work milestones, one Asset
//! Execution Backend, the archive reader, and the capacity and volume-identity
//! probes. A filesystem abstraction, a clock and the staging nonce are not
//! seams; tests reach faults through the seams plus real temporary directories.
//!
//! The crate needs no C++ toolchain, logs through the `log` facade (a run that
//! aborts for destroying its own worker also writes that diagnostic straight
//! to stderr, which no log sink would flush in time), and
//! keeps its domain records (`StartError`, `RunFailure`, `OperationFailure`
//! state, `RunOutcome`) as plain data. [`Error`] is the crate's error type for
//! the places where C++ threw.
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
