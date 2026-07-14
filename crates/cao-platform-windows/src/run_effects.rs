//! Bounded production filesystem, logging, clock, and identity effects for one run.

use crate::WindowsAtomicFilePublisher;
use crate::process::WindowsOneShotProcess;
use crate::{FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};
use cao_application::{
    ActiveProfileId, AtomicFilePublisher, Clock, FailureKind, IdSource, InventoryEntry,
    MAX_RETAINED_RUN_LOG_BYTES, MAX_RETAINED_RUN_LOGS, MAX_RUN_LOG_BYTES, OneShotProcess,
    OperationId, PortFailure, PortId, Residue, ResidueReport, RunEnvironment,
    RunEnvironmentFactory, RunEnvironmentRequest, RunId, RunLog, RunStore, RunStoreLimits,
    StagedArtifact, TimestampMillis, WriteAuditAction, WriteAuditEntry,
};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_WINDOWS_RUN: AtomicU64 = AtomicU64::new(1);

/// Production source of fresh bounded effects for one Windows processing run.
#[derive(Clone, Debug)]
pub struct WindowsRunEnvironmentFactory {
    executable_root: PathBuf,
    limits: RunStoreLimits,
    active_logs: Arc<Mutex<BTreeSet<PathBuf>>>,
}

impl WindowsRunEnvironmentFactory {
    /// Captures the canonical executable root used for durable run logs.
    ///
    /// # Errors
    ///
    /// Returns a stable run-environment failure when the executable is relative,
    /// missing, a directory, or a Windows reparse point.
    pub fn for_executable(executable: impl AsRef<Path>) -> Result<Self, PortFailure> {
        let portable = crate::WindowsPortableStateFactory::for_executable(executable)?;
        Ok(portable.run_environment_factory())
    }

    /// Builds per-run effects from the canonical root captured by platform startup.
    pub(crate) fn for_executable_root(executable_root: PathBuf) -> Self {
        debug_assert!(executable_root.is_absolute());
        Self {
            executable_root: without_verbatim_prefix(&executable_root),
            limits: RunStoreLimits::default(),
            active_logs: Arc::new(Mutex::new(BTreeSet::new())),
        }
    }

    /// Returns a factory with explicit per-operation filesystem bounds.
    #[must_use]
    pub fn with_store_limits(mut self, limits: RunStoreLimits) -> Self {
        self.limits = limits;
        self
    }
}

impl RunEnvironmentFactory for WindowsRunEnvironmentFactory {
    /// Allocates a unique scratch tree and durable UTF-8 log, then returns fresh run capabilities.
    fn create(
        &self,
        request: RunEnvironmentRequest,
    ) -> Result<Box<dyn RunEnvironment>, PortFailure> {
        let scratch_parent = std::env::temp_dir().join("Tracetide");
        std::fs::create_dir_all(&scratch_parent).map_err(|error| {
            io_failure(
                PortId::RunEnvironment,
                OperationId::CreateRunEnvironment,
                &scratch_parent,
                error,
            )
        })?;
        validate_effect_path(&scratch_parent, false, OperationId::CreateRunEnvironment)?;
        let (timestamp, run_id, scratch_directory) = loop {
            let ordinal = NEXT_WINDOWS_RUN
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                    value.checked_add(1)
                })
                .map_err(|_| {
                    effect_failure(
                        PortId::Identity,
                        OperationId::GenerateId,
                        FailureKind::ResourceExhausted,
                        None,
                        "Windows run identity counter exhausted",
                    )
                })?;
            let timestamp = system_timestamp()?;
            let run_id = RunId::new(
                (u128::from(timestamp.get()) << 64)
                    | (u128::from(std::process::id()) << 32)
                    | u128::from(ordinal),
            );
            let candidate = scratch_parent.join(format!("run-{:032x}", run_id.get()));
            match std::fs::create_dir(&candidate) {
                Ok(()) => break (timestamp, run_id, candidate),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    // A stale crash residue never acquires meaning for a fresh processing run.
                }
                Err(error) => {
                    return Err(io_failure(
                        PortId::RunEnvironment,
                        OperationId::CreateRunEnvironment,
                        &candidate,
                        error,
                    ));
                }
            }
        };
        let staging_directory = scratch_directory.join("staged");
        if let Err(error) = std::fs::create_dir(&staging_directory) {
            // The unique scratch root is inactive until every required run capability exists.
            let _ = std::fs::remove_dir_all(&scratch_directory);
            return Err(io_failure(
                PortId::RunEnvironment,
                OperationId::CreateRunEnvironment,
                &staging_directory,
                error,
            ));
        }

        let logs = self.executable_root.join("data/logs");
        if let Err(failure) = create_log_directory(&self.executable_root, &logs) {
            let _ = std::fs::remove_dir_all(&scratch_directory);
            return Err(failure);
        }
        let active = match active_log_snapshot(&self.active_logs, &logs) {
            Ok(active) => active,
            Err(failure) => {
                let _ = std::fs::remove_dir_all(&scratch_directory);
                return Err(failure);
            }
        };
        if let Err(failure) = prune_completed_logs(&logs, &active) {
            let _ = std::fs::remove_dir_all(&scratch_directory);
            return Err(failure);
        }
        let log_path = logs.join(format!(
            "{}-{}-{:032x}.log",
            timestamp.get(),
            profile_slug(request.profile_id()),
            run_id.get()
        ));
        let mut log_file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&log_path)
        {
            Ok(file) => file,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&scratch_directory);
                return Err(io_failure(
                    PortId::RunLog,
                    OperationId::WriteLog,
                    &log_path,
                    error,
                ));
            }
        };
        let header = format!(
            "timestamp={} profile={} run_id={} dry_run={}\n",
            timestamp.get(),
            profile_slug(request.profile_id()),
            run_id.get(),
            request.dry_run()
        );
        if let Err(error) = log_file.write_all(header.as_bytes()) {
            drop(log_file);
            let _ = std::fs::remove_file(&log_path);
            let _ = std::fs::remove_dir_all(&scratch_directory);
            return Err(io_failure(
                PortId::RunLog,
                OperationId::WriteLog,
                &log_path,
                error,
            ));
        }
        match self.active_logs.lock() {
            Ok(mut active) => {
                active.insert(log_path.clone());
            }
            Err(_) => {
                drop(log_file);
                let _ = std::fs::remove_file(&log_path);
                let _ = std::fs::remove_dir_all(&scratch_directory);
                return Err(active_log_lock_failure(&log_path));
            }
        }

        Ok(Box::new(WindowsRunEnvironment {
            run_id,
            scratch_directory: scratch_directory.clone(),
            log_path: log_path.clone(),
            store: WindowsRunStore::new(
                scratch_directory,
                staging_directory,
                request.dry_run(),
                self.limits,
                run_id,
            ),
            log: WindowsRunLog {
                path: log_path,
                file: Some(log_file),
                bytes_written: header.len() as u64,
            },
            process: WindowsOneShotProcess,
            clock: WindowsClock,
            ids: WindowsIdSource {
                prefix: run_id.get() << 32,
                next: 1,
            },
            active_logs: Arc::clone(&self.active_logs),
            finalized: false,
        }))
    }
}

