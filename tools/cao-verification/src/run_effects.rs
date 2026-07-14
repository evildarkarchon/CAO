//! Deterministic per-run effect adapters and their shared conformance contract.

use crate::FaultPlan;
use cao_application::{
    ActiveProfileId, AtomicFilePublisher, CancellationProbe, Clock, FailureKind, IdSource,
    InventoryEntry, MAX_RETAINED_RUN_LOG_BYTES, MAX_RETAINED_RUN_LOGS, MAX_RUN_LOG_BYTES,
    OneShotProcess, OperationId, PortFailure, PortId, ProcessFacts, ProcessOutput, ProcessRequest,
    ProcessTermination, Residue, ResidueReport, RunEnvironment, RunEnvironmentFactory,
    RunEnvironmentRequest, RunId, RunLog, RunStore, RunStoreLimits, StagedArtifact,
    TimestampMillis, WriteAuditAction, WriteAuditEntry,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const BASE_TIMESTAMP_MILLIS: u64 = 1_700_000_000_000;

struct NeverCancelled;

impl CancellationProbe for NeverCancelled {
    fn is_cancelled(&self) -> bool {
        false
    }
}

struct AlwaysCancelled;

impl CancellationProbe for AlwaysCancelled {
    fn is_cancelled(&self) -> bool {
        true
    }
}

/// Scripted terminal output returned by a deterministic one-shot process.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeterministicProcessScript {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    exit_code: Option<i32>,
}

impl DeterministicProcessScript {
    /// Creates one stable helper result whose streams are bounded by each request.
    #[must_use]
    pub fn new(stdout: Vec<u8>, stderr: Vec<u8>, exit_code: Option<i32>) -> Self {
        Self {
            stdout,
            stderr,
            exit_code,
        }
    }
}

impl Default for DeterministicProcessScript {
    fn default() -> Self {
        Self::new(
            b"deterministic helper stdout".to_vec(),
            b"deterministic helper stderr".to_vec(),
            Some(0),
        )
    }
}

/// Reusable deterministic implementation of all fresh per-run effect capabilities.
#[derive(Clone)]
pub struct DeterministicRunEnvironmentFactory {
    root: PathBuf,
    limits: RunStoreLimits,
    next_run: Arc<AtomicU64>,
    faults: Arc<Mutex<FaultPlan>>,
    process_script: DeterministicProcessScript,
    active_logs: Arc<Mutex<BTreeSet<PathBuf>>>,
    atomic_files: Arc<dyn AtomicFilePublisher>,
}

impl DeterministicRunEnvironmentFactory {
    /// Creates a deterministic factory rooted in one verification-owned sandbox.
    #[must_use]
    pub fn new(root: PathBuf, atomic_files: Arc<dyn AtomicFilePublisher>) -> Self {
        Self::with_configuration(
            root,
            RunStoreLimits::default(),
            FaultPlan::default(),
            DeterministicProcessScript::default(),
            atomic_files,
        )
    }

    /// Creates a deterministic factory with explicit bounds, faults, and process facts.
    #[must_use]
    pub fn with_configuration(
        root: PathBuf,
        limits: RunStoreLimits,
        faults: FaultPlan,
        process_script: DeterministicProcessScript,
        atomic_files: Arc<dyn AtomicFilePublisher>,
    ) -> Self {
        Self {
            root,
            limits,
            next_run: Arc::new(AtomicU64::new(1)),
            faults: Arc::new(Mutex::new(faults)),
            process_script,
            active_logs: Arc::new(Mutex::new(BTreeSet::new())),
            atomic_files,
        }
    }
}

impl RunEnvironmentFactory for DeterministicRunEnvironmentFactory {
    /// Allocates deterministic scratch, log, and capability state for one isolated run.
    fn create(
        &self,
        request: RunEnvironmentRequest,
    ) -> Result<Box<dyn RunEnvironment>, PortFailure> {
        take_fault(
            &self.faults,
            PortId::RunEnvironment,
            OperationId::CreateRunEnvironment,
            Some(&self.root),
        )?;
        let ordinal = self
            .next_run
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map_err(|_| {
                resource_failure(
                    PortId::RunEnvironment,
                    OperationId::CreateRunEnvironment,
                    &self.root,
                    "deterministic run identity exhausted",
                )
            })?;
        let run_id = RunId::new(u128::from(ordinal));
        let timestamp = BASE_TIMESTAMP_MILLIS.checked_add(ordinal).ok_or_else(|| {
            resource_failure(
                PortId::RunEnvironment,
                OperationId::CreateRunEnvironment,
                &self.root,
                "deterministic run timestamp exhausted",
            )
        })?;
        let scratch_parent = self.root.join("scratch");
        let scratch_directory = scratch_parent.join(format!("run-{ordinal:016x}"));
        std::fs::create_dir_all(&scratch_parent).map_err(|error| {
            io_failure(
                PortId::RunEnvironment,
                OperationId::CreateRunEnvironment,
                &scratch_parent,
                error,
            )
        })?;
        std::fs::create_dir(&scratch_directory).map_err(|error| {
            io_failure(
                PortId::RunEnvironment,
                OperationId::CreateRunEnvironment,
                &scratch_directory,
                error,
            )
        })?;
        let staging_directory = scratch_directory.join("staged");
        if let Err(error) = std::fs::create_dir(&staging_directory) {
            discard_failed_run_initialization(&scratch_directory, None);
            return Err(io_failure(
                PortId::RunEnvironment,
                OperationId::CreateRunEnvironment,
                &staging_directory,
                error,
            ));
        }

        let logs = self.root.join("logs");
        if let Err(error) = std::fs::create_dir_all(&logs) {
            discard_failed_run_initialization(&scratch_directory, None);
            return Err(io_failure(
                PortId::RunEnvironment,
                OperationId::CreateRunEnvironment,
                &logs,
                error,
            ));
        }
        let active = match active_log_snapshot(&self.active_logs, &logs) {
            Ok(active) => active,
            Err(failure) => {
                discard_failed_run_initialization(&scratch_directory, None);
                return Err(failure);
            }
        };
        if let Err(failure) = prune_completed_logs(&logs, &active) {
            discard_failed_run_initialization(&scratch_directory, None);
            return Err(failure);
        }
        let log_path = logs.join(format!(
            "{timestamp}-{}-{ordinal:032x}.log",
            profile_slug(request.profile_id())
        ));
        let mut log_file = match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&log_path)
        {
            Ok(file) => file,
            Err(error) => {
                discard_failed_run_initialization(&scratch_directory, None);
                return Err(io_failure(
                    PortId::RunEnvironment,
                    OperationId::CreateRunEnvironment,
                    &log_path,
                    error,
                ));
            }
        };
        if let Err(error) = writeln!(
            log_file,
            "timestamp={timestamp} profile={} run_id={} dry_run={}",
            profile_slug(request.profile_id()),
            run_id.get(),
            request.dry_run()
        ) {
            drop(log_file);
            discard_failed_run_initialization(&scratch_directory, Some(&log_path));
            return Err(io_failure(
                PortId::RunLog,
                OperationId::WriteLog,
                &log_path,
                error,
            ));
        }
        let log_bytes = match log_file.metadata() {
            Ok(metadata) => metadata.len(),
            Err(error) => {
                drop(log_file);
                discard_failed_run_initialization(&scratch_directory, Some(&log_path));
                return Err(io_failure(
                    PortId::RunLog,
                    OperationId::WriteLog,
                    &log_path,
                    error,
                ));
            }
        };
        match self.active_logs.lock() {
            Ok(mut active) => {
                active.insert(log_path.clone());
            }
            Err(_) => {
                drop(log_file);
                discard_failed_run_initialization(&scratch_directory, Some(&log_path));
                return Err(active_log_lock_failure(&log_path));
            }
        }

        Ok(Box::new(DeterministicRunEnvironment {
            run_id,
            scratch_directory: scratch_directory.clone(),
            log_path: log_path.clone(),
            store: DeterministicRunStore::new(
                scratch_directory,
                staging_directory,
                request.dry_run(),
                self.limits,
                Arc::clone(&self.faults),
                Arc::clone(&self.atomic_files),
            ),
            log: DeterministicRunLog {
                path: log_path,
                file: Some(log_file),
                byte_len: log_bytes,
                faults: Arc::clone(&self.faults),
            },
            process: DeterministicOneShotProcess {
                script: self.process_script.clone(),
                faults: Arc::clone(&self.faults),
            },
            clock: DeterministicClock {
                next_millis: timestamp,
                faults: Arc::clone(&self.faults),
            },
            ids: DeterministicIdSource {
                next: (u128::from(ordinal) << 64) | 1,
                faults: Arc::clone(&self.faults),
            },
            active_logs: Arc::clone(&self.active_logs),
            finalized: false,
        }))
    }
}

