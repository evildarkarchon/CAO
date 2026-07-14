//! Application-owned contracts for bounded per-run effects.

use crate::{ActiveProfileId, PortFailure};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Maximum number of completed run logs retained in the portable state tree.
pub const MAX_RETAINED_RUN_LOGS: usize = 100;
/// Maximum aggregate bytes retained across completed run logs.
pub const MAX_RETAINED_RUN_LOG_BYTES: u64 = 100 * 1024 * 1024;
/// Maximum bytes accepted by one active run log before further records are rejected.
pub const MAX_RUN_LOG_BYTES: u64 = MAX_RETAINED_RUN_LOG_BYTES;
/// Default maximum bytes retained independently from helper stdout and stderr.
pub const DEFAULT_PROCESS_OUTPUT_BYTES: usize = 64 * 1024;

/// Stable identity for one processing run.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RunId(u128);

impl RunId {
    /// Creates a run identity from its stable numeric representation.
    #[must_use]
    pub const fn new(value: u128) -> Self {
        Self(value)
    }

    /// Returns the stable numeric representation of this run identity.
    #[must_use]
    pub const fn get(self) -> u128 {
        self.0
    }
}

/// Milliseconds since the Unix epoch observed through the run clock.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TimestampMillis(u64);

impl TimestampMillis {
    /// Creates a timestamp from a non-negative Unix-epoch millisecond count.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the non-negative Unix-epoch millisecond count.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Immutable inputs needed to create fresh effects for one processing run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RunEnvironmentRequest {
    profile_id: ActiveProfileId,
    dry_run: bool,
}

impl RunEnvironmentRequest {
    /// Creates a request for the active profile and frozen dry-run policy.
    #[must_use]
    pub const fn new(profile_id: ActiveProfileId, dry_run: bool) -> Self {
        Self {
            profile_id,
            dry_run,
        }
    }

    /// Returns the profile identity captured by the run plan.
    #[must_use]
    pub const fn profile_id(self) -> ActiveProfileId {
        self.profile_id
    }

    /// Reports whether asset mutations must be audited instead of applied.
    #[must_use]
    pub const fn dry_run(self) -> bool {
        self.dry_run
    }
}

/// Thread-safe source of fresh worker-owned run capabilities.
pub trait RunEnvironmentFactory: Send + Sync + 'static {
    /// Creates a unique scratch directory, run log, and fresh effect sessions.
    ///
    /// # Errors
    ///
    /// Returns a stable failure when any required run capability cannot be initialized.
    fn create(
        &self,
        request: RunEnvironmentRequest,
    ) -> Result<Box<dyn RunEnvironment>, PortFailure>;
}

/// Application-owned leaf capability for one-step same-volume file publication.
pub trait AtomicFilePublisher: Send + Sync + 'static {
    /// Replaces `destination` with the prepared `source` in one atomic operation.
    ///
    /// Both paths must be absolute same-volume siblings. On failure, the existing
    /// destination remains authoritative and the source remains available as residue.
    ///
    /// # Errors
    ///
    /// Returns a stable `RunStore` failure when publication cannot complete atomically.
    fn replace(&self, source: &Path, destination: &Path) -> Result<(), PortFailure>;
}

/// Worker-owned collection of all effects available to one processing run.
pub trait RunEnvironment: Send + 'static {
    /// Returns the unique identity allocated to this run.
    fn run_id(&self) -> RunId;

    /// Returns the unique disposable directory owned by this run.
    fn scratch_directory(&self) -> &Path;

    /// Returns the durable portable-state path of this run's UTF-8 log.
    fn log_path(&self) -> &Path;

    /// Borrows the run's bounded filesystem effect session.
    fn store(&mut self) -> &mut dyn RunStore;

    /// Borrows the run's durable UTF-8 log session.
    fn log(&mut self) -> &mut dyn RunLog;

    /// Borrows the run's contained one-shot process session.
    fn process(&mut self) -> &mut dyn OneShotProcess;

    /// Borrows the run's clock seam.
    fn clock(&mut self) -> &mut dyn Clock;

    /// Borrows the run's identity seam.
    fn ids(&mut self) -> &mut dyn IdSource;

    /// Flushes durable diagnostics and attempts best-effort scratch cleanup.
    ///
    /// The returned report is exact after the cleanup attempt and may be non-empty
    /// without making the already-completed processing work invalid.
    fn finalize(&mut self) -> Result<ResidueReport, PortFailure>;
}

/// Limits applied independently to bounded filesystem operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RunStoreLimits {
    /// Maximum entries returned by one inventory operation.
    pub max_inventory_entries: usize,
    /// Maximum bytes returned by one read operation.
    pub max_read_bytes: usize,
    /// Maximum private staged artifacts and accumulated audit records.
    pub max_operations: usize,
    /// Maximum aggregate bytes held in private staging at once.
    pub max_staged_bytes: u64,
}