struct WindowsRunEnvironment {
    run_id: RunId,
    scratch_directory: PathBuf,
    log_path: PathBuf,
    store: WindowsRunStore,
    log: WindowsRunLog,
    process: WindowsOneShotProcess,
    clock: WindowsClock,
    ids: WindowsIdSource,
    active_logs: Arc<Mutex<BTreeSet<PathBuf>>>,
    finalized: bool,
}

impl WindowsRunEnvironment {
    /// Marks the durable log completed and enforces completed-log retention.
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
                "run log has no retention directory",
            )
        })?;
        prune_completed_logs(logs, &completed_snapshot)
    }
}

impl RunEnvironment for WindowsRunEnvironment {
    /// Returns the immutable identity assigned when this run environment was created.
    fn run_id(&self) -> RunId {
        self.run_id
    }
    /// Returns the unique private directory owned by this run.
    fn scratch_directory(&self) -> &Path {
        &self.scratch_directory
    }
    /// Returns the durable UTF-8 log path retained after scratch cleanup.
    fn log_path(&self) -> &Path {
        &self.log_path
    }
    /// Borrows this run's bounded filesystem capability.
    fn store(&mut self) -> &mut dyn RunStore {
        &mut self.store
    }
    /// Borrows this run's bounded durable log capability.
    fn log(&mut self) -> &mut dyn RunLog {
        &mut self.log
    }
    /// Borrows this run's contained one-shot process capability.
    fn process(&mut self) -> &mut dyn OneShotProcess {
        &mut self.process
    }
    /// Borrows this run's system clock capability.
    fn clock(&mut self) -> &mut dyn Clock {
        &mut self.clock
    }
    /// Borrows this run's monotonic identity source.
    fn ids(&mut self) -> &mut dyn IdSource {
        &mut self.ids
    }