/// Runs the adapter-neutral environment, store, log, clock, and identity contract.
///
/// # Errors
///
/// Returns a concise assertion diagnostic when the supplied adapter violates the contract.
pub fn assert_run_effects_conformance(
    factory: &dyn RunEnvironmentFactory,
    asset_root: &Path,
) -> Result<(), String> {
    std::fs::create_dir_all(asset_root)
        .map_err(|error| format!("create conformance asset root: {error}"))?;
    let request = RunEnvironmentRequest::new(ActiveProfileId::Sse, false);
    let mut first = factory
        .create(request)
        .map_err(|failure| format!("create first environment: {failure:?}"))?;
    let mut second = factory
        .create(request)
        .map_err(|failure| format!("create second environment: {failure:?}"))?;
    if first.run_id() == second.run_id()
        || first.scratch_directory() == second.scratch_directory()
        || first.log_path() == second.log_path()
    {
        return Err("fresh environments did not allocate unique identities and paths".to_owned());
    }

    let first_timestamp = first
        .clock()
        .now()
        .map_err(|failure| format!("read first timestamp: {failure:?}"))?;
    let second_timestamp = first
        .clock()
        .now()
        .map_err(|failure| format!("read second timestamp: {failure:?}"))?;
    if second_timestamp < first_timestamp {
        return Err("run clock moved backwards".to_owned());
    }
    let first_id = first
        .ids()
        .next_id()
        .map_err(|failure| format!("allocate first identity: {failure:?}"))?;
    let second_id = first
        .ids()
        .next_id()
        .map_err(|failure| format!("allocate second identity: {failure:?}"))?;
    if first_id == second_id {
        return Err("run identity source returned a duplicate".to_owned());
    }

    let source = asset_root.join("source.bin");
    std::fs::write(&source, b"source bytes")
        .map_err(|error| format!("write inventory source: {error}"))?;
    let inventory = first
        .store()
        .inventory(asset_root)
        .map_err(|failure| format!("inventory asset root: {failure:?}"))?;
    if !inventory.iter().any(|entry| entry.path() == source) {
        return Err("bounded inventory omitted its source file".to_owned());
    }
    if first
        .store()
        .read(&source)
        .map_err(|failure| format!("read source file: {failure:?}"))?
        != b"source bytes"
    {
        return Err("bounded read changed source bytes".to_owned());
    }

    let target = asset_root.join("committed.bin");
    let created = first
        .store()
        .stage(b"created")
        .map_err(|failure| format!("stage create: {failure:?}"))?;
    first
        .store()
        .verify_staged(created, b"created")
        .map_err(|failure| format!("verify create: {failure:?}"))?;
    first
        .store()
        .commit_replace(created, &target)
        .map_err(|failure| format!("commit create: {failure:?}"))?;
    let replacement = first
        .store()
        .stage(b"replacement")
        .map_err(|failure| format!("stage replacement: {failure:?}"))?;
    first
        .store()
        .verify_staged(replacement, b"replacement")
        .map_err(|failure| format!("verify replacement: {failure:?}"))?;
    first
        .store()
        .commit_replace(replacement, &target)
        .map_err(|failure| format!("commit replacement: {failure:?}"))?;
    if std::fs::read(&target).map_err(|error| format!("read committed target: {error}"))?
        != b"replacement"
    {
        return Err("replace commit did not publish staged bytes".to_owned());
    }
    first
        .store()
        .delete(&target)
        .map_err(|failure| format!("delete target: {failure:?}"))?;
    if target.exists() {
        return Err("delete left the authoritative target present".to_owned());
    }
    let actions: Vec<_> = first
        .store()
        .audit()
        .iter()
        .map(WriteAuditEntry::action)
        .collect();
    if actions
        != [
            WriteAuditAction::Create,
            WriteAuditAction::Replace,
            WriteAuditAction::Delete,
        ]
    {
        return Err(format!("unexpected write audit sequence: {actions:?}"));
    }

    let residue_artifact = first
        .store()
        .stage(b"temporary residue")
        .map_err(|failure| format!("stage residue: {failure:?}"))?;
    first
        .store()
        .verify_staged(residue_artifact, b"temporary residue")
        .map_err(|failure| format!("verify residue: {failure:?}"))?;
    let residue = first
        .store()
        .residue()
        .map_err(|failure| format!("report residue: {failure:?}"))?;
    if residue.is_empty() {
        return Err("staged private artifact was absent from residue".to_owned());
    }

    first
        .log()
        .write("severity=info message=Unicode ✓")
        .map_err(|failure| format!("write UTF-8 log: {failure:?}"))?;
    first
        .log()
        .flush()
        .map_err(|failure| format!("flush UTF-8 log: {failure:?}"))?;
    let log_bytes =
        std::fs::read(first.log_path()).map_err(|error| format!("read UTF-8 log: {error}"))?;
    let log =
        String::from_utf8(log_bytes).map_err(|error| format!("log was not UTF-8: {error}"))?;
    if !log.contains("Unicode ✓") {
        return Err("run log omitted the accepted UTF-8 record".to_owned());
    }

    let process_request = ProcessRequest::new(
        std::env::current_exe()
            .map_err(|error| format!("resolve conformance test executable: {error}"))?,
        asset_root.to_path_buf(),
        vec![
            "--exact".to_owned(),
            "run_effects::tests::conformance_process_probe".to_owned(),
        ],
        Vec::new(),
        Duration::from_secs(5),
        32,
    );
    let process = first
        .process()
        .execute(&process_request, &NeverCancelled)
        .map_err(|failure| format!("execute bounded process: {failure:?}"))?;
    if process.termination() != ProcessTermination::Exited(Some(0))
        || process.stdout().bytes().len() > 32
        || process.stderr().bytes().len() > 32
    {
        return Err(format!("unexpected bounded process facts: {process:?}"));
    }

    let timeout_request = ProcessRequest::new(
        process_request.executable().to_path_buf(),
        process_request.working_directory().to_path_buf(),
        process_request.arguments().to_vec(),
        process_request.environment().to_vec(),
        Duration::ZERO,
        32,
    );
    let timed_out = first
        .process()
        .execute(&timeout_request, &NeverCancelled)
        .map_err(|failure| format!("execute timed process: {failure:?}"))?;
    if timed_out.termination() != ProcessTermination::TimedOut {
        return Err(format!(
            "zero-timeout process did not report timeout: {timed_out:?}"
        ));
    }
    let cancelled = first
        .process()
        .execute(&process_request, &AlwaysCancelled)
        .map_err(|failure| format!("execute cancelled process: {failure:?}"))?;
    if cancelled.termination() != ProcessTermination::Cancelled {
        return Err(format!(
            "pre-cancelled process did not report cancellation: {cancelled:?}"
        ));
    }

    let cleanup = first
        .finalize()
        .map_err(|failure| format!("finalize run environment: {failure:?}"))?;
    if !cleanup.is_empty() {
        return Err(format!(
            "finalization left residue: {:?}",
            cleanup.entries()
        ));
    }
    let repeated_cleanup = first
        .finalize()
        .map_err(|failure| format!("repeat run finalization: {failure:?}"))?;
    if !repeated_cleanup.is_empty() {
        return Err(format!(
            "repeated finalization left residue: {:?}",
            repeated_cleanup.entries()
        ));
    }
    let second_cleanup = second
        .finalize()
        .map_err(|failure| format!("finalize second run environment: {failure:?}"))?;
    if !second_cleanup.is_empty() {
        return Err(format!(
            "second finalization left residue: {:?}",
            second_cleanup.entries()
        ));
    }

    let dry_target = asset_root.join("dry-run.bin");
    std::fs::write(&dry_target, b"original")
        .map_err(|error| format!("write dry-run target: {error}"))?;
    let mut dry = factory
        .create(RunEnvironmentRequest::new(ActiveProfileId::Sse, true))
        .map_err(|failure| format!("create dry-run environment: {failure:?}"))?;
    let dry_artifact = dry
        .store()
        .stage(b"predicted")
        .map_err(|failure| format!("stage dry-run replacement: {failure:?}"))?;
    dry.store()
        .commit_replace(dry_artifact, &dry_target)
        .map_err(|failure| format!("audit dry-run replacement: {failure:?}"))?;
    dry.store()
        .delete(&dry_target)
        .map_err(|failure| format!("audit dry-run deletion: {failure:?}"))?;
    if std::fs::read(&dry_target).map_err(|error| format!("read dry-run target: {error}"))?
        != b"original"
        || dry.store().audit().len() != 2
    {
        return Err("dry-run conformance mutated authority or omitted audit facts".to_owned());
    }
    if !dry
        .finalize()
        .map_err(|failure| format!("finalize dry-run environment: {failure:?}"))?
        .is_empty()
    {
        return Err("dry-run finalization left private residue".to_owned());
    }
    Ok(())
}