impl Default for RunStoreLimits {
    fn default() -> Self {
        Self {
            max_inventory_entries: 100_000,
            max_read_bytes: 256 * 1024 * 1024,
            max_operations: 100_000,
            max_staged_bytes: 1024 * 1024 * 1024,
        }
    }
}

/// One deterministic entry in a bounded filesystem inventory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventoryEntry {
    path: PathBuf,
    byte_len: u64,
    is_directory: bool,
}

impl InventoryEntry {
    /// Creates an owned inventory entry from observed filesystem metadata.
    #[must_use]
    pub fn new(path: PathBuf, byte_len: u64, is_directory: bool) -> Self {
        Self {
            path,
            byte_len,
            is_directory,
        }
    }

    /// Returns the absolute inventoried path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the observed byte length, or zero for directories.
    #[must_use]
    pub const fn byte_len(&self) -> u64 {
        self.byte_len
    }

    /// Reports whether this entry represents a directory.
    #[must_use]
    pub const fn is_directory(&self) -> bool {
        self.is_directory
    }
}

/// Opaque handle to one private staged artifact.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct StagedArtifact(u64);

impl StagedArtifact {
    /// Creates an adapter-scoped staged-artifact handle.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the adapter-scoped numeric handle.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Predicted or applied persistent filesystem mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteAuditAction {
    /// Create a target that did not previously exist.
    Create,
    /// Replace an existing target atomically.
    Replace,
    /// Remove an existing target atomically from its authoritative path.
    Delete,
}

/// One ordered dry-run or applied-write audit record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WriteAuditEntry {
    action: WriteAuditAction,
    target: PathBuf,
    byte_len: u64,
}

impl WriteAuditEntry {
    /// Creates an audit entry for one target mutation.
    #[must_use]
    pub fn new(action: WriteAuditAction, target: PathBuf, byte_len: u64) -> Self {
        Self {
            action,
            target,
            byte_len,
        }
    }

    /// Returns the predicted or applied mutation kind.
    #[must_use]
    pub const fn action(&self) -> WriteAuditAction {
        self.action
    }

    /// Returns the absolute asset target.
    #[must_use]
    pub fn target(&self) -> &Path {
        &self.target
    }

    /// Returns bytes supplied by a create/replace, or zero for deletion.
    #[must_use]
    pub const fn byte_len(&self) -> u64 {
        self.byte_len
    }
}

/// One private artifact that remained after a cleanup attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Residue {
    path: PathBuf,
    byte_len: u64,
}

impl Residue {
    /// Creates an exact residue entry from its current path and byte length.
    #[must_use]
    pub fn new(path: PathBuf, byte_len: u64) -> Self {
        Self { path, byte_len }
    }

    /// Returns the remaining private path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the currently observed byte length.
    #[must_use]
    pub const fn byte_len(&self) -> u64 {
        self.byte_len
    }
}

/// Exact bounded inventory of private artifacts remaining after cleanup.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResidueReport {
    entries: Vec<Residue>,
}

impl ResidueReport {
    /// Creates a report whose entries are already in deterministic path order.
    #[must_use]
    pub fn new(entries: Vec<Residue>) -> Self {
        Self { entries }
    }

    /// Returns remaining private artifacts in deterministic path order.
    #[must_use]
    pub fn entries(&self) -> &[Residue] {
        &self.entries
    }

    /// Reports whether cleanup removed every private artifact.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Bounded staged filesystem effects for one processing run.
pub trait RunStore: Send + 'static {
    /// Inventories an absolute directory in deterministic path order.
    fn inventory(&mut self, root: &Path) -> Result<Vec<InventoryEntry>, PortFailure>;

    /// Reads an absolute regular file subject to this store's read bound.
    fn read(&mut self, path: &Path) -> Result<Vec<u8>, PortFailure>;

    /// Writes one private artifact subject to the aggregate staging bound.
    fn stage(&mut self, bytes: &[u8]) -> Result<StagedArtifact, PortFailure>;

    /// Compares a staged artifact byte-for-byte with independently expected output.
    fn verify_staged(
        &mut self,
        artifact: StagedArtifact,
        expected: &[u8],
    ) -> Result<(), PortFailure>;

    /// Atomically creates or replaces an absolute target, or only audits it in dry-run mode.
    fn commit_replace(
        &mut self,
        artifact: StagedArtifact,
        target: &Path,
    ) -> Result<(), PortFailure>;

    /// Atomically removes an absolute target, or only audits it in dry-run mode.
    fn delete(&mut self, target: &Path) -> Result<(), PortFailure>;

    /// Returns ordered predicted or applied write facts accumulated by this run.
    fn audit(&self) -> &[WriteAuditEntry];

    /// Reports the exact private artifacts that currently remain.
    fn residue(&mut self) -> Result<ResidueReport, PortFailure>;

    /// Attempts to remove every private artifact and reports any exact residue.
    fn cleanup(&mut self) -> Result<ResidueReport, PortFailure>;
}