    /// Flushes durable logging, prunes completed logs, and removes private residue.
    ///
    /// Repeated calls are safe and report any residue that remains after the first cleanup.
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

impl Drop for WindowsRunEnvironment {
    /// Best-effort finalizes capabilities that were not explicitly finalized by their owner.
    fn drop(&mut self) {
        if !self.finalized {
            // Drop cannot report finalization diagnostics; explicit finalization remains authoritative.
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

struct WindowsRunStore {
    scratch_directory: PathBuf,
    staging_directory: PathBuf,
    dry_run: bool,
    limits: RunStoreLimits,
    run_id: RunId,
    next_artifact: u64,
    staged_bytes: u64,
    artifacts: BTreeMap<u64, ArtifactState>,
    external_residue: BTreeMap<PathBuf, u64>,
    audit: Vec<WriteAuditEntry>,
}

impl WindowsRunStore {
    /// Creates one worker-owned bounded store over a unique scratch directory.
    fn new(
        scratch_directory: PathBuf,
        staging_directory: PathBuf,
        dry_run: bool,
        limits: RunStoreLimits,
        run_id: RunId,
    ) -> Self {
        Self {
            scratch_directory,
            staging_directory,
            dry_run,
            limits,
            run_id,
            next_artifact: 1,
            staged_bytes: 0,
            artifacts: BTreeMap::new(),
            external_residue: BTreeMap::new(),
            audit: Vec::new(),
        }
    }

    /// Returns one currently owned staged artifact.
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

    /// Removes consumed staging without changing an already-committed result on cleanup failure.
    fn consume_best_effort(&mut self, artifact: StagedArtifact) {
        let Some(state) = self.artifacts.get(&artifact.get()) else {
            return;
        };
        if std::fs::remove_file(&state.path).is_ok()
            && let Some(state) = self.artifacts.remove(&artifact.get())
        {
            self.staged_bytes = self.staged_bytes.saturating_sub(state.byte_len);
        }
    }

    /// Creates a collision-proof private sibling on the target's volume.
    fn target_sibling(
        &self,
        target: &Path,
        artifact: u64,
        suffix: &str,
    ) -> Result<PathBuf, PortFailure> {
        let name = target.file_name().ok_or_else(|| {
            effect_failure(
                PortId::RunStore,
                OperationId::CommitReplace,
                FailureKind::InvalidInput,
                Some(target),
                "asset target has no file name",
            )
        })?;
        let mut sibling = OsString::from(name);
        sibling.push(format!(
            ".tracetide-{:032x}-{artifact:016x}.{suffix}",
            self.run_id.get()
        ));
        Ok(target.with_file_name(sibling))
    }

    /// Ensures another ordered mutation can be audited before touching authority.
    fn require_audit_capacity(
        &self,
        target: &Path,
        operation: OperationId,
    ) -> Result<(), PortFailure> {
        if self.audit.len() >= self.limits.max_operations {
            Err(resource_failure(
                operation,
                target,
                "write audit exceeded its operation bound",
            ))
        } else {
            Ok(())
        }
    }
}

impl RunStore for WindowsRunStore {
    /// Recursively inventories ordinary entries beneath an absolute root within configured bounds.
    fn inventory(&mut self, root: &Path) -> Result<Vec<InventoryEntry>, PortFailure> {
        validate_effect_path(root, false, OperationId::Inventory)?;
        let mut pending = vec![root.to_path_buf()];
        let mut entries = Vec::new();
        while let Some(directory) = pending.pop() {
            for child in std::fs::read_dir(&directory).map_err(|error| {
                io_failure(PortId::RunStore, OperationId::Inventory, &directory, error)
            })? {
                let path = child
                    .map_err(|error| {
                        io_failure(PortId::RunStore, OperationId::Inventory, &directory, error)
                    })?
                    .path();
                validate_effect_path(&path, false, OperationId::Inventory)?;
                let metadata = path.symlink_metadata().map_err(|error| {
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

    /// Reads one ordinary file without following a reparse point or exceeding the byte limit.
    fn read(&mut self, path: &Path) -> Result<Vec<u8>, PortFailure> {
        validate_effect_path(path, false, OperationId::Read)?;
        let mut file = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .map_err(|error| io_failure(PortId::RunStore, OperationId::Read, path, error))?;
        let metadata = file
            .metadata()
            .map_err(|error| io_failure(PortId::RunStore, OperationId::Read, path, error))?;
        if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(effect_failure(
                PortId::RunStore,
                OperationId::Read,
                FailureKind::Integrity,
                Some(path),
                "bounded read requires an ordinary file",
            ));
        }
        if metadata.len() > self.limits.max_read_bytes as u64 {
            return Err(resource_failure(
                OperationId::Read,
                path,
                "file exceeded the bounded read limit",
            ));
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.read_to_end(&mut bytes)
            .map_err(|error| io_failure(PortId::RunStore, OperationId::Read, path, error))?;
        if bytes.len() > self.limits.max_read_bytes {
            return Err(resource_failure(
                OperationId::Read,
                path,
                "file grew beyond the bounded read limit",
            ));
        }
        Ok(bytes)
    }

    /// Persists bytes to a private run-owned artifact and returns its opaque handle.
    fn stage(&mut self, bytes: &[u8]) -> Result<StagedArtifact, PortFailure> {
        if self.artifacts.len() >= self.limits.max_operations {
            return Err(resource_failure(
                OperationId::StageWrite,
                &self.staging_directory,
                "private staging exceeded its operation bound",
            ));
        }
        let byte_len = u64::try_from(bytes.len()).map_err(|_| {
            resource_failure(
                OperationId::StageWrite,
                &self.staging_directory,
                "staged artifact length cannot be represented",
            )
        })?;
        let aggregate = self.staged_bytes.checked_add(byte_len).ok_or_else(|| {
            resource_failure(
                OperationId::StageWrite,
                &self.staging_directory,
                "aggregate staging byte count overflowed",
            )
        })?;
        if aggregate > self.limits.max_staged_bytes {
            return Err(resource_failure(
                OperationId::StageWrite,
                &self.staging_directory,
                "aggregate private staging exceeded its byte bound",
            ));
        }
        let artifact = StagedArtifact::new(self.next_artifact);
        let path = self
            .staging_directory
            .join(format!("artifact-{:016x}.stage", artifact.get()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&path)
            .map_err(|error| io_failure(PortId::RunStore, OperationId::StageWrite, &path, error))?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| io_failure(PortId::RunStore, OperationId::StageWrite, &path, error))?;
        self.next_artifact = self.next_artifact.checked_add(1).ok_or_else(|| {
            resource_failure(
                OperationId::StageWrite,
                &path,
                "staged artifact identity exhausted",
            )
        })?;
        self.staged_bytes = aggregate;
        self.artifacts
            .insert(artifact.get(), ArtifactState { path, byte_len });
        Ok(artifact)
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
        let actual = self.read_private(&path, OperationId::VerifyStaged)?;
        if actual == expected {
            Ok(())
        } else {
            Err(effect_failure(
                PortId::RunStore,
                OperationId::VerifyStaged,
                FailureKind::Integrity,
                Some(&path),
                "staged artifact did not match independently expected bytes",
            ))
        }
    }

    /// Atomically publishes a staged artifact at the target path, or records only an audit
    /// in dry-run mode.
    fn commit_replace(
        &mut self,
        artifact: StagedArtifact,
        target: &Path,
    ) -> Result<(), PortFailure> {
        validate_effect_target(target, OperationId::CommitReplace)?;
        self.require_audit_capacity(target, OperationId::CommitReplace)?;
        let state = self.artifact(artifact, OperationId::CommitReplace)?;
        let staged_path = state.path.clone();
        let byte_len = state.byte_len;
        let action = if target.exists() {
            WriteAuditAction::Replace
        } else {
            WriteAuditAction::Create
        };
        if self.dry_run {
            self.audit
                .push(WriteAuditEntry::new(action, target.to_path_buf(), byte_len));
            self.consume_best_effort(artifact);
            return Ok(());
        }
        let pending = self.target_sibling(target, artifact.get(), "pending")?;
        validate_effect_target(&pending, OperationId::CommitReplace)?;
        let mut source = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&staged_path)
            .map_err(|error| {
                io_failure(
                    PortId::RunStore,
                    OperationId::CommitReplace,
                    &staged_path,
                    error,
                )
            })?;
        let mut candidate = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&pending)
            .map_err(|error| {
                io_failure(
                    PortId::RunStore,
                    OperationId::CommitReplace,
                    &pending,
                    error,
                )
            })?;
        self.external_residue.insert(pending.clone(), 0);
        let copied = std::io::copy(&mut source, &mut candidate)
            .and_then(|count| candidate.sync_all().map(|()| count))
            .map_err(|error| {
                io_failure(
                    PortId::RunStore,
                    OperationId::CommitReplace,
                    &pending,
                    error,
                )
            })?;
        self.external_residue.insert(pending.clone(), copied);
        // MoveFileEx requires every non-sharing candidate handle to close before publication.
        drop(candidate);
        drop(source);
        WindowsAtomicFilePublisher.replace(&pending, target)?;
        self.external_residue.remove(&pending);
        self.audit
            .push(WriteAuditEntry::new(action, target.to_path_buf(), byte_len));
        self.consume_best_effort(artifact);
        Ok(())
    }

    /// Atomically removes an ordinary target from authority, or records only an audit in
    /// dry-run mode.
    fn delete(&mut self, target: &Path) -> Result<(), PortFailure> {
        validate_effect_path(target, false, OperationId::Delete)?;
        self.require_audit_capacity(target, OperationId::Delete)?;
        let metadata = target
            .symlink_metadata()
            .map_err(|error| io_failure(PortId::RunStore, OperationId::Delete, target, error))?;
        if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(effect_failure(
                PortId::RunStore,
                OperationId::Delete,
                FailureKind::Integrity,
                Some(target),
                "delete target must be an ordinary file",
            ));
        }
        if self.dry_run {
            self.audit.push(WriteAuditEntry::new(
                WriteAuditAction::Delete,
                target.to_path_buf(),
                0,
            ));
            return Ok(());
        }
        let tombstone = self.target_sibling(target, self.next_artifact, "delete")?;
        validate_effect_target(&tombstone, OperationId::Delete)?;
        WindowsAtomicFilePublisher
            .replace(target, &tombstone)
            .map_err(|failure| {
                effect_failure(
                    PortId::RunStore,
                    OperationId::Delete,
                    failure.kind(),
                    Some(target),
                    failure.diagnostic().as_str(),
                )
            })?;
        self.external_residue
            .insert(tombstone.clone(), metadata.len());
        if std::fs::remove_file(&tombstone).is_ok() {
            self.external_residue.remove(&tombstone);
        }
        self.audit.push(WriteAuditEntry::new(
            WriteAuditAction::Delete,
            target.to_path_buf(),
            0,
        ));
        Ok(())
    }

    /// Returns ordered mutation facts accumulated by this run.
    fn audit(&self) -> &[WriteAuditEntry] {
        &self.audit
    }

    /// Reports exact private scratch and target-volume residue still owned by this run.
    fn residue(&mut self) -> Result<ResidueReport, PortFailure> {
        self.build_residue()
    }

    /// Best-effort removes all tracked private artifacts and reports anything that remains.
    fn cleanup(&mut self) -> Result<ResidueReport, PortFailure> {
        let external: Vec<_> = self.external_residue.keys().cloned().collect();
        for path in external {
            if std::fs::remove_file(&path).is_ok() || !path.exists() {
                self.external_residue.remove(&path);
            }
        }
        if self.scratch_directory.exists() {
            let _ = std::fs::remove_dir_all(&self.scratch_directory);
        }
        if !self.scratch_directory.exists() {
            self.artifacts.clear();
            self.staged_bytes = 0;
        }
        self.build_residue()
    }
}

impl WindowsRunStore {
    /// Reads a private staged file through a no-follow handle.
    fn read_private(&self, path: &Path, operation: OperationId) -> Result<Vec<u8>, PortFailure> {
        let mut file = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .map_err(|error| io_failure(PortId::RunStore, operation, path, error))?;
        let metadata = file
            .metadata()
            .map_err(|error| io_failure(PortId::RunStore, operation, path, error))?;
        if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(effect_failure(
                PortId::RunStore,
                operation,
                FailureKind::Integrity,
                Some(path),
                "private artifact is not an ordinary file",
            ));
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.read_to_end(&mut bytes)
            .map_err(|error| io_failure(PortId::RunStore, operation, path, error))?;
        Ok(bytes)
    }

    /// Builds deterministic residue across scratch and target-volume private siblings.
    fn build_residue(&mut self) -> Result<ResidueReport, PortFailure> {
        let mut entries = Vec::new();
        if self.scratch_directory.exists() {
            let mut pending = vec![self.scratch_directory.clone()];
            while let Some(directory) = pending.pop() {
                for child in std::fs::read_dir(&directory).map_err(|error| {
                    io_failure(
                        PortId::RunStore,
                        OperationId::ReportResidue,
                        &directory,
                        error,
                    )
                })? {
                    let path = child
                        .map_err(|error| {
                            io_failure(
                                PortId::RunStore,
                                OperationId::ReportResidue,
                                &directory,
                                error,
                            )
                        })?
                        .path();
                    let metadata = path.symlink_metadata().map_err(|error| {
                        io_failure(PortId::RunStore, OperationId::ReportResidue, &path, error)
                    })?;
                    if metadata.is_dir() {
                        pending.push(path);
                    } else {
                        entries.push(Residue::new(path, metadata.len()));
                    }
                }
            }
        }
        let tracked: Vec<_> = self.external_residue.keys().cloned().collect();
        for path in tracked {
            match path.symlink_metadata() {
                Ok(metadata) => entries.push(Residue::new(path.clone(), metadata.len())),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    self.external_residue.remove(&path);
                }
                Err(error) => {
                    return Err(io_failure(
                        PortId::RunStore,
                        OperationId::ReportResidue,
                        &path,
                        error,
                    ));
                }
            }
        }
        entries.sort_by(|left, right| left.path().cmp(right.path()));
        Ok(ResidueReport::new(entries))
    }
}

struct WindowsRunLog {
    path: PathBuf,
    file: Option<File>,
    bytes_written: u64,
}

impl WindowsRunLog {
    /// Releases the completed log handle so retention can delete old logs immediately.
    fn close(&mut self) {
        self.file.take();
    }
}

impl RunLog for WindowsRunLog {
    /// Appends one UTF-8 record and newline without exceeding the active-log byte bound.
    fn write(&mut self, record: &str) -> Result<(), PortFailure> {
        let added = u64::try_from(record.len().saturating_add(1)).map_err(|_| {
            resource_failure(
                OperationId::WriteLog,
                &self.path,
                "run-log record length cannot be represented",
            )
        })?;
        let total = self.bytes_written.checked_add(added).ok_or_else(|| {
            resource_failure(
                OperationId::WriteLog,
                &self.path,
                "run-log byte count overflowed",
            )
        })?;
        if total > MAX_RUN_LOG_BYTES {
            return Err(resource_failure(
                OperationId::WriteLog,
                &self.path,
                "active run log exceeded its byte bound",
            ));
        }
        let file = self.file.as_mut().ok_or_else(|| {
            effect_failure(
                PortId::RunLog,
                OperationId::WriteLog,
                FailureKind::Internal,
                Some(&self.path),
                "completed run log is closed",
            )
        })?;
        file.write_all(record.as_bytes())
            .and_then(|()| file.write_all(b"\n"))
            .map_err(|error| {
                io_failure(PortId::RunLog, OperationId::WriteLog, &self.path, error)
            })?;
        self.bytes_written = total;
        Ok(())
    }

    /// Flushes userspace buffers and synchronizes the durable log file.
    fn flush(&mut self) -> Result<(), PortFailure> {
        let Some(file) = self.file.as_mut() else {
            return Ok(());
        };
        file.flush()
            .and_then(|()| file.sync_all())
            .map_err(|error| io_failure(PortId::RunLog, OperationId::FlushLog, &self.path, error))
    }
}

struct WindowsClock;
impl Clock for WindowsClock {
    /// Returns the current non-negative Unix-epoch timestamp in milliseconds.
    fn now(&mut self) -> Result<TimestampMillis, PortFailure> {
        system_timestamp()
    }
}

struct WindowsIdSource {
    prefix: u128,
    next: u64,
}
impl IdSource for WindowsIdSource {
    /// Returns the next run-scoped identifier or fails when its sequence is exhausted.
    fn next_id(&mut self) -> Result<RunId, PortFailure> {
        let value = self.prefix | u128::from(self.next);
        self.next = self.next.checked_add(1).ok_or_else(|| {
            effect_failure(
                PortId::Identity,
                OperationId::GenerateId,
                FailureKind::ResourceExhausted,
                None,
                "Windows run identity source exhausted",
            )
        })?;
        Ok(RunId::new(value))
    }
}

/// Returns the current non-negative Unix-epoch millisecond count.
fn system_timestamp() -> Result<TimestampMillis, PortFailure> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            effect_failure(
                PortId::Clock,
                OperationId::ReadClock,
                FailureKind::Internal,
                None,
                error.to_string(),
            )
        })?;
    let millis = u64::try_from(duration.as_millis()).map_err(|_| {
        effect_failure(
            PortId::Clock,
            OperationId::ReadClock,
            FailureKind::ResourceExhausted,
            None,
            "Windows timestamp cannot be represented",
        )
    })?;
    Ok(TimestampMillis::new(millis))
}

/// Removes the Win32 verbatim prefix while preserving the canonical absolute spelling.
fn without_verbatim_prefix(path: &Path) -> PathBuf {
    let text = path.as_os_str().to_string_lossy();
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{unc}"))
    } else if let Some(local) = text.strip_prefix(r"\\?\") {
        PathBuf::from(local)
    } else {
        path.to_path_buf()
    }
}

/// Creates and validates the executable-relative durable log directory.
fn create_log_directory(root: &Path, logs: &Path) -> Result<(), PortFailure> {
    let data = root.join("data");
    for path in [&data, logs] {
        if path.exists() {
            validate_managed_log_path(root, path)?;
        } else {
            std::fs::create_dir(path)
                .map_err(|error| io_failure(PortId::RunLog, OperationId::WriteLog, path, error))?;
            validate_managed_log_path(root, path)?;
        }
    }
    Ok(())
}

/// Confirms a log directory is ordinary and remains beneath the executable root.
fn validate_managed_log_path(root: &Path, path: &Path) -> Result<(), PortFailure> {
    let canonical = path
        .canonicalize()
        .map_err(|error| io_failure(PortId::RunLog, OperationId::WriteLog, path, error))?;
    let canonical = without_verbatim_prefix(&canonical);
    let metadata = path
        .symlink_metadata()
        .map_err(|error| io_failure(PortId::RunLog, OperationId::WriteLog, path, error))?;
    if !canonical.starts_with(root)
        || !metadata.is_dir()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err(effect_failure(
            PortId::RunLog,
            OperationId::WriteLog,
            FailureKind::Integrity,
            Some(path),
            "run-log directory escaped the executable root or is a reparse point",
        ));
    }
    Ok(())
}