/// Runs the adapter-neutral operation-bound contract against a tightly configured factory.
///
/// The supplied factory must use limits of one inventory entry, four read and staged bytes,
/// and one concurrent operation/audit record.
///
/// # Errors
///
/// Returns a concise assertion diagnostic when any bound is omitted or mapped unstably.
pub fn assert_run_store_bounds_conformance(
    factory: &dyn RunEnvironmentFactory,
    asset_root: &Path,
) -> Result<(), String> {
    let inventory_root = asset_root.join("inventory-bound");
    std::fs::create_dir_all(&inventory_root)
        .map_err(|error| format!("create bounded inventory root: {error}"))?;
    std::fs::write(inventory_root.join("a.bin"), b"a")
        .and_then(|()| std::fs::write(inventory_root.join("b.bin"), b"b"))
        .map_err(|error| format!("write bounded inventory fixtures: {error}"))?;
    let request = RunEnvironmentRequest::new(ActiveProfileId::Sse, false);
    let mut inventory = factory
        .create(request)
        .map_err(|failure| format!("create inventory-bound environment: {failure:?}"))?;
    let failure = inventory
        .store()
        .inventory(&inventory_root)
        .expect_err("inventory beyond the configured entry bound must fail");
    expect_resource_failure(&failure, OperationId::Inventory)?;
    inventory
        .finalize()
        .map_err(|failure| format!("finalize inventory-bound environment: {failure:?}"))?;

    let oversized = asset_root.join("oversized-read.bin");
    std::fs::write(&oversized, b"12345")
        .map_err(|error| format!("write oversized read fixture: {error}"))?;
    let mut read = factory
        .create(request)
        .map_err(|failure| format!("create read-bound environment: {failure:?}"))?;
    let failure = read
        .store()
        .read(&oversized)
        .expect_err("read beyond the configured byte bound must fail");
    expect_resource_failure(&failure, OperationId::Read)?;
    read.finalize()
        .map_err(|failure| format!("finalize read-bound environment: {failure:?}"))?;

    let mut staging = factory
        .create(request)
        .map_err(|failure| format!("create staging-bound environment: {failure:?}"))?;
    staging
        .store()
        .stage(b"1234")
        .map_err(|failure| format!("stage at configured bounds: {failure:?}"))?;
    let failure = staging
        .store()
        .stage(b"")
        .expect_err("a second concurrent staged operation must fail");
    expect_resource_failure(&failure, OperationId::StageWrite)?;
    staging
        .finalize()
        .map_err(|failure| format!("finalize staging-bound environment: {failure:?}"))?;

    let mut audit = factory
        .create(request)
        .map_err(|failure| format!("create audit-bound environment: {failure:?}"))?;
    let first = audit
        .store()
        .stage(b"one")
        .map_err(|failure| format!("stage first audited write: {failure:?}"))?;
    audit
        .store()
        .commit_replace(first, &asset_root.join("first-audit.bin"))
        .map_err(|failure| format!("commit first audited write: {failure:?}"))?;
    let second = audit
        .store()
        .stage(b"two")
        .map_err(|failure| format!("stage second audited write: {failure:?}"))?;
    let failure = audit
        .store()
        .commit_replace(second, &asset_root.join("second-audit.bin"))
        .expect_err("a second audit record beyond the configured bound must fail");
    expect_resource_failure(&failure, OperationId::CommitReplace)?;
    audit
        .finalize()
        .map_err(|failure| format!("finalize audit-bound environment: {failure:?}"))?;
    Ok(())
}

/// Runs completed-log count and aggregate-byte retention against any run factory.
///
/// # Errors
///
/// Returns a concise assertion diagnostic when pruning fails or exceeds either bound.
pub fn assert_run_log_retention_conformance(
    factory: &dyn RunEnvironmentFactory,
) -> Result<(), String> {
    let request = RunEnvironmentRequest::new(ActiveProfileId::Sse, false);
    let mut seed = factory
        .create(request)
        .map_err(|failure| format!("create retention seed environment: {failure:?}"))?;
    let logs = seed
        .log_path()
        .parent()
        .ok_or_else(|| "retention seed log has no parent".to_owned())?
        .to_path_buf();
    seed.finalize()
        .map_err(|failure| format!("finalize retention seed: {failure:?}"))?;
    for index in 0..2 {
        let path = logs.join(format!("0000000000000000000-old-{index}.log"));
        File::create(&path)
            .and_then(|file| file.set_len(60 * 1024 * 1024))
            .map_err(|error| format!("create sparse retention fixture: {error}"))?;
    }
    for _ in 0..=MAX_RETAINED_RUN_LOGS {
        let mut environment = factory
            .create(request)
            .map_err(|failure| format!("create retained environment: {failure:?}"))?;
        environment
            .finalize()
            .map_err(|failure| format!("finalize retained environment: {failure:?}"))?;
    }
    let retained: Vec<_> = std::fs::read_dir(&logs)
        .map_err(|error| format!("enumerate retained logs: {error}"))?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|value| value.to_str()) == Some("log"))
        .collect();
    let retained_bytes = retained
        .iter()
        .try_fold(0_u64, |total, entry| {
            entry
                .metadata()
                .map(|metadata| total.saturating_add(metadata.len()))
        })
        .map_err(|error| format!("read retained log metadata: {error}"))?;
    if retained.len() > MAX_RETAINED_RUN_LOGS || retained_bytes > MAX_RETAINED_RUN_LOG_BYTES {
        return Err(format!(
            "retained logs exceeded bounds: count={} bytes={retained_bytes}",
            retained.len()
        ));
    }
    Ok(())
}

/// Runs failed atomic-replacement authority and exact-residue checks against any adapter.
///
/// # Errors
///
/// Returns a concise assertion diagnostic when a locked authority changes, failure mapping
/// drifts, or the adapter does not report both its staged and same-directory pending files.
pub fn assert_failed_atomic_replace_conformance(
    factory: &dyn RunEnvironmentFactory,
    asset_root: &Path,
) -> Result<(), String> {
    std::fs::create_dir_all(asset_root)
        .map_err(|error| format!("create atomic failure root: {error}"))?;
    let target = asset_root.join("locked-authority.bin");
    std::fs::write(&target, b"original")
        .map_err(|error| format!("write atomic failure authority: {error}"))?;
    let lock = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&target)
        .map_err(|error| format!("lock atomic failure authority: {error}"))?;
    let mut environment = factory
        .create(RunEnvironmentRequest::new(ActiveProfileId::Sse, false))
        .map_err(|failure| format!("create atomic failure environment: {failure:?}"))?;
    let artifact = environment
        .store()
        .stage(b"replacement")
        .map_err(|failure| format!("stage atomic failure candidate: {failure:?}"))?;
    let failure = environment
        .store()
        .commit_replace(artifact, &target)
        .expect_err("locked atomic replacement must fail");
    if failure.port() != PortId::RunStore || failure.operation() != OperationId::CommitReplace {
        return Err(format!(
            "unstable atomic replacement failure mapping: {failure:?}"
        ));
    }
    if std::fs::read(&target).map_err(|error| format!("read locked authority: {error}"))?
        != b"original"
    {
        return Err("failed replacement changed authoritative bytes".to_owned());
    }
    let residue = environment
        .store()
        .residue()
        .map_err(|failure| format!("report failed replacement residue: {failure:?}"))?;
    let scratch_count = residue
        .entries()
        .iter()
        .filter(|entry| entry.path().starts_with(environment.scratch_directory()))
        .count();
    let sibling_count = residue
        .entries()
        .iter()
        .filter(|entry| entry.path().parent() == target.parent())
        .count();
    if residue.entries().len() != 2 || scratch_count != 1 || sibling_count != 1 {
        return Err(format!(
            "failed replacement residue was not exact: {:?}",
            residue.entries()
        ));
    }
    drop(lock);
    let cleanup = environment
        .finalize()
        .map_err(|failure| format!("clean failed replacement residue: {failure:?}"))?;
    if !cleanup.is_empty() {
        return Err(format!(
            "failed replacement cleanup left residue: {cleanup:?}"
        ));
    }
    Ok(())
}

/// Confirms a bounded operation maps to stable resource-exhaustion vocabulary.
fn expect_resource_failure(failure: &PortFailure, operation: OperationId) -> Result<(), String> {
    if failure.port() == PortId::RunStore
        && failure.operation() == operation
        && failure.kind() == FailureKind::ResourceExhausted
    {
        Ok(())
    } else {
        Err(format!("unexpected bounded-operation failure: {failure:?}"))
    }
}