/// Durable UTF-8 diagnostic sink for one processing run.
pub trait RunLog: Send + 'static {
    /// Appends one complete UTF-8 record to this run's unique log.
    fn write(&mut self, record: &str) -> Result<(), PortFailure>;

    /// Flushes all previously accepted records to durable storage.
    fn flush(&mut self) -> Result<(), PortFailure>;
}

/// Application clock seam with deterministic failure behavior.
pub trait Clock: Send + 'static {
    /// Returns the current non-negative Unix-epoch timestamp.
    fn now(&mut self) -> Result<TimestampMillis, PortFailure>;
}

/// Application identity seam with deterministic failure behavior.
pub trait IdSource: Send + 'static {
    /// Returns a fresh identity that has not previously been produced by this source.
    fn next_id(&mut self) -> Result<RunId, PortFailure>;
}

/// Cooperative cancellation state observed while a helper process is active.
pub trait CancellationProbe: Send + Sync {
    /// Reports whether cancellation has been accepted for the owning run.
    fn is_cancelled(&self) -> bool;
}

/// Immutable invocation of one absolute helper executable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessRequest {
    executable: PathBuf,
    working_directory: PathBuf,
    arguments: Vec<String>,
    environment: Vec<(String, String)>,
    timeout: Duration,
    output_limit: usize,
}

impl ProcessRequest {
    /// Creates an invocation with explicit arguments, minimal environment, and bounds.
    #[must_use]
    pub fn new(
        executable: PathBuf,
        working_directory: PathBuf,
        arguments: Vec<String>,
        environment: Vec<(String, String)>,
        timeout: Duration,
        output_limit: usize,
    ) -> Self {
        Self {
            executable,
            working_directory,
            arguments,
            environment,
            timeout,
            output_limit: output_limit.min(DEFAULT_PROCESS_OUTPUT_BYTES),
        }
    }

    /// Returns the absolute helper executable requested by the caller.
    #[must_use]
    pub fn executable(&self) -> &Path {
        &self.executable
    }

    /// Returns the explicit absolute working directory.
    #[must_use]
    pub fn working_directory(&self) -> &Path {
        &self.working_directory
    }

    /// Returns arguments without an inferred shell or command line.
    #[must_use]
    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }

    /// Returns the complete minimal environment supplied to the child.
    #[must_use]
    pub fn environment(&self) -> &[(String, String)] {
        &self.environment
    }

    /// Returns the maximum runtime before the whole process job is terminated.
    #[must_use]
    pub const fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Returns the independent byte cap applied to stdout and stderr retention.
    #[must_use]
    pub const fn output_limit(&self) -> usize {
        self.output_limit
    }
}

/// Bounded bytes drained from one helper output stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

impl ProcessOutput {
    /// Creates output facts after the complete stream has been drained.
    #[must_use]
    pub fn new(bytes: Vec<u8>, truncated: bool) -> Self {
        Self { bytes, truncated }
    }

    /// Returns retained bytes in their original order.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Reports whether additional stream bytes were drained but omitted.
    #[must_use]
    pub const fn was_truncated(&self) -> bool {
        self.truncated
    }
}

/// Structured terminal classification for one helper process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessTermination {
    /// The helper exited independently with the optional platform exit code.
    Exited(Option<i32>),
    /// The configured timeout elapsed and the complete process job was terminated.
    TimedOut,
    /// Run cancellation was accepted and the complete process job was terminated.
    Cancelled,
}

/// Complete bounded observations from one contained helper invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessFacts {
    termination: ProcessTermination,
    stdout: ProcessOutput,
    stderr: ProcessOutput,
}

impl ProcessFacts {
    /// Creates structured process facts from terminal and drained-output observations.
    #[must_use]
    pub fn new(
        termination: ProcessTermination,
        stdout: ProcessOutput,
        stderr: ProcessOutput,
    ) -> Self {
        Self {
            termination,
            stdout,
            stderr,
        }
    }

    /// Returns how the contained helper reached a terminal state.
    #[must_use]
    pub const fn termination(&self) -> ProcessTermination {
        self.termination
    }

    /// Returns bounded stdout facts.
    #[must_use]
    pub const fn stdout(&self) -> &ProcessOutput {
        &self.stdout
    }

    /// Returns bounded stderr facts.
    #[must_use]
    pub const fn stderr(&self) -> &ProcessOutput {
        &self.stderr
    }
}

/// Worker-owned contained helper-process capability.
pub trait OneShotProcess: Send + 'static {
    /// Executes one helper without a shell, inheritable ambient handles, or ambient environment.
    ///
    /// Timeout and cancellation are successful control-flow results. Spawn, containment,
    /// waiting, output-drain, or termination failures return a stable process failure.
    fn execute(
        &mut self,
        request: &ProcessRequest,
        cancellation: &dyn CancellationProbe,
    ) -> Result<ProcessFacts, PortFailure>;
}