/// Prunes oldest completed logs until both retention bounds hold.
fn prune_completed_logs(logs: &Path, active: &BTreeSet<PathBuf>) -> Result<(), PortFailure> {
    let mut completed = Vec::new();
    for entry in std::fs::read_dir(logs)
        .map_err(|error| io_failure(PortId::RunLog, OperationId::Cleanup, logs, error))?
    {
        let path = entry
            .map_err(|error| io_failure(PortId::RunLog, OperationId::Cleanup, logs, error))?
            .path();
        if active.contains(&path)
            || path.extension().and_then(|value| value.to_str()) != Some("log")
        {
            continue;
        }
        let metadata = path
            .symlink_metadata()
            .map_err(|error| io_failure(PortId::RunLog, OperationId::Cleanup, &path, error))?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(effect_failure(
                PortId::RunLog,
                OperationId::Cleanup,
                FailureKind::Integrity,
                Some(&path),
                "retained run log is a reparse point",
            ));
        }
        if metadata.is_file() {
            completed.push((path, metadata.len()));
        }
    }
    completed.sort_by(|left, right| left.0.file_name().cmp(&right.0.file_name()));
    let mut bytes: u64 = completed.iter().map(|(_, len)| *len).sum();
    while completed.len() > MAX_RETAINED_RUN_LOGS || bytes > MAX_RETAINED_RUN_LOG_BYTES {
        let (path, len) = completed.remove(0);
        std::fs::remove_file(&path)
            .map_err(|error| io_failure(PortId::RunLog, OperationId::Cleanup, &path, error))?;
        bytes = bytes.saturating_sub(len);
    }
    Ok(())
}