struct DeterministicRunEnvironment {
    run_id: RunId,
    scratch_directory: PathBuf,
    log_path: PathBuf,
    store: DeterministicRunStore,
    log: DeterministicRunLog,
    process: DeterministicOneShotProcess,
    clock: DeterministicClock,
    ids: DeterministicIdSource,
    active_logs: Arc<Mutex<BTreeSet<PathBuf>>>,
    finalized: bool,
}

impl DeterministicRunEnvironment {
    /// Marks the durable log completed and enforces both completed-log retention bounds.
    fn complete_log(&mut self) -> Result<(), PortFailure> {
        let mut active = self
            .active_logs
            .lock()
            .map_err(|_| active_log_lock_failure(&self.log_path))?;
        active.remove(&self.log_path);
        let completed_snapshot = active.clone();
        drop(active);
        let logs = self.log_path.parent().ok_or_else(|| {
            effect_failure(
                PortId::RunLog,
                OperationId::Cleanup,
                FailureKind::Internal,
                Some(&self.log_path),
                "deterministic run log has no retention directory",
            )
        })?;
        prune_completed_logs(logs, &completed_snapshot)
    }
}

impl RunEnvironment for DeterministicRunEnvironment {
    /// Returns the deterministic identity assigned when this environment was created.
    fn run_id(&self) -> RunId {
        self.run_id
    }

    /// Returns the unique private directory owned by this deterministic run.
    fn scratch_directory(&self) -> &Path {
        &self.scratch_directory
    }

    /// Returns the durable UTF-8 log path retained after scratch cleanup.
    fn log_path(&self) -> &Path {
        &self.log_path
    }

    /// Borrows this run's bounded deterministic filesystem capability.
    fn store(&mut self) -> &mut dyn RunStore {
        &mut self.store
    }

    /// Borrows this run's bounded deterministic log capability.
    fn log(&mut self) -> &mut dyn RunLog {
        &mut self.log
    }

    /// Borrows this run's scripted one-shot process capability.
    fn process(&mut self) -> &mut dyn OneShotProcess {
        &mut self.process
    }

    /// Borrows this run's deterministic monotonic clock.
    fn clock(&mut self) -> &mut dyn Clock {
        &mut self.clock
    }

    /// Borrows this run's deterministic identity source.
    fn ids(&mut self) -> &mut dyn IdSource {
        &mut self.ids
    }

    /// Flushes logging, applies retention, and removes all tracked private residue.
    ///
    /// Repeated calls are safe and report any residue remaining after the first cleanup.
    fn finalize(&mut self) -> Result<ResidueReport, PortFailure> {
        if self.finalized {
            return self.store.residue();
        }
        let flush = self.log.flush();
        self.log.close();
        let cleanup = self.store.cleanup();
        let retention = self.complete_log();
        self.finalized = true;
        flush?;
        retention?;
        cleanup
    }
}

impl Drop for DeterministicRunEnvironment {
    /// Best-effort finalizes capabilities that were not explicitly finalized by their owner.
    fn drop(&mut self) {
        if !self.finalized {
            // Drop cannot surface cleanup diagnostics, so explicit finalization remains authoritative.
            let _ = self.log.flush();
            self.log.close();
            let _ = self.store.cleanup();
            let _ = self.complete_log();
        }
    }
}

struct ArtifactState {
    path: PathBuf,
    byte_len: u64,
}

struct DeterministicRunStore {
    scratch_directory: PathBuf,
    staging_directory: PathBuf,
    dry_run: bool,
    limits: RunStoreLimits,
    next_artifact: u64,
    staged_bytes: u64,
    artifacts: BTreeMap<u64, ArtifactState>,
    sibling_residue: BTreeSet<PathBuf>,
    audit: Vec<WriteAuditEntry>,
    faults: Arc<Mutex<FaultPlan>>,
    atomic_files: Arc<dyn AtomicFilePublisher>,
}

impl DeterministicRunStore {
    /// Creates a worker-owned store whose private bytes live under one scratch directory.
    fn new(
        scratch_directory: PathBuf,
        staging_directory: PathBuf,
        dry_run: bool,
        limits: RunStoreLimits,
        faults: Arc<Mutex<FaultPlan>>,
        atomic_files: Arc<dyn AtomicFilePublisher>,
    ) -> Self {
        Self {
            scratch_directory,
            staging_directory,
            dry_run,
            limits,
            next_artifact: 1,
            staged_bytes: 0,
            artifacts: BTreeMap::new(),
            sibling_residue: BTreeSet::new(),
            audit: Vec::new(),
            faults,
            atomic_files,
        }
    }

    /// Removes one consumed staged artifact from both disk and bounded accounting.
    fn consume(&mut self, artifact: StagedArtifact) -> Result<(), PortFailure> {
        let state = self.artifacts.remove(&artifact.get()).ok_or_else(|| {
            effect_failure(
                PortId::RunStore,
                OperationId::Read,
                FailureKind::NotFound,
                None,
                "staged artifact handle was not found",
            )
        })?;
        std::fs::remove_file(&state.path).map_err(|error| {
            io_failure(PortId::RunStore, OperationId::Cleanup, &state.path, error)
        })?;
        self.staged_bytes -= state.byte_len;
        Ok(())
    }

    /// Returns a currently owned staged artifact or a stable not-found failure.
    fn artifact(
        &self,
        artifact: StagedArtifact,
        operation: OperationId,
    ) -> Result<&ArtifactState, PortFailure> {
        self.artifacts.get(&artifact.get()).ok_or_else(|| {
            effect_failure(
                PortId::RunStore,
                operation,
                FailureKind::NotFound,
                None,
                "staged artifact handle was not found",
            )
        })
    }

    /// Ensures another private staged artifact fits the operation-count bound.
    fn require_stage_capacity(&self, path: &Path) -> Result<(), PortFailure> {
        if self.artifacts.len() >= self.limits.max_operations {
            Err(resource_failure(
                PortId::RunStore,
                OperationId::StageWrite,
                path,
                "private staging exceeded its operation-count bound",
            ))
        } else {
            Ok(())
        }
    }

    /// Ensures another durable mutation fact fits the audit-count bound.
    fn require_audit_capacity(
        &self,
        operation: OperationId,
        target: &Path,
    ) -> Result<(), PortFailure> {
        if self.audit.len() >= self.limits.max_operations {
            Err(resource_failure(
                PortId::RunStore,
                operation,
                target,
                "write audit exceeded its operation-count bound",
            ))
        } else {
            Ok(())
        }
    }

    /// Publishes staged bytes through a flushed same-directory candidate.
    ///
    /// Every injected fault is consumed before this method starts. Filesystem failures
    /// before the final replacement leave the original authority untouched, and the
    /// pending file remains registered for exact residue reporting and cleanup.
    fn commit_from_sibling(
        &mut self,
        staged_path: &Path,
        artifact: StagedArtifact,
        target: &Path,
    ) -> Result<(), PortFailure> {
        let pending = sibling_path(
            target,
            artifact_suffix(artifact.get(), "pending"),
            OperationId::CommitReplace,
        )?;
        if pending.exists() {
            return Err(effect_failure(
                PortId::RunStore,
                OperationId::CommitReplace,
                FailureKind::Conflict,
                Some(&pending),
                "private commit sibling already exists",
            ));
        }

        let bytes = std::fs::read(staged_path).map_err(|error| {
            io_failure(
                PortId::RunStore,
                OperationId::CommitReplace,
                staged_path,
                error,
            )
        })?;
        let mut pending_file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&pending)
            .map_err(|error| {
                io_failure(
                    PortId::RunStore,
                    OperationId::CommitReplace,
                    &pending,
                    error,
                )
            })?;
        self.sibling_residue.insert(pending.clone());
        pending_file
            .write_all(&bytes)
            .and_then(|()| pending_file.flush())
            .and_then(|()| pending_file.sync_all())
            .map_err(|error| {
                io_failure(
                    PortId::RunStore,
                    OperationId::CommitReplace,
                    &pending,
                    error,
                )
            })?;
        drop(pending_file);

        self.atomic_files.replace(&pending, target)?;
        self.sibling_residue.remove(&pending);
        Ok(())
    }
}