/// Clones the active-log set while mapping poisoned synchronization into port vocabulary.
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
        "active run-log bookkeeping lock was poisoned",
    )
}

/// Validates an absolute Windows path and rejects traversal and reparse components.
fn validate_effect_path(
    path: &Path,
    allow_missing_leaf: bool,
    operation: OperationId,
) -> Result<(), PortFailure> {
    if !path.is_absolute() {
        return Err(effect_failure(
            PortId::RunStore,
            operation,
            FailureKind::InvalidInput,
            Some(path),
            "effect path must be absolute",
        ));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(effect_failure(
            PortId::RunStore,
            operation,
            FailureKind::InvalidInput,
            Some(path),
            "effect path contains lexical traversal",
        ));
    }
    let mut current = PathBuf::new();
    let component_count = path.components().count();
    for (index, component) in path.components().enumerate() {
        current.push(component.as_os_str());
        match current.symlink_metadata() {
            Ok(metadata) if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 => {
                return Err(effect_failure(
                    PortId::RunStore,
                    operation,
                    FailureKind::Integrity,
                    Some(&current),
                    "effect path contains a Windows reparse point",
                ));
            }
            Ok(_) => {}
            Err(error)
                if allow_missing_leaf
                    && index + 1 == component_count
                    && error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_failure(PortId::RunStore, operation, &current, error)),
        }
    }
    Ok(())
}

/// Validates an asset target whose ordinary parent exists but leaf may not.
fn validate_effect_target(path: &Path, operation: OperationId) -> Result<(), PortFailure> {
    let parent = path.parent().ok_or_else(|| {
        effect_failure(
            PortId::RunStore,
            operation,
            FailureKind::InvalidInput,
            Some(path),
            "asset target has no parent directory",
        )
    })?;
    validate_effect_path(parent, false, operation)?;
    validate_effect_path(path, true, operation)
}

/// Maps one Windows filesystem error into the application-owned run vocabulary.
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

/// Creates one stable RunStore resource-exhaustion failure.
fn resource_failure(operation: OperationId, path: &Path, diagnostic: &str) -> PortFailure {
    effect_failure(
        PortId::RunStore,
        operation,
        FailureKind::ResourceExhausted,
        Some(path),
        diagnostic,
    )
}

/// Creates a stable bounded failure with an optional affected path.
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

/// Returns the stable built-in profile spelling used in log names and headers.
const fn profile_slug(profile: ActiveProfileId) -> &'static str {
    match profile {
        ActiveProfileId::Fo4 => "FO4",
        ActiveProfileId::Sse => "SSE",
        ActiveProfileId::Tes5 => "TES5",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FILE_SHARE_READ, MAX_RETAINED_RUN_LOG_BYTES, MAX_RETAINED_RUN_LOGS,
        WindowsRunEnvironmentFactory,
    };
    use cao_application::{
        ActiveProfileId, FailureKind, RunEnvironmentFactory, RunEnvironmentRequest, RunStoreLimits,
        WriteAuditAction,
    };
    use std::fs::{self, File, OpenOptions};
    use std::os::windows::fs::OpenOptionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct RunSandbox {
        root: PathBuf,
        executable: PathBuf,
    }

    impl RunSandbox {
        /// Copies the current test binary into a unique executable-relative fixture tree.
        fn create(label: &str) -> Self {
            static NEXT_ID: AtomicU64 = AtomicU64::new(1);
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "tracetide-run-effects-{label}-{}-{id}",
                std::process::id()
            ));
            fs::create_dir(&root).expect("run-effects fixture root should be created");
            let executable = root.join("tracetide.exe");
            fs::copy(
                std::env::current_exe().expect("test executable should resolve"),
                &executable,
            )
            .expect("test executable should be copied into the fixture");
            Self { root, executable }
        }

        fn root(&self) -> &Path {
            &self.root
        }

        fn executable(&self) -> &Path {
            &self.executable
        }
    }

    impl Drop for RunSandbox {
        /// Best-effort removes the test-owned executable, logs, assets, and scratch tree.
        fn drop(&mut self) {
            // Test cleanup is best-effort because a failed assertion may leave a file open.
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn factory_creates_unique_scratch_and_utf8_logs_and_finalizes_idempotently() {
        let sandbox = RunSandbox::create("unique");
        let factory = WindowsRunEnvironmentFactory::for_executable(sandbox.executable())
            .expect("run factory should resolve the executable root");
        let request = RunEnvironmentRequest::new(ActiveProfileId::Sse, false);
        let mut first = factory
            .create(request)
            .expect("first run environment should be created");
        let second = factory
            .create(request)
            .expect("second run environment should be created");

        assert_ne!(first.run_id(), second.run_id());
        assert_ne!(first.scratch_directory(), second.scratch_directory());
        assert_ne!(first.log_path(), second.log_path());
        assert!(first.scratch_directory().starts_with(std::env::temp_dir()));
        assert!(
            first
                .log_path()
                .starts_with(sandbox.root().join("data/logs"))
        );
        assert!(first.log_path().to_string_lossy().contains("SSE"));

        let log_path = first.log_path().to_path_buf();
        let scratch = first.scratch_directory().to_path_buf();
        first
            .log()
            .write("profile=SSE message=caf\u{e9}")
            .expect("UTF-8 log record should be accepted");
        assert!(
            first
                .finalize()
                .expect("first finalize should succeed")
                .is_empty()
        );
        assert!(
            first
                .finalize()
                .expect("second finalize should succeed")
                .is_empty()
        );
        assert!(!scratch.exists());
        assert!(
            fs::read_to_string(log_path)
                .expect("run log should be valid UTF-8")
                .contains("caf\u{e9}")
        );
    }

    #[test]
    fn run_store_inventories_stages_verifies_and_applies_ordered_atomic_writes() {
        let sandbox = RunSandbox::create("real-store");
        let assets = sandbox.root().join("assets");
        fs::create_dir(&assets).expect("asset root should be created");
        let existing = assets.join("existing.bin");
        fs::write(&existing, b"before").expect("existing asset should be written");
        let factory = WindowsRunEnvironmentFactory::for_executable(sandbox.executable())
            .expect("run factory should resolve the executable root");
        let mut environment = factory
            .create(RunEnvironmentRequest::new(ActiveProfileId::Fo4, false))
            .expect("real run environment should be created");
        let store = environment.store();

        let inventory = store.inventory(&assets).expect("inventory should succeed");
        assert_eq!(inventory.len(), 1);
        assert_eq!(inventory[0].path(), existing);
        assert_eq!(
            store.read(&existing).expect("read should succeed"),
            b"before"
        );

        let replacement = store.stage(b"after").expect("replacement should stage");
        store
            .verify_staged(replacement, b"after")
            .expect("exact staged verification should pass");
        store
            .commit_replace(replacement, &existing)
            .expect("replacement should commit atomically");
        let created = assets.join("created.bin");
        let creation = store.stage(b"new").expect("creation should stage");
        store
            .commit_replace(creation, &created)
            .expect("creation should commit atomically");
        store
            .delete(&existing)
            .expect("existing target should delete");

        assert!(!existing.exists());
        assert_eq!(
            fs::read(&created).expect("created asset should remain"),
            b"new"
        );
        assert_eq!(
            store
                .audit()
                .iter()
                .map(|entry| entry.action())
                .collect::<Vec<_>>(),
            vec![
                WriteAuditAction::Replace,
                WriteAuditAction::Create,
                WriteAuditAction::Delete,
            ]
        );
        assert!(store.residue().expect("residue should report").is_empty());
    }

    #[test]
    fn dry_run_audits_without_mutating_assets_and_consumes_private_staging() {
        let sandbox = RunSandbox::create("dry-store");
        let assets = sandbox.root().join("assets");
        fs::create_dir(&assets).expect("asset root should be created");
        let existing = assets.join("existing.bin");
        fs::write(&existing, b"before").expect("existing asset should be written");
        let factory = WindowsRunEnvironmentFactory::for_executable(sandbox.executable())
            .expect("run factory should resolve the executable root");
        let mut environment = factory
            .create(RunEnvironmentRequest::new(ActiveProfileId::Tes5, true))
            .expect("dry-run environment should be created");
        let store = environment.store();

        let replacement = store.stage(b"after").expect("dry replacement should stage");
        store
            .commit_replace(replacement, &existing)
            .expect("dry replacement should audit");
        store.delete(&existing).expect("dry delete should audit");

        assert_eq!(fs::read(&existing).expect("asset should remain"), b"before");
        assert_eq!(store.audit().len(), 2);
        assert!(store.residue().expect("residue should report").is_empty());
    }

    #[test]
    fn store_enforces_read_inventory_staging_and_operation_bounds() {
        let sandbox = RunSandbox::create("bounds");
        let assets = sandbox.root().join("assets");
        fs::create_dir(&assets).expect("asset root should be created");
        fs::write(assets.join("a.bin"), b"four").expect("first asset should be written");
        fs::write(assets.join("b.bin"), b"five").expect("second asset should be written");
        let limits = RunStoreLimits {
            max_inventory_entries: 1,
            max_read_bytes: 3,
            max_operations: 1,
            max_staged_bytes: 3,
        };
        let factory = WindowsRunEnvironmentFactory::for_executable(sandbox.executable())
            .expect("run factory should resolve the executable root")
            .with_store_limits(limits);
        let mut environment = factory
            .create(RunEnvironmentRequest::new(ActiveProfileId::Sse, false))
            .expect("bounded environment should be created");
        let store = environment.store();

        assert_eq!(
            store
                .inventory(&assets)
                .expect_err("inventory must be bounded")
                .kind(),
            FailureKind::ResourceExhausted
        );
        assert_eq!(
            store
                .read(&assets.join("a.bin"))
                .expect_err("read must be bounded")
                .kind(),
            FailureKind::ResourceExhausted
        );
        assert_eq!(
            store
                .stage(b"four")
                .expect_err("staging bytes must be bounded")
                .kind(),
            FailureKind::ResourceExhausted
        );
        let first = store.stage(b"one").expect("first operation should fit");
        assert_eq!(
            store
                .stage(b"two")
                .expect_err("operation count must be bounded")
                .kind(),
            FailureKind::ResourceExhausted
        );
        store
            .commit_replace(first, &assets.join("created.bin"))
            .expect("first audit operation should fit");
        let second = store
            .stage(b"two")
            .expect("consumed staging frees one slot");
        assert_eq!(
            store
                .commit_replace(second, &assets.join("other.bin"))
                .expect_err("audit operation count must be bounded")
                .kind(),
            FailureKind::ResourceExhausted
        );
    }

    #[test]
    fn store_rejects_relative_traversal_and_reparse_targets() {
        let sandbox = RunSandbox::create("containment");
        let assets = sandbox.root().join("assets");
        fs::create_dir(&assets).expect("asset root should be created");
        let target = assets.join("target.bin");
        fs::write(&target, b"target").expect("target should be written");
        let redirected = assets.join("redirected.bin");
        std::os::windows::fs::symlink_file(&target, &redirected)
            .expect("reparse fixture should be created");
        let factory = WindowsRunEnvironmentFactory::for_executable(sandbox.executable())
            .expect("run factory should resolve the executable root");
        let mut environment = factory
            .create(RunEnvironmentRequest::new(ActiveProfileId::Sse, false))
            .expect("contained environment should be created");
        let store = environment.store();

        assert_eq!(
            store
                .read(Path::new("relative.bin"))
                .expect_err("relative path must fail")
                .kind(),
            FailureKind::InvalidInput
        );
        assert_eq!(
            store
                .read(&assets.join("nested").join("..").join("target.bin"))
                .expect_err("parent traversal must fail")
                .kind(),
            FailureKind::InvalidInput
        );
        assert_eq!(
            store
                .read(&redirected)
                .expect_err("reparse target must fail")
                .kind(),
            FailureKind::Integrity
        );
    }

    #[test]
    fn run_log_retention_enforces_count_and_aggregate_byte_bounds() {
        let sandbox = RunSandbox::create("log-retention");
        let logs = sandbox.root().join("data/logs");
        fs::create_dir_all(&logs).expect("retention log directory should be created");
        for index in 0..3 {
            let file = File::create(logs.join(format!("000{index}-old.log")))
                .expect("old retained log should be created");
            file.set_len(40 * 1024 * 1024)
                .expect("old retained log should have a sparse bounded length");
        }
        let factory = WindowsRunEnvironmentFactory::for_executable(sandbox.executable())
            .expect("run factory should resolve the executable root");
        let request = RunEnvironmentRequest::new(ActiveProfileId::Sse, false);
        for _ in 0..101 {
            let mut environment = factory
                .create(request)
                .expect("retained run environment should be created");
            environment
                .finalize()
                .expect("retained run environment should finalize");
        }

        let retained: Vec<_> = fs::read_dir(&logs)
            .expect("retained logs should be enumerable")
            .map(|entry| entry.expect("retained log entry should resolve").path())
            .collect();
        let retained_bytes: u64 = retained
            .iter()
            .map(|path| {
                fs::metadata(path)
                    .expect("retained log metadata should resolve")
                    .len()
            })
            .sum();
        assert!(retained.len() <= MAX_RETAINED_RUN_LOGS);
        assert!(retained_bytes <= MAX_RETAINED_RUN_LOG_BYTES);
    }

    #[test]
    fn failed_atomic_replace_preserves_original_and_reports_pending_residue() {
        let sandbox = RunSandbox::create("replace-failure");
        let assets = sandbox.root().join("assets");
        fs::create_dir(&assets).expect("asset root should be created");
        let target = assets.join("locked.bin");
        fs::write(&target, b"original").expect("original target should be written");
        let blocker = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&target)
            .expect("target should be opened without delete sharing");
        let factory = WindowsRunEnvironmentFactory::for_executable(sandbox.executable())
            .expect("run factory should resolve the executable root");
        let mut environment = factory
            .create(RunEnvironmentRequest::new(ActiveProfileId::Sse, false))
            .expect("run environment should be created");
        let store = environment.store();
        let artifact = store
            .stage(b"replacement")
            .expect("replacement should stage");

        let failure = store
            .commit_replace(artifact, &target)
            .expect_err("locked authority must reject atomic replacement");

        assert_eq!(failure.kind(), FailureKind::PermissionDenied);
        assert_eq!(
            fs::read(&target).expect("original should remain"),
            b"original"
        );
        assert!(
            store
                .residue()
                .expect("residue should report")
                .entries()
                .iter()
                .any(|entry| entry.path().parent() == Some(assets.as_path()))
        );
        drop(blocker);
        assert!(
            store
                .cleanup()
                .expect("cleanup should retry residue")
                .is_empty()
        );
    }
}