impl RunStore for DeterministicRunStore {
    /// Recursively inventories entries beneath an absolute root within configured bounds.
    fn inventory(&mut self, root: &Path) -> Result<Vec<InventoryEntry>, PortFailure> {
        require_absolute(PortId::RunStore, OperationId::Inventory, root)?;
        take_fault(
            &self.faults,
            PortId::RunStore,
            OperationId::Inventory,
            Some(root),
        )?;
        let mut pending = vec![root.to_path_buf()];
        let mut entries = Vec::new();
        while let Some(directory) = pending.pop() {
            let children = std::fs::read_dir(&directory).map_err(|error| {
                io_failure(PortId::RunStore, OperationId::Inventory, &directory, error)
            })?;
            for child in children {
                let child = child.map_err(|error| {
                    io_failure(PortId::RunStore, OperationId::Inventory, &directory, error)
                })?;
                let path = child.path();
                let metadata = child.metadata().map_err(|error| {
                    io_failure(PortId::RunStore, OperationId::Inventory, &path, error)
                })?;
                let is_directory = metadata.is_dir();
                entries.push(InventoryEntry::new(
                    path.clone(),
                    if is_directory { 0 } else { metadata.len() },
                    is_directory,
                ));
                if entries.len() > self.limits.max_inventory_entries {
                    return Err(resource_failure(
                        PortId::RunStore,
                        OperationId::Inventory,
                        root,
                        "filesystem inventory exceeded its entry bound",
                    ));
                }
                if is_directory {
                    pending.push(path);
                }
            }
        }
        entries.sort_by(|left, right| left.path().cmp(right.path()));
        Ok(entries)
    }

    /// Reads one absolute file without exceeding the configured byte limit.
    fn read(&mut self, path: &Path) -> Result<Vec<u8>, PortFailure> {
        require_absolute(PortId::RunStore, OperationId::Read, path)?;
        take_fault(
            &self.faults,
            PortId::RunStore,
            OperationId::Read,
            Some(path),
        )?;
        let metadata = std::fs::metadata(path)
            .map_err(|error| io_failure(PortId::RunStore, OperationId::Read, path, error))?;
        if metadata.len() > self.limits.max_read_bytes as u64 {
            return Err(resource_failure(
                PortId::RunStore,
                OperationId::Read,
                path,
                "file exceeded the bounded read limit",
            ));
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        File::open(path)
            .and_then(|mut file| file.read_to_end(&mut bytes))
            .map_err(|error| io_failure(PortId::RunStore, OperationId::Read, path, error))?;
        if bytes.len() > self.limits.max_read_bytes {
            return Err(resource_failure(
                PortId::RunStore,
                OperationId::Read,
                path,
                "file grew beyond the bounded read limit",
            ));
        }
        Ok(bytes)
    }

    /// Persists bytes to a private artifact and returns its deterministic opaque handle.
    fn stage(&mut self, bytes: &[u8]) -> Result<StagedArtifact, PortFailure> {
        let handle = StagedArtifact::new(self.next_artifact);
        let path = self
            .staging_directory
            .join(format!("artifact-{:016x}.stage", handle.get()));
        self.require_stage_capacity(&path)?;
        take_fault(
            &self.faults,
            PortId::RunStore,
            OperationId::StageWrite,
            Some(&path),
        )?;
        let byte_len = u64::try_from(bytes.len()).map_err(|_| {
            resource_failure(
                PortId::RunStore,
                OperationId::StageWrite,
                &path,
                "staged artifact length cannot be represented",
            )
        })?;
        let aggregate = self.staged_bytes.checked_add(byte_len).ok_or_else(|| {
            resource_failure(
                PortId::RunStore,
                OperationId::StageWrite,
                &path,
                "aggregate staging byte count overflowed",
            )
        })?;
        if aggregate > self.limits.max_staged_bytes {
            return Err(resource_failure(
                PortId::RunStore,
                OperationId::StageWrite,
                &path,
                "aggregate private staging exceeded its byte bound",
            ));
        }
        std::fs::write(&path, bytes)
            .map_err(|error| io_failure(PortId::RunStore, OperationId::StageWrite, &path, error))?;
        self.next_artifact = self.next_artifact.checked_add(1).ok_or_else(|| {
            resource_failure(
                PortId::RunStore,
                OperationId::StageWrite,
                &path,
                "staged artifact identity exhausted",
            )
        })?;
        self.staged_bytes = aggregate;
        self.artifacts
            .insert(handle.get(), ArtifactState { path, byte_len });
        Ok(handle)
    }

    /// Re-reads a staged artifact and rejects it unless it exactly matches expected bytes.
    fn verify_staged(
        &mut self,
        artifact: StagedArtifact,
        expected: &[u8],
    ) -> Result<(), PortFailure> {
        let path = self
            .artifact(artifact, OperationId::VerifyStaged)?
            .path
            .clone();
        take_fault(
            &self.faults,
            PortId::RunStore,
            OperationId::VerifyStaged,
            Some(&path),
        )?;
        let actual = std::fs::read(&path).map_err(|error| {
            io_failure(PortId::RunStore, OperationId::VerifyStaged, &path, error)
        })?;
        if actual != expected {
            return Err(effect_failure(
                PortId::RunStore,
                OperationId::VerifyStaged,
                FailureKind::Integrity,
                Some(&path),
                "staged artifact did not match independently expected bytes",
            ));
        }
        Ok(())
    }

    /// Atomically publishes a staged artifact, or records only an audit in dry-run mode.
    fn commit_replace(
        &mut self,
        artifact: StagedArtifact,
        target: &Path,
    ) -> Result<(), PortFailure> {
        require_absolute(PortId::RunStore, OperationId::CommitReplace, target)?;
        let state = self.artifact(artifact, OperationId::CommitReplace)?;
        let staged_path = state.path.clone();
        let byte_len = state.byte_len;
        self.require_audit_capacity(OperationId::CommitReplace, target)?;
        take_fault(
            &self.faults,
            PortId::RunStore,
            OperationId::CommitReplace,
            Some(target),
        )?;
        let action = if target.exists() {
            WriteAuditAction::Replace
        } else {
            WriteAuditAction::Create
        };
        if !self.dry_run {
            self.commit_from_sibling(&staged_path, artifact, target)?;
        }
        self.audit
            .push(WriteAuditEntry::new(action, target.to_path_buf(), byte_len));
        self.consume(artifact)
    }

    /// Atomically removes an authoritative target, or records only an audit in dry-run mode.
    fn delete(&mut self, target: &Path) -> Result<(), PortFailure> {
        require_absolute(PortId::RunStore, OperationId::Delete, target)?;
        self.require_audit_capacity(OperationId::Delete, target)?;
        take_fault(
            &self.faults,
            PortId::RunStore,
            OperationId::Delete,
            Some(target),
        )?;
        if !target.exists() {
            return Err(effect_failure(
                PortId::RunStore,
                OperationId::Delete,
                FailureKind::NotFound,
                Some(target),
                "delete target was not found",
            ));
        }
        if !self.dry_run {
            let tombstone = sibling_path(
                target,
                artifact_suffix(self.next_artifact, "delete"),
                OperationId::Delete,
            )?;
            if tombstone.exists() {
                return Err(effect_failure(
                    PortId::RunStore,
                    OperationId::Delete,
                    FailureKind::Conflict,
                    Some(&tombstone),
                    "private deletion tombstone already exists",
                ));
            }
            std::fs::rename(target, &tombstone).map_err(|error| {
                io_failure(PortId::RunStore, OperationId::Delete, target, error)
            })?;
            self.sibling_residue.insert(tombstone.clone());
            // Tombstone deletion is cleanup after atomic removal from the authoritative path.
            if std::fs::remove_file(&tombstone).is_ok() {
                self.sibling_residue.remove(&tombstone);
            }
        }
        self.audit.push(WriteAuditEntry::new(
            WriteAuditAction::Delete,
            target.to_path_buf(),
            0,
        ));
        Ok(())
    }

    /// Returns ordered mutation facts accumulated by this deterministic run.
    fn audit(&self) -> &[WriteAuditEntry] {
        &self.audit
    }

    /// Reports exact scratch and target-volume residue still owned by this run.
    fn residue(&mut self) -> Result<ResidueReport, PortFailure> {
        take_fault(
            &self.faults,
            PortId::RunStore,
            OperationId::ReportResidue,
            Some(&self.scratch_directory),
        )?;
        residue_report(&self.scratch_directory, &mut self.sibling_residue)
    }

    /// Removes tracked private artifacts and reports anything that remains.
    fn cleanup(&mut self) -> Result<ResidueReport, PortFailure> {
        take_fault(
            &self.faults,
            PortId::RunStore,
            OperationId::Cleanup,
            Some(&self.scratch_directory),
        )?;
        for path in self.sibling_residue.clone() {
            if path.exists() && std::fs::remove_file(&path).is_ok() {
                self.sibling_residue.remove(&path);
            }
        }
        if self.scratch_directory.exists() {
            std::fs::remove_dir_all(&self.scratch_directory).map_err(|error| {
                io_failure(
                    PortId::RunStore,
                    OperationId::Cleanup,
                    &self.scratch_directory,
                    error,
                )
            })?;
        }
        self.artifacts.clear();
        self.staged_bytes = 0;
        residue_report(&self.scratch_directory, &mut self.sibling_residue)
    }
}

struct DeterministicRunLog {
    path: PathBuf,
    file: Option<File>,
    byte_len: u64,
    faults: Arc<Mutex<FaultPlan>>,
}

impl DeterministicRunLog {
    /// Releases a completed deterministic log so retention can delete it immediately.
    fn close(&mut self) {
        self.file.take();
    }
}

impl RunLog for DeterministicRunLog {
    /// Appends one UTF-8 record and newline within the active-log byte bound.
    fn write(&mut self, record: &str) -> Result<(), PortFailure> {
        take_fault(
            &self.faults,
            PortId::RunLog,
            OperationId::WriteLog,
            Some(&self.path),
        )?;
        let record_bytes = u64::try_from(record.len())
            .ok()
            .and_then(|length| length.checked_add(1))
            .and_then(|length| self.byte_len.checked_add(length))
            .ok_or_else(|| {
                resource_failure(
                    PortId::RunLog,
                    OperationId::WriteLog,
                    &self.path,
                    "run log byte count overflowed",
                )
            })?;
        if record_bytes > MAX_RUN_LOG_BYTES {
            return Err(resource_failure(
                PortId::RunLog,
                OperationId::WriteLog,
                &self.path,
                "run log exceeded its per-run byte bound",
            ));
        }
        let file = self.file.as_mut().ok_or_else(|| {
            effect_failure(
                PortId::RunLog,
                OperationId::WriteLog,
                FailureKind::Internal,
                Some(&self.path),
                "completed deterministic run log is closed",
            )
        })?;
        file.write_all(record.as_bytes())
            .and_then(|()| file.write_all(b"\n"))
            .map_err(|error| {
                io_failure(PortId::RunLog, OperationId::WriteLog, &self.path, error)
            })?;
        self.byte_len = record_bytes;
        Ok(())
    }

    /// Flushes and synchronizes the deterministic durable log unless a fault is injected.
    fn flush(&mut self) -> Result<(), PortFailure> {
        take_fault(
            &self.faults,
            PortId::RunLog,
            OperationId::FlushLog,
            Some(&self.path),
        )?;
        let Some(file) = self.file.as_mut() else {
            return Ok(());
        };
        file.flush()
            .and_then(|()| file.sync_all())
            .map_err(|error| io_failure(PortId::RunLog, OperationId::FlushLog, &self.path, error))
    }
}

struct DeterministicClock {
    next_millis: u64,
    faults: Arc<Mutex<FaultPlan>>,
}

impl Clock for DeterministicClock {
    /// Returns the next deterministic millisecond timestamp or an injected stable failure.
    fn now(&mut self) -> Result<TimestampMillis, PortFailure> {
        take_fault(&self.faults, PortId::Clock, OperationId::ReadClock, None)?;
        let observed = self.next_millis;
        self.next_millis = self.next_millis.checked_add(1).ok_or_else(|| {
            effect_failure(
                PortId::Clock,
                OperationId::ReadClock,
                FailureKind::ResourceExhausted,
                None,
                "deterministic clock exhausted",
            )
        })?;
        Ok(TimestampMillis::new(observed))
    }
}

struct DeterministicIdSource {
    next: u128,
    faults: Arc<Mutex<FaultPlan>>,
}

impl IdSource for DeterministicIdSource {
    /// Returns the next deterministic identifier or an injected stable failure.
    fn next_id(&mut self) -> Result<RunId, PortFailure> {
        take_fault(
            &self.faults,
            PortId::Identity,
            OperationId::GenerateId,
            None,
        )?;
        let observed = self.next;
        self.next = self.next.checked_add(1).ok_or_else(|| {
            effect_failure(
                PortId::Identity,
                OperationId::GenerateId,
                FailureKind::ResourceExhausted,
                None,
                "deterministic identity source exhausted",
            )
        })?;
        Ok(RunId::new(observed))
    }
}

struct DeterministicOneShotProcess {
    script: DeterministicProcessScript,
    faults: Arc<Mutex<FaultPlan>>,
}

impl OneShotProcess for DeterministicOneShotProcess {
    /// Returns scripted bounded facts after deterministic fault, cancellation, and timeout
    /// control flow is applied.
    fn execute(
        &mut self,
        request: &ProcessRequest,
        cancellation: &dyn CancellationProbe,
    ) -> Result<ProcessFacts, PortFailure> {
        require_absolute(
            PortId::Process,
            OperationId::ExecuteProcess,
            request.executable(),
        )?;
        require_absolute(
            PortId::Process,
            OperationId::ExecuteProcess,
            request.working_directory(),
        )?;
        take_fault(
            &self.faults,
            PortId::Process,
            OperationId::ExecuteProcess,
            Some(request.executable()),
        )?;
        let termination = if cancellation.is_cancelled() {
            ProcessTermination::Cancelled
        } else if request.timeout().is_zero() {
            ProcessTermination::TimedOut
        } else {
            ProcessTermination::Exited(self.script.exit_code)
        };
        Ok(ProcessFacts::new(
            termination,
            bounded_output(&self.script.stdout, request.output_limit()),
            bounded_output(&self.script.stderr, request.output_limit()),
        ))
    }
}

/// Retains a bounded prefix while recording whether additional bytes were drained.
fn bounded_output(bytes: &[u8], limit: usize) -> ProcessOutput {
    ProcessOutput::new(
        bytes[..bytes.len().min(limit)].to_vec(),
        bytes.len() > limit,
    )
}

/// Prunes oldest completed deterministic logs until both production bounds hold.
fn prune_completed_logs(logs: &Path, active: &BTreeSet<PathBuf>) -> Result<(), PortFailure> {
    let mut completed = Vec::new();
    for entry in std::fs::read_dir(logs)
        .map_err(|error| io_failure(PortId::RunLog, OperationId::Cleanup, logs, error))?
    {
        let path = entry
            .map_err(|error| io_failure(PortId::RunLog, OperationId::Cleanup, logs, error))?
            .path();
        if active.contains(&path)
            || path.extension().and_then(|extension| extension.to_str()) != Some("log")
        {
            continue;
        }
        let metadata = path
            .metadata()
            .map_err(|error| io_failure(PortId::RunLog, OperationId::Cleanup, &path, error))?;
        if metadata.is_file() {
            completed.push((path, metadata.len()));
        }
    }
    completed.sort_by(|left, right| left.0.file_name().cmp(&right.0.file_name()));
    let mut retained_bytes: u64 = completed.iter().map(|(_, byte_len)| *byte_len).sum();
    while completed.len() > MAX_RETAINED_RUN_LOGS || retained_bytes > MAX_RETAINED_RUN_LOG_BYTES {
        let (path, byte_len) = completed.remove(0);
        std::fs::remove_file(&path)
            .map_err(|error| io_failure(PortId::RunLog, OperationId::Cleanup, &path, error))?;
        retained_bytes = retained_bytes.saturating_sub(byte_len);
    }
    Ok(())
}

/// Clones active-log bookkeeping while mapping poisoned synchronization to a port failure.
fn active_log_snapshot(
    active_logs: &Arc<Mutex<BTreeSet<PathBuf>>>,
    subject: &Path,
) -> Result<BTreeSet<PathBuf>, PortFailure> {
    active_logs
        .lock()
        .map(|active| active.clone())
        .map_err(|_| active_log_lock_failure(subject))
}

/// Creates the stable failure returned when active-log bookkeeping is unavailable.
fn active_log_lock_failure(subject: &Path) -> PortFailure {
    effect_failure(
        PortId::RunLog,
        OperationId::Cleanup,
        FailureKind::Internal,
        Some(subject),
        "deterministic active run-log bookkeeping lock was poisoned",
    )
}

/// Best-effort removes every artifact created before a run environment becomes usable.
fn discard_failed_run_initialization(scratch_directory: &Path, log_path: Option<&Path>) {
    if let Some(log_path) = log_path {
        let _ = std::fs::remove_file(log_path);
    }
    let _ = std::fs::remove_dir_all(scratch_directory);
}

/// Builds an exact deterministic inventory of private files below one scratch root.
fn residue_report(
    scratch_directory: &Path,
    sibling_residue: &mut BTreeSet<PathBuf>,
) -> Result<ResidueReport, PortFailure> {
    let mut pending = if scratch_directory.exists() {
        vec![scratch_directory.to_path_buf()]
    } else {
        Vec::new()
    };
    let mut residue = Vec::new();
    while let Some(directory) = pending.pop() {
        let children = std::fs::read_dir(&directory).map_err(|error| {
            io_failure(
                PortId::RunStore,
                OperationId::ReportResidue,
                &directory,
                error,
            )
        })?;
        for child in children {
            let child = child.map_err(|error| {
                io_failure(
                    PortId::RunStore,
                    OperationId::ReportResidue,
                    &directory,
                    error,
                )
            })?;
            let path = child.path();
            let metadata = child.metadata().map_err(|error| {
                io_failure(PortId::RunStore, OperationId::ReportResidue, &path, error)
            })?;
            if metadata.is_dir() {
                pending.push(path);
            } else {
                residue.push(Residue::new(path, metadata.len()));
            }
        }
    }
    sibling_residue.retain(|path| path.exists());
    for path in sibling_residue.iter() {
        let metadata = std::fs::metadata(path).map_err(|error| {
            io_failure(PortId::RunStore, OperationId::ReportResidue, path, error)
        })?;
        residue.push(Residue::new(path.clone(), metadata.len()));
    }
    residue.sort_by(|left, right| left.path().cmp(right.path()));
    Ok(ResidueReport::new(residue))
}

/// Derives a collision-resistant private sibling without changing filesystem volume.
fn sibling_path(
    target: &Path,
    suffix: String,
    operation: OperationId,
) -> Result<PathBuf, PortFailure> {
    let parent = target.parent().ok_or_else(|| {
        effect_failure(
            PortId::RunStore,
            operation,
            FailureKind::InvalidInput,
            Some(target),
            "asset target has no containing directory",
        )
    })?;
    let leaf = target.file_name().ok_or_else(|| {
        effect_failure(
            PortId::RunStore,
            operation,
            FailureKind::InvalidInput,
            Some(target),
            "asset target has no file name",
        )
    })?;
    let mut sibling = leaf.to_os_string();
    sibling.push(suffix);
    Ok(parent.join(sibling))
}

/// Formats one adapter-private sibling suffix from a unique staged identity.
fn artifact_suffix(identity: u64, role: &str) -> String {
    format!(".tracetide-{identity:016x}-{role}")
}

/// Consumes one fault matching the stable operation and optional fixture path.
fn take_fault(
    faults: &Arc<Mutex<FaultPlan>>,
    port: PortId,
    operation: OperationId,
    actual_path: Option<&Path>,
) -> Result<(), PortFailure> {
    let failure = faults
        .lock()
        .map_err(|_| {
            effect_failure(
                port,
                operation,
                FailureKind::Internal,
                actual_path,
                "run-effects fault-plan lock was poisoned",
            )
        })?
        .take(operation, actual_path);
    match failure {
        Some(failure) => Err(failure),
        None => Ok(()),
    }
}

/// Rejects paths whose meaning would depend on an ambient working directory.
fn require_absolute(port: PortId, operation: OperationId, path: &Path) -> Result<(), PortFailure> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(effect_failure(
            port,
            operation,
            FailureKind::InvalidInput,
            Some(path),
            "effect path must be absolute",
        ))
    }
}

/// Maps one deterministic filesystem error into stable application vocabulary.
fn io_failure(
    port: PortId,
    operation: OperationId,
    path: &Path,
    error: std::io::Error,
) -> PortFailure {
    let kind = match error.kind() {
        std::io::ErrorKind::NotFound => FailureKind::NotFound,
        std::io::ErrorKind::PermissionDenied => FailureKind::PermissionDenied,
        std::io::ErrorKind::AlreadyExists => FailureKind::Conflict,
        _ => FailureKind::Io,
    };
    effect_failure(port, operation, kind, Some(path), error.to_string())
}

/// Creates one stable bounded-resource failure for a filesystem subject.
fn resource_failure(
    port: PortId,
    operation: OperationId,
    path: &Path,
    diagnostic: &str,
) -> PortFailure {
    effect_failure(
        port,
        operation,
        FailureKind::ResourceExhausted,
        Some(path),
        diagnostic,
    )
}

/// Creates a CAO-owned failure with an optional deterministic affected path.
fn effect_failure(
    port: PortId,
    operation: OperationId,
    kind: FailureKind,
    path: Option<&Path>,
    diagnostic: impl Into<String>,
) -> PortFailure {
    let failure = PortFailure::new(port, operation, kind, diagnostic);
    match path {
        Some(path) => failure.with_subject(path.display().to_string()),
        None => failure,
    }
}

/// Returns the stable built-in profile component used in deterministic log names.
fn profile_slug(profile: ActiveProfileId) -> &'static str {
    match profile {
        ActiveProfileId::Fo4 => "FO4",
        ActiveProfileId::Sse => "SSE",
        ActiveProfileId::Tes5 => "TES5",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DeterministicProcessScript, DeterministicRunEnvironmentFactory,
        assert_failed_atomic_replace_conformance, assert_run_effects_conformance,
        assert_run_log_retention_conformance, assert_run_store_bounds_conformance,
    };
    use crate::FaultPlan;
    use cao_application::{
        ActiveProfileId, CancellationProbe, FailureKind, OperationId, PortFailure, PortId,
        ProcessRequest, ProcessTermination, RunEnvironmentFactory, RunEnvironmentRequest,
        RunStoreLimits, WriteAuditAction,
    };
    use cao_platform_windows::{WindowsAtomicFilePublisher, WindowsRunEnvironmentFactory};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    struct Cancellation(AtomicBool);

    impl CancellationProbe for Cancellation {
        fn is_cancelled(&self) -> bool {
            self.0.load(Ordering::Relaxed)
        }
    }

    fn sandbox(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "tracetide-run-effects-{name}-{}-{}",
            std::process::id(),
            unique_test_id()
        ));
        std::fs::create_dir_all(&path).expect("run-effects sandbox should be created");
        path
    }

    fn unique_test_id() -> u64 {
        use std::sync::atomic::AtomicU64;
        static NEXT: AtomicU64 = AtomicU64::new(1);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    /// Provides the platform leaf capability beneath either RunStore adapter under test.
    fn atomic_files() -> Arc<dyn cao_application::AtomicFilePublisher> {
        Arc::new(WindowsAtomicFilePublisher)
    }

    #[test]
    fn deterministic_factory_passes_shared_run_effects_conformance() {
        let root = sandbox("conformance");
        let asset_root = root.join("assets");
        std::fs::create_dir_all(&asset_root).expect("asset root should be created");
        let factory = DeterministicRunEnvironmentFactory::new(root.join("effects"), atomic_files());

        assert_run_effects_conformance(&factory, &asset_root)
            .expect("deterministic run effects should satisfy the shared contract");
        assert_run_log_retention_conformance(&DeterministicRunEnvironmentFactory::new(
            root.join("retention-effects"),
            atomic_files(),
        ))
        .expect("deterministic run logs should satisfy shared retention");
        let bounded = DeterministicRunEnvironmentFactory::with_configuration(
            root.join("bounded-effects"),
            tight_store_limits(),
            FaultPlan::default(),
            DeterministicProcessScript::default(),
            atomic_files(),
        );
        assert_run_store_bounds_conformance(&bounded, &root.join("bounded-assets"))
            .expect("deterministic store should satisfy shared bounds");
        assert_failed_atomic_replace_conformance(
            &DeterministicRunEnvironmentFactory::new(root.join("atomic-effects"), atomic_files()),
            &root.join("atomic-assets"),
        )
        .expect("deterministic store should satisfy shared atomic failure semantics");
    }

    #[test]
    fn conformance_process_probe() {
        // The shared suite only needs a stable zero-exit helper with no ambient environment.
    }

    #[test]
    fn failed_environment_initialization_removes_unique_scratch() {
        let root = sandbox("failed-initialization");
        let effects = root.join("effects");
        std::fs::create_dir_all(&effects).expect("effect root should be created");
        std::fs::write(effects.join("logs"), b"log-directory conflict")
            .expect("log conflict fixture should be written");
        let factory = DeterministicRunEnvironmentFactory::new(effects.clone(), atomic_files());

        assert!(
            factory
                .create(RunEnvironmentRequest::new(ActiveProfileId::Sse, false))
                .is_err(),
            "log directory conflict must fail initialization"
        );

        assert_eq!(
            std::fs::read_dir(effects.join("scratch"))
                .expect("scratch parent should remain enumerable")
                .count(),
            0,
            "failed initialization must remove its unique run directory"
        );
    }

    #[test]
    fn windows_factory_passes_shared_run_effects_conformance() {
        let root = sandbox("windows-conformance");
        let executable = root.join("tracetide.exe");
        std::fs::write(&executable, b"verification executable")
            .expect("verification executable should be written");
        let asset_root = root.join("assets");
        let factory = WindowsRunEnvironmentFactory::for_executable(&executable)
            .expect("Windows run factory should resolve its executable root");

        assert_run_effects_conformance(&factory, &asset_root)
            .expect("Windows run effects should satisfy the shared contract");
        assert_run_log_retention_conformance(&factory)
            .expect("Windows run logs should satisfy shared retention");
        let bounded = WindowsRunEnvironmentFactory::for_executable(&executable)
            .expect("bounded Windows run factory should resolve its executable root")
            .with_store_limits(tight_store_limits());
        assert_run_store_bounds_conformance(&bounded, &root.join("bounded-assets"))
            .expect("Windows store should satisfy shared bounds");
        assert_failed_atomic_replace_conformance(&factory, &root.join("atomic-assets"))
            .expect("Windows store should satisfy shared atomic failure semantics");
    }

    /// Returns the deliberately narrow bounds required by the shared bounds suite.
    fn tight_store_limits() -> RunStoreLimits {
        RunStoreLimits {
            max_inventory_entries: 1,
            max_read_bytes: 4,
            max_operations: 1,
            max_staged_bytes: 4,
        }
    }

    #[test]
    fn dry_run_audits_replace_and_delete_without_mutating_assets() {
        let root = sandbox("dry-run");
        let asset_root = root.join("assets");
        std::fs::create_dir_all(&asset_root).expect("asset root should be created");
        let target = asset_root.join("mesh.nif");
        std::fs::write(&target, b"original").expect("target fixture should be written");
        let factory = DeterministicRunEnvironmentFactory::new(root.join("effects"), atomic_files());
        let mut environment = factory
            .create(RunEnvironmentRequest::new(ActiveProfileId::Sse, true))
            .expect("dry-run environment should be created");

        let artifact = environment
            .store()
            .stage(b"replacement")
            .expect("replacement should stage");
        environment
            .store()
            .commit_replace(artifact, &target)
            .expect("dry-run replacement should audit");
        environment
            .store()
            .delete(&target)
            .expect("dry-run deletion should audit");

        assert_eq!(
            std::fs::read(&target).expect("dry run must preserve the target"),
            b"original"
        );
        let audit = environment.store().audit();
        assert_eq!(audit.len(), 2);
        assert_eq!(audit[0].action(), WriteAuditAction::Replace);
        assert_eq!(audit[1].action(), WriteAuditAction::Delete);
        assert!(
            environment
                .finalize()
                .expect("finalize should clean")
                .is_empty()
        );
    }

    #[test]
    fn operation_bounds_cover_zero_byte_staging_and_accumulated_audit() {
        let root = sandbox("operation-bounds");
        let assets = root.join("assets");
        std::fs::create_dir_all(&assets).expect("asset root should be created");
        let limits = RunStoreLimits {
            max_inventory_entries: 8,
            max_read_bytes: 8,
            max_operations: 1,
            max_staged_bytes: 8,
        };
        let factory = DeterministicRunEnvironmentFactory::with_configuration(
            root.join("effects"),
            limits,
            FaultPlan::default(),
            DeterministicProcessScript::default(),
            atomic_files(),
        );
        let mut environment = factory
            .create(RunEnvironmentRequest::new(ActiveProfileId::Sse, false))
            .expect("bounded environment should be created");

        let first = environment
            .store()
            .stage(b"")
            .expect("first zero-byte artifact should stage");
        assert_eq!(
            environment
                .store()
                .stage(b"")
                .expect_err("a second zero-byte artifact must still consume an operation")
                .kind(),
            FailureKind::ResourceExhausted
        );
        environment
            .store()
            .commit_replace(first, &assets.join("first.bin"))
            .expect("first audit record should fit");
        let second = environment
            .store()
            .stage(b"next")
            .expect("consuming an artifact should free staging capacity");
        assert_eq!(
            environment
                .store()
                .commit_replace(second, &assets.join("second.bin"))
                .expect_err("a second audit record must exceed its operation bound")
                .kind(),
            FailureKind::ResourceExhausted
        );
        assert!(!assets.join("second.bin").exists());
    }

    #[test]
    fn injected_commit_fault_preserves_original_before_sibling_publication() {
        let root = sandbox("commit-fault");
        let assets = root.join("assets");
        std::fs::create_dir_all(&assets).expect("asset root should be created");
        let target = assets.join("protected.bin");
        std::fs::write(&target, b"original").expect("original should be written");
        let failure = PortFailure::new(
            PortId::RunStore,
            OperationId::CommitReplace,
            FailureKind::Io,
            "injected commit failure",
        );
        let factory = DeterministicRunEnvironmentFactory::with_configuration(
            root.join("effects"),
            RunStoreLimits::default(),
            FaultPlan::fail_once_at(OperationId::CommitReplace, "protected.bin", failure.clone()),
            DeterministicProcessScript::default(),
            atomic_files(),
        );
        let mut environment = factory
            .create(RunEnvironmentRequest::new(ActiveProfileId::Sse, false))
            .expect("faulted environment should be created");
        let artifact = environment
            .store()
            .stage(b"replacement")
            .expect("replacement should stage");

        assert_eq!(
            environment.store().commit_replace(artifact, &target),
            Err(failure)
        );
        assert_eq!(
            std::fs::read(&target).expect("original should remain readable"),
            b"original"
        );
        assert_eq!(
            std::fs::read_dir(&assets)
                .expect("asset directory should be readable")
                .count(),
            1,
            "an injected pre-commit fault must not create target siblings"
        );
        assert_eq!(
            environment
                .store()
                .residue()
                .expect("residue should report")
                .entries()
                .len(),
            1
        );
    }

    #[test]
    fn operation_and_path_keyed_process_fault_fires_only_for_matching_helper() {
        let root = sandbox("process-fault");
        let helper = root.join("bin/helper.exe");
        let other = root.join("bin/other.exe");
        let working = root.join("scratch");
        std::fs::create_dir_all(&working).expect("working directory should be created");
        let failure = PortFailure::new(
            PortId::Process,
            OperationId::ExecuteProcess,
            FailureKind::BackendCrashed,
            "injected helper crash",
        );
        let factory = DeterministicRunEnvironmentFactory::with_configuration(
            root.join("effects"),
            RunStoreLimits::default(),
            FaultPlan::fail_once_at(OperationId::ExecuteProcess, "helper.exe", failure.clone()),
            DeterministicProcessScript::new(b"abcdef".to_vec(), b"uvwxyz".to_vec(), Some(7)),
            atomic_files(),
        );
        let mut environment = factory
            .create(RunEnvironmentRequest::new(ActiveProfileId::Sse, false))
            .expect("environment should be created");
        let cancellation = Cancellation(AtomicBool::new(false));
        let other_request = process_request(&other, &working, 3);
        let matching_request = process_request(&helper, &working, 3);

        let facts = environment
            .process()
            .execute(&other_request, &cancellation)
            .expect("non-matching helper should execute");
        assert_eq!(facts.termination(), ProcessTermination::Exited(Some(7)));
        assert_eq!(facts.stdout().bytes(), b"abc");
        assert!(facts.stdout().was_truncated());
        assert_eq!(facts.stderr().bytes(), b"uvw");
        assert!(facts.stderr().was_truncated());

        assert_eq!(
            environment
                .process()
                .execute(&matching_request, &cancellation),
            Err(failure)
        );
    }

    #[test]
    fn deterministic_clock_ids_timeout_and_cancellation_are_stable_control_flow() {
        let root = sandbox("deterministic-seams");
        let working = root.join("scratch");
        std::fs::create_dir_all(&working).expect("working directory should be created");
        let factory = DeterministicRunEnvironmentFactory::new(root.join("effects"), atomic_files());
        let mut environment = factory
            .create(RunEnvironmentRequest::new(ActiveProfileId::Tes5, false))
            .expect("environment should be created");

        let first_time = environment.clock().now().expect("clock should be readable");
        let second_time = environment.clock().now().expect("clock should advance");
        assert_eq!(second_time.get(), first_time.get() + 1);
        let first_id = environment.ids().next_id().expect("first ID should exist");
        let second_id = environment.ids().next_id().expect("second ID should exist");
        assert_eq!(second_id.get(), first_id.get() + 1);

        let helper = root.join("bin/helper.exe");
        let timed_out = ProcessRequest::new(
            helper.clone(),
            working.clone(),
            Vec::new(),
            Vec::new(),
            Duration::ZERO,
            8,
        );
        let active = Cancellation(AtomicBool::new(false));
        assert_eq!(
            environment
                .process()
                .execute(&timed_out, &active)
                .expect("timeout is structured control flow")
                .termination(),
            ProcessTermination::TimedOut
        );

        let cancelled = Cancellation(AtomicBool::new(true));
        assert_eq!(
            environment
                .process()
                .execute(&process_request(&helper, &working, 8), &cancelled)
                .expect("cancellation is structured control flow")
                .termination(),
            ProcessTermination::Cancelled
        );
    }

    fn process_request(executable: &Path, working: &Path, output_limit: usize) -> ProcessRequest {
        ProcessRequest::new(
            executable.to_path_buf(),
            working.to_path_buf(),
            vec!["request.json".to_owned()],
            vec![("SYSTEMROOT".to_owned(), "C:\\Windows".to_owned())],
            Duration::from_secs(5),
            output_limit,
        )
    }
}
