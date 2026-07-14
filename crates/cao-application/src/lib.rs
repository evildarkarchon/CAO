#![forbid(unsafe_code)]
//! Application orchestration and inward-facing ports for Tracetide.

mod run_effects;

pub use cao_domain::{
    ActiveProfileId, AnimationChoices, ArchiveChoices, ArchiveFormatTarget, AssetPath,
    AssetPathError, AuthenticatedProfileAsset, BuiltInProfileDefinition, EffectiveProfileOverlay,
    MeshChoices, MeshFormatTarget, ProcessingMode, ProfileCapabilities, ProfileOverlay,
    ProfileSelectionFallback, SetupState, TextureChoices, TextureFormatTarget,
    authenticated_built_in_profile_contract,
};
use crossbeam_channel::{Receiver, Sender, TrySendError, bounded};
pub use run_effects::{
    AtomicFilePublisher, CancellationProbe, Clock, DEFAULT_PROCESS_OUTPUT_BYTES, IdSource,
    InventoryEntry, MAX_RETAINED_RUN_LOG_BYTES, MAX_RETAINED_RUN_LOGS, MAX_RUN_LOG_BYTES,
    OneShotProcess, ProcessFacts, ProcessOutput, ProcessRequest, ProcessTermination, Residue,
    ResidueReport, RunEnvironment, RunEnvironmentFactory, RunEnvironmentRequest, RunId, RunLog,
    RunStore, RunStoreLimits, StagedArtifact, TimestampMillis, WriteAuditAction, WriteAuditEntry,
};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::thread::{self, JoinHandle};

/// Maximum UTF-8 byte length retained for one public diagnostic or subject.
pub const MAX_DIAGNOSTIC_BYTES: usize = 4 * 1024;

const INTENT_QUEUE_CAPACITY: usize = 64;

/// Monotonic identifier for one authoritative workbench snapshot.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SnapshotRevision(u64);

impl SnapshotRevision {
    /// Revision assigned to setup loaded when an application runtime starts.
    pub const INITIAL: Self = Self(0);

    /// Returns the stable numeric representation of this revision.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Returns the next revision, or `None` when monotonic publication is exhausted.
    const fn next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

/// One typed mutation of the active profile overlay.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileOverlayEdit {
    /// Enables or disables faithful non-mutating processing.
    SetDryRun(bool),
    /// Stores animation optimization even when the active definition masks the control.
    SetAnimationOptimization(bool),
}

/// Explicit user choice for recovering corrupt fork-owned global configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GlobalStateRecoveryAction {
    /// Restores the newest valid restorable backup discovered by the state adapter.
    RestoreBackup,
    /// Replaces corrupt global configuration with documented fork defaults.
    Reset,
}

/// Closed set of commands accepted by the UI-independent application seam.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Intent {
    /// Applies one setup edit only if the caller observed the current revision.
    EditProfileOverlay {
        /// Snapshot revision against which the edit was made.
        expected_revision: SnapshotRevision,
        /// Typed profile-overlay mutation to persist.
        edit: ProfileOverlayEdit,
    },
    /// Selects one built-in by stable identity without deriving it from a name or path.
    SelectProfile {
        /// Snapshot revision against which the selection was made.
        expected_revision: SnapshotRevision,
        /// Reserved built-in identity to make active.
        profile_id: ActiveProfileId,
    },
    /// Removes only the active profile's overlay and restores documented defaults.
    ResetProfileOverlay {
        /// Snapshot revision against which the reset was requested.
        expected_revision: SnapshotRevision,
    },
    /// Applies an explicit recovery choice while corrupt global state blocks setup edits.
    RecoverGlobalState {
        /// Snapshot revision against which the recovery choice was made.
        expected_revision: SnapshotRevision,
        /// Restore or reset transaction selected by the user.
        action: GlobalStateRecoveryAction,
    },
}

impl Intent {
    /// Returns the authoritative snapshot revision observed when this intent was created.
    const fn expected_revision(&self) -> SnapshotRevision {
        match self {
            Self::EditProfileOverlay {
                expected_revision, ..
            }
            | Self::SelectProfile {
                expected_revision, ..
            }
            | Self::ResetProfileOverlay { expected_revision }
            | Self::RecoverGlobalState {
                expected_revision, ..
            } => *expected_revision,
        }
    }
}

/// Stable identifier assigned to one accepted intent.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct IntentId(u64);

impl IntentId {
    /// Returns the stable numeric representation of this identifier.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Transport receipt proving that an intent entered the bounded application queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IntentReceipt {
    intent_id: IntentId,
}

impl IntentReceipt {
    /// Returns the identifier used to correlate this receipt with a snapshot outcome.
    #[must_use]
    pub const fn intent_id(self) -> IntentId {
        self.intent_id
    }
}

/// Stable reasons an intent cannot be accepted or applied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntentRejection {
    /// The bounded intent queue has no remaining capacity.
    Busy,
    /// The edit was based on a snapshot older than the authoritative state.
    StaleRevision,
    /// The application lifecycle does not permit the requested action.
    InvalidLifecycle,
    /// Current setup validation prevents the requested action.
    ValidationBlocked,
    /// The requested application-owned entity does not exist.
    NotFound,
}

/// Application capability that reported a stable port failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortId {
    /// Portable configuration and profile state.
    PortableState,
    /// Fresh filesystem, logging, process, clock, and identity capabilities for one run.
    RunEnvironment,
    /// Bounded staged filesystem effects for one run.
    RunStore,
    /// Durable UTF-8 diagnostics for one run.
    RunLog,
    /// Contained one-shot helper execution.
    Process,
    /// Wall-clock observations owned by a run.
    Clock,
    /// Unique identity allocation owned by a run.
    Identity,
    /// Long-lived application-supervisor runtime.
    ApplicationRuntime,
}

/// Application operation that failed at a port boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationId {
    /// Open a portable-state session.
    Open,
    /// Load persisted setup values.
    LoadSetup,
    /// Atomically persist setup values.
    PersistSetup,
    /// Apply ordered forward migrations to fork-owned portable state.
    MigrateState,
    /// Restore corrupt global configuration from a valid backup.
    RestoreGlobalState,
    /// Reset corrupt global configuration to documented defaults.
    ResetGlobalState,
    /// Create all fresh capabilities for one processing run.
    CreateRunEnvironment,
    /// Enumerate a bounded filesystem inventory.
    Inventory,
    /// Read a bounded asset or staged artifact.
    Read,
    /// Write a private staged artifact.
    StageWrite,
    /// Verify a private staged artifact before commit.
    VerifyStaged,
    /// Atomically create or replace an asset from a staged artifact.
    CommitReplace,
    /// Atomically remove an asset through a private tombstone.
    Delete,
    /// Report predicted dry-run writes.
    Audit,
    /// Report remaining private artifacts.
    ReportResidue,
    /// Remove disposable run artifacts.
    Cleanup,
    /// Append one UTF-8 run-log record.
    WriteLog,
    /// Flush a run log to durable storage.
    FlushLog,
    /// Execute one contained helper process.
    ExecuteProcess,
    /// Read the application clock.
    ReadClock,
    /// Allocate a unique application identity.
    GenerateId,
    /// Start the application-supervisor thread.
    StartSupervisor,
    /// Stop and join the application-supervisor thread.
    StopSupervisor,
}

/// Stable category for failures returned by application-owned ports.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureKind {
    /// The caller supplied invalid input.
    InvalidInput,
    /// The requested behavior is intentionally unsupported.
    Unsupported,
    /// A required capability is unavailable.
    Unavailable,
    /// A required subject was not found.
    NotFound,
    /// Access was denied.
    PermissionDenied,
    /// Existing state conflicts with the requested operation.
    Conflict,
    /// A bounded resource was exhausted.
    ResourceExhausted,
    /// Persisted or produced data is corrupt.
    CorruptData,
    /// A generic I/O operation failed.
    Io,
    /// A process or helper protocol was violated.
    Protocol,
    /// A format backend rejected valid application input.
    BackendRejected,
    /// A format backend terminated unexpectedly.
    BackendCrashed,
    /// Output integrity cannot be established.
    Integrity,
    /// An application invariant failed.
    Internal,
}

/// Owned UTF-8 text bounded for safe publication in application snapshots.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundedText {
    text: Box<str>,
    truncated: bool,
}

impl BoundedText {
    /// Retains at most [`MAX_DIAGNOSTIC_BYTES`] without splitting a UTF-8 scalar.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        if text.len() <= MAX_DIAGNOSTIC_BYTES {
            return Self {
                text: text.into_boxed_str(),
                truncated: false,
            };
        }

        let mut retained_bytes = MAX_DIAGNOSTIC_BYTES;
        while !text.is_char_boundary(retained_bytes) {
            retained_bytes -= 1;
        }
        let retained = text[..retained_bytes].to_owned().into_boxed_str();
        Self {
            text: retained,
            truncated: true,
        }
    }

    /// Returns the retained valid UTF-8 text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Reports whether bytes were omitted to enforce the projection bound.
    #[must_use]
    pub const fn was_truncated(&self) -> bool {
        self.truncated
    }
}

/// Backend-neutral failure returned across an application-owned port.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PortFailure {
    port: PortId,
    operation: OperationId,
    kind: FailureKind,
    subject: Option<BoundedText>,
    diagnostic: BoundedText,
}

impl PortFailure {
    /// Creates a stable failure without exposing an adapter's source error.
    #[must_use]
    pub fn new(
        port: PortId,
        operation: OperationId,
        kind: FailureKind,
        diagnostic: impl Into<String>,
    ) -> Self {
        Self {
            port,
            operation,
            kind,
            subject: None,
            diagnostic: BoundedText::new(diagnostic),
        }
    }

    /// Returns this failure with an optional affected subject.
    #[must_use]
    pub fn with_subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(BoundedText::new(subject));
        self
    }

    /// Returns the capability that reported the failure.
    #[must_use]
    pub const fn port(&self) -> PortId {
        self.port
    }

    /// Returns the stable operation identifier.
    #[must_use]
    pub const fn operation(&self) -> OperationId {
        self.operation
    }

    /// Returns the stable failure category.
    #[must_use]
    pub const fn kind(&self) -> FailureKind {
        self.kind
    }

    /// Returns the affected subject when one was supplied by the adapter.
    #[must_use]
    pub fn subject(&self) -> Option<&BoundedText> {
        self.subject.as_ref()
    }

    /// Returns the bounded diagnostic intended for logs and projections.
    #[must_use]
    pub const fn diagnostic(&self) -> &BoundedText {
        &self.diagnostic
    }
}

/// Recoverable corrupt-global-state condition projected through the public seam.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GlobalStateRecovery {
    corrupt_failure: PortFailure,
    backup_available: bool,
}

impl GlobalStateRecovery {
    /// Creates a recovery requirement from the load failure and backup availability.
    #[must_use]
    pub const fn new(corrupt_failure: PortFailure, backup_available: bool) -> Self {
        Self {
            corrupt_failure,
            backup_available,
        }
    }

    /// Returns the stable corruption failure that prevented normal setup loading.
    #[must_use]
    pub const fn corrupt_failure(&self) -> &PortFailure {
        &self.corrupt_failure
    }

    /// Reports whether the state adapter found a valid restorable backup.
    #[must_use]
    pub const fn backup_available(&self) -> bool {
        self.backup_available
    }
}

/// Result of loading setup from one exclusively owned portable state tree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SetupLoadOutcome {
    /// Valid setup is ready for ordinary editing and processing.
    Ready(SetupState),
    /// Corrupt global state requires an explicit restore or reset choice.
    RecoveryRequired(GlobalStateRecovery),
}

/// Supervisor-owned session for loading and atomically persisting setup authorities.
pub trait PortableState: Send + 'static {
    /// Loads valid setup or a recoverable corrupt-global-state condition at startup.
    ///
    /// # Errors
    ///
    /// Returns a stable [`PortFailure`] when startup cannot retain a usable state session.
    fn load_setup(&mut self) -> Result<SetupLoadOutcome, PortFailure>;

    /// Persists one profile overlay before it becomes authoritative in memory.
    ///
    /// # Errors
    ///
    /// Returns a stable [`PortFailure`] when the overlay cannot be committed.
    fn persist_profile_overlay(
        &mut self,
        profile: ActiveProfileId,
        overlay: &ProfileOverlay,
    ) -> Result<(), PortFailure>;

    /// Persists one stable active-profile identity before it becomes authoritative in memory.
    ///
    /// # Errors
    ///
    /// Returns a stable [`PortFailure`] when the selection cannot be committed.
    fn persist_active_profile(&mut self, profile: ActiveProfileId) -> Result<(), PortFailure>;

    /// Applies one explicit corrupt-global-state recovery transaction.
    ///
    /// The returned setup is the newly authoritative state after the transaction commits.
    ///
    /// # Errors
    ///
    /// Returns a stable [`PortFailure`] when restore or reset cannot be committed. The
    /// original corrupt global state must remain recoverable after failure.
    fn recover_global_state(
        &mut self,
        action: GlobalStateRecoveryAction,
    ) -> Result<SetupState, PortFailure>;
}

/// Thread-safe factory for a supervisor-owned portable-state session.
pub trait PortableStateFactory: Send + Sync + 'static {
    /// Opens one exclusive portable-state session for an application runtime.
    ///
    /// # Errors
    ///
    /// Returns a stable [`PortFailure`] when the session cannot be opened.
    fn open(&self) -> Result<Box<dyn PortableState>, PortFailure>;
}

/// Receives immutable authoritative state from the application supervisor.
pub trait SnapshotSink: Send + Sync + 'static {
    /// Publishes one owned snapshot without granting mutation access to the sink.
    fn publish(&self, snapshot: Arc<WorkbenchSnapshot>);
}

/// Authoritative result of a previously accepted intent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IntentOutcome {
    /// The edit was durably persisted and applied.
    Applied(IntentReceipt),
    /// The supervisor rejected the edit without changing setup state.
    Rejected {
        /// Receipt returned when the intent entered the queue.
        receipt: IntentReceipt,
        /// Stable semantic rejection reason.
        rejection: IntentRejection,
    },
    /// Persistence failed and setup state remained unchanged.
    Failed {
        /// Receipt returned when the intent entered the queue.
        receipt: IntentReceipt,
        /// Stable application-owned persistence failure.
        failure: PortFailure,
    },
}

/// Bounded, immutable, UI-independent projection of authoritative application state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkbenchSnapshot {
    revision: SnapshotRevision,
    setup: SetupState,
    global_state_recovery: Option<GlobalStateRecovery>,
    last_intent: Option<IntentOutcome>,
}

impl WorkbenchSnapshot {
    /// Returns the monotonic publication revision.
    #[must_use]
    pub const fn revision(&self) -> SnapshotRevision {
        self.revision
    }

    /// Returns the confirmed setup state owned by this snapshot.
    #[must_use]
    pub const fn setup(&self) -> &SetupState {
        &self.setup
    }

    /// Returns the recovery requirement currently blocking ordinary setup edits.
    #[must_use]
    pub const fn global_state_recovery(&self) -> Option<&GlobalStateRecovery> {
        self.global_state_recovery.as_ref()
    }

    /// Returns the most recently processed intent outcome, if any.
    #[must_use]
    pub const fn last_intent(&self) -> Option<&IntentOutcome> {
        self.last_intent.as_ref()
    }

    /// Constructs one internally validated immutable projection.
    fn new(
        revision: SnapshotRevision,
        setup: SetupState,
        global_state_recovery: Option<GlobalStateRecovery>,
        last_intent: Option<IntentOutcome>,
    ) -> Self {
        Self {
            revision,
            setup,
            global_state_recovery,
            last_intent,
        }
    }
}

struct IntentEnvelope {
    receipt: IntentReceipt,
    intent: Intent,
}

enum SupervisorMessage {
    Intent(IntentEnvelope),
    Shutdown,
}

/// Cloneable bounded sender for typed application intents.
#[derive(Clone)]
pub struct ApplicationHandle {
    sender: Sender<SupervisorMessage>,
    next_intent_id: Arc<AtomicU64>,
}

impl ApplicationHandle {
    /// Attempts to enqueue an intent without waiting for application processing.
    ///
    /// A successful receipt confirms transport acceptance only. Observe the matching
    /// [`IntentOutcome`] in a later [`WorkbenchSnapshot`] for authoritative state.
    ///
    /// # Errors
    ///
    /// Returns [`IntentRejection::Busy`] when the capacity-64 queue is full, or
    /// [`IntentRejection::InvalidLifecycle`] after the runtime has stopped.
    pub fn submit(&self, intent: Intent) -> Result<IntentReceipt, IntentRejection> {
        // The channel establishes message ordering; IDs only need atomic uniqueness.
        let intent_id = self
            .next_intent_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map_err(|_| IntentRejection::InvalidLifecycle)?;
        let receipt = IntentReceipt {
            intent_id: IntentId(intent_id),
        };
        let message = SupervisorMessage::Intent(IntentEnvelope { receipt, intent });

        match self.sender.try_send(message) {
            Ok(()) => Ok(receipt),
            Err(TrySendError::Full(_)) => Err(IntentRejection::Busy),
            Err(TrySendError::Disconnected(_)) => Err(IntentRejection::InvalidLifecycle),
        }
    }
}

/// Owner of the application-supervisor thread and its orderly shutdown.
pub struct ApplicationRuntime {
    sender: Option<Sender<SupervisorMessage>>,
    supervisor: Option<JoinHandle<()>>,
}

impl ApplicationRuntime {
    /// Opens portable state, publishes its initial snapshot, and starts the supervisor.
    ///
    /// The factory session and every mutable setup value move to the supervisor thread
    /// before this function returns.
    ///
    /// # Errors
    ///
    /// Returns a stable [`PortFailure`] when state cannot be opened or retained for
    /// recovery, or when the supervisor thread cannot be started.
    pub fn start<F, S>(
        portable_state_factory: Arc<F>,
        snapshot_sink: Arc<S>,
    ) -> Result<(ApplicationHandle, Self), PortFailure>
    where
        F: PortableStateFactory + ?Sized,
        S: SnapshotSink + ?Sized,
    {
        let (sender, receiver) = bounded(INTENT_QUEUE_CAPACITY);
        let (initialization_sender, initialization_receiver) = bounded(1);
        let supervisor = thread::Builder::new()
            .name("tracetide-application-supervisor".to_owned())
            .spawn(move || {
                run_supervisor(
                    portable_state_factory,
                    snapshot_sink,
                    receiver,
                    initialization_sender,
                );
            })
            .map_err(|error| {
                runtime_failure(
                    OperationId::StartSupervisor,
                    FailureKind::ResourceExhausted,
                    error.to_string(),
                )
            })?;

        match initialization_receiver.recv() {
            Ok(Ok(())) => {
                let handle = ApplicationHandle {
                    sender: sender.clone(),
                    next_intent_id: Arc::new(AtomicU64::new(1)),
                };
                Ok((
                    handle,
                    Self {
                        sender: Some(sender),
                        supervisor: Some(supervisor),
                    },
                ))
            }
            Ok(Err(failure)) => {
                // The adapter failure takes precedence; joining only releases thread ownership.
                let _ = supervisor.join();
                Err(failure)
            }
            Err(_) => {
                // Channel loss is the stable failure; joining prevents a detached supervisor.
                let _ = supervisor.join();
                Err(runtime_failure(
                    OperationId::StartSupervisor,
                    FailureKind::Internal,
                    "application supervisor ended during initialization",
                ))
            }
        }
    }

    /// Requests orderly supervisor shutdown and waits until its state session is dropped.
    ///
    /// # Errors
    ///
    /// Returns a stable [`PortFailure`] if the supervisor terminated unexpectedly.
    pub fn shutdown(mut self) -> Result<(), PortFailure> {
        self.shutdown_inner()
    }

    /// Performs idempotent shutdown for both the consuming method and `Drop`.
    fn shutdown_inner(&mut self) -> Result<(), PortFailure> {
        let disconnected = self
            .sender
            .take()
            .is_some_and(|sender| sender.send(SupervisorMessage::Shutdown).is_err());
        let panicked = self
            .supervisor
            .take()
            .is_some_and(|supervisor| supervisor.join().is_err());

        if panicked {
            return Err(runtime_failure(
                OperationId::StopSupervisor,
                FailureKind::Internal,
                "application supervisor panicked during shutdown",
            ));
        }
        if disconnected {
            return Err(runtime_failure(
                OperationId::StopSupervisor,
                FailureKind::Internal,
                "application supervisor stopped before shutdown was requested",
            ));
        }
        Ok(())
    }
}

impl Drop for ApplicationRuntime {
    fn drop(&mut self) {
        // Drop cannot surface a shutdown failure, but it must still release the state session.
        let _ = self.shutdown_inner();
    }
}

/// Opens state on the supervisor thread, reports startup, and owns the intent loop.
fn run_supervisor<F, S>(
    portable_state_factory: Arc<F>,
    snapshot_sink: Arc<S>,
    receiver: Receiver<SupervisorMessage>,
    initialization_sender: Sender<Result<(), PortFailure>>,
) where
    F: PortableStateFactory + ?Sized,
    S: SnapshotSink + ?Sized,
{
    let mut portable_state = match portable_state_factory.open() {
        Ok(portable_state) => portable_state,
        Err(failure) => {
            // The caller may abandon startup; returning also releases factory ownership.
            let _ = initialization_sender.send(Err(failure));
            return;
        }
    };
    let load_outcome = match portable_state.load_setup() {
        Ok(load_outcome) => load_outcome,
        Err(failure) => {
            // Returning after notification drops the failed portable-state session here.
            let _ = initialization_sender.send(Err(failure));
            return;
        }
    };
    let (mut setup, mut global_state_recovery) = match load_outcome {
        SetupLoadOutcome::Ready(setup) => (setup, None),
        SetupLoadOutcome::RecoveryRequired(recovery) => {
            // Defaults provide a bounded inert projection; validation blocks their use
            // until restore or reset returns newly authoritative setup.
            (SetupState::default(), Some(recovery))
        }
    };
    let mut revision = SnapshotRevision::INITIAL;
    snapshot_sink.publish(Arc::new(WorkbenchSnapshot::new(
        revision,
        setup.clone(),
        global_state_recovery.clone(),
        None,
    )));
    if initialization_sender.send(Ok(())).is_err() {
        // The startup caller disappeared, so no runtime can own this state session.
        return;
    }

    while let Ok(message) = receiver.recv() {
        match message {
            SupervisorMessage::Intent(envelope) => {
                let Some(snapshot) = process_intent(
                    portable_state.as_mut(),
                    &mut setup,
                    &mut global_state_recovery,
                    &mut revision,
                    envelope,
                ) else {
                    // A u64 revision cannot advance, so stopping preserves monotonic snapshots.
                    return;
                };
                snapshot_sink.publish(Arc::new(snapshot));
            }
            SupervisorMessage::Shutdown => return,
        }
    }
}

/// Applies one accepted intent transactionally and builds its authoritative projection.
///
/// Setup and recovery state change only after their corresponding portable-state transaction
/// commits. Returns `None` without further mutation when the snapshot revision is exhausted.
fn process_intent(
    portable_state: &mut dyn PortableState,
    setup: &mut SetupState,
    global_state_recovery: &mut Option<GlobalStateRecovery>,
    revision: &mut SnapshotRevision,
    envelope: IntentEnvelope,
) -> Option<WorkbenchSnapshot> {
    let expected_revision = envelope.intent.expected_revision();
    let outcome = if expected_revision != *revision {
        IntentOutcome::Rejected {
            receipt: envelope.receipt,
            rejection: IntentRejection::StaleRevision,
        }
    } else {
        match envelope.intent {
            Intent::EditProfileOverlay { .. }
            | Intent::SelectProfile { .. }
            | Intent::ResetProfileOverlay { .. }
                if global_state_recovery.is_some() =>
            {
                IntentOutcome::Rejected {
                    receipt: envelope.receipt,
                    rejection: IntentRejection::ValidationBlocked,
                }
            }
            Intent::EditProfileOverlay { edit, .. } => {
                let profile_overlay = match edit {
                    ProfileOverlayEdit::SetDryRun(dry_run) => {
                        setup.profile_overlay().with_dry_run(dry_run)
                    }
                    ProfileOverlayEdit::SetAnimationOptimization(optimize) => {
                        setup.profile_overlay().with_animations(
                            setup.profile_overlay().animations().with_optimize(optimize),
                        )
                    }
                };
                let candidate = setup.with_profile_overlay(profile_overlay);
                match portable_state
                    .persist_profile_overlay(setup.active_profile(), candidate.profile_overlay())
                {
                    Ok(()) => {
                        *setup = candidate;
                        IntentOutcome::Applied(envelope.receipt)
                    }
                    Err(failure) => IntentOutcome::Failed {
                        receipt: envelope.receipt,
                        failure,
                    },
                }
            }
            Intent::SelectProfile { profile_id, .. } => {
                let candidate = setup.with_active_profile(profile_id);
                match portable_state.persist_active_profile(profile_id) {
                    Ok(()) => {
                        *setup = candidate;
                        IntentOutcome::Applied(envelope.receipt)
                    }
                    Err(failure) => IntentOutcome::Failed {
                        receipt: envelope.receipt,
                        failure,
                    },
                }
            }
            Intent::ResetProfileOverlay { .. } => {
                let candidate = setup.reset_profile_overlay(setup.active_profile());
                match portable_state
                    .persist_profile_overlay(setup.active_profile(), candidate.profile_overlay())
                {
                    Ok(()) => {
                        *setup = candidate;
                        IntentOutcome::Applied(envelope.receipt)
                    }
                    Err(failure) => IntentOutcome::Failed {
                        receipt: envelope.receipt,
                        failure,
                    },
                }
            }
            Intent::RecoverGlobalState { .. } if global_state_recovery.is_none() => {
                IntentOutcome::Rejected {
                    receipt: envelope.receipt,
                    rejection: IntentRejection::ValidationBlocked,
                }
            }
            Intent::RecoverGlobalState {
                action: GlobalStateRecoveryAction::RestoreBackup,
                ..
            } if global_state_recovery
                .as_ref()
                .is_some_and(|recovery| !recovery.backup_available()) =>
            {
                IntentOutcome::Rejected {
                    receipt: envelope.receipt,
                    rejection: IntentRejection::ValidationBlocked,
                }
            }
            Intent::RecoverGlobalState { action, .. } => {
                match portable_state.recover_global_state(action) {
                    Ok(recovered_setup) => {
                        *setup = recovered_setup;
                        *global_state_recovery = None;
                        IntentOutcome::Applied(envelope.receipt)
                    }
                    Err(failure) => IntentOutcome::Failed {
                        receipt: envelope.receipt,
                        failure,
                    },
                }
            }
        }
    };

    *revision = revision.next()?;
    Some(WorkbenchSnapshot::new(
        *revision,
        setup.clone(),
        global_state_recovery.clone(),
        Some(outcome),
    ))
}

/// Creates a supervisor-owned failure without leaking a thread or I/O source error.
fn runtime_failure(
    operation: OperationId,
    kind: FailureKind,
    diagnostic: impl Into<String>,
) -> PortFailure {
    PortFailure::new(PortId::ApplicationRuntime, operation, kind, diagnostic)
}

#[cfg(test)]
mod tests {
    use super::{
        ActiveProfileId, ApplicationRuntime, FailureKind, GlobalStateRecovery,
        GlobalStateRecoveryAction, Intent, IntentOutcome, IntentRejection, OperationId,
        PortFailure, PortId, PortableState, PortableStateFactory, ProfileOverlay,
        ProfileOverlayEdit, SetupLoadOutcome, SetupState, SnapshotSink, WorkbenchSnapshot,
    };
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};

    const WAIT_TIMEOUT: Duration = Duration::from_secs(5);

    #[derive(Default)]
    struct RecordingSink {
        snapshots: Mutex<Vec<Arc<WorkbenchSnapshot>>>,
        changed: Condvar,
    }

    impl RecordingSink {
        /// Waits for one public snapshot, failing instead of hanging on supervisor loss.
        fn wait_for(&self, index: usize) -> Arc<WorkbenchSnapshot> {
            let deadline = Instant::now() + WAIT_TIMEOUT;
            let mut snapshots = self.snapshots.lock().expect("snapshot lock was poisoned");
            while snapshots.len() <= index {
                let remaining = deadline
                    .checked_duration_since(Instant::now())
                    .expect("snapshot was not published before the timeout");
                (snapshots, _) = self
                    .changed
                    .wait_timeout(snapshots, remaining)
                    .expect("snapshot lock was poisoned while waiting");
            }
            Arc::clone(&snapshots[index])
        }
    }

    impl SnapshotSink for RecordingSink {
        fn publish(&self, snapshot: Arc<WorkbenchSnapshot>) {
            self.snapshots
                .lock()
                .expect("snapshot lock was poisoned")
                .push(snapshot);
            self.changed.notify_all();
        }
    }

    struct FakeStateFactory {
        shared: Arc<FakeStateShared>,
    }

    struct FakeStateShared {
        load: SetupLoadOutcome,
        recovery_result: Mutex<Result<SetupState, PortFailure>>,
        recovery_actions: Mutex<Vec<GlobalStateRecoveryAction>>,
        persist_count: Mutex<usize>,
    }

    impl FakeStateFactory {
        /// Creates a ready or recovery-required state session with a scripted recovery result.
        fn new(load: SetupLoadOutcome, recovery_result: Result<SetupState, PortFailure>) -> Self {
            Self {
                shared: Arc::new(FakeStateShared {
                    load,
                    recovery_result: Mutex::new(recovery_result),
                    recovery_actions: Mutex::new(Vec::new()),
                    persist_count: Mutex::new(0),
                }),
            }
        }

        /// Returns the recovery actions that crossed the portable-state port.
        fn recovery_actions(&self) -> Vec<GlobalStateRecoveryAction> {
            self.shared
                .recovery_actions
                .lock()
                .expect("recovery action lock was poisoned")
                .clone()
        }

        /// Returns how many setup candidates crossed the persistence port.
        fn persist_count(&self) -> usize {
            *self
                .shared
                .persist_count
                .lock()
                .expect("persist count lock was poisoned")
        }
    }

    impl PortableStateFactory for FakeStateFactory {
        fn open(&self) -> Result<Box<dyn PortableState>, PortFailure> {
            Ok(Box::new(FakeState {
                shared: Arc::clone(&self.shared),
            }))
        }
    }

    struct FakeState {
        shared: Arc<FakeStateShared>,
    }

    impl FakeState {
        fn record_persist(&self) {
            *self
                .shared
                .persist_count
                .lock()
                .expect("persist count lock was poisoned") += 1;
        }
    }

    impl PortableState for FakeState {
        fn load_setup(&mut self) -> Result<SetupLoadOutcome, PortFailure> {
            Ok(self.shared.load.clone())
        }

        fn persist_profile_overlay(
            &mut self,
            _profile: ActiveProfileId,
            _overlay: &ProfileOverlay,
        ) -> Result<(), PortFailure> {
            self.record_persist();
            Ok(())
        }

        fn persist_active_profile(&mut self, _profile: ActiveProfileId) -> Result<(), PortFailure> {
            self.record_persist();
            Ok(())
        }

        fn recover_global_state(
            &mut self,
            action: GlobalStateRecoveryAction,
        ) -> Result<SetupState, PortFailure> {
            self.shared
                .recovery_actions
                .lock()
                .expect("recovery action lock was poisoned")
                .push(action);
            self.shared
                .recovery_result
                .lock()
                .expect("recovery result lock was poisoned")
                .clone()
        }
    }

    /// Creates a stable corrupt-global-state projection for recovery tests.
    fn corrupt_recovery(backup_available: bool) -> GlobalStateRecovery {
        GlobalStateRecovery::new(
            PortFailure::new(
                PortId::PortableState,
                OperationId::LoadSetup,
                FailureKind::CorruptData,
                "global configuration is corrupt",
            ),
            backup_available,
        )
    }

    #[test]
    fn corrupt_global_state_starts_in_recovery_and_blocks_profile_edits() {
        let recovery = corrupt_recovery(true);
        let factory = Arc::new(FakeStateFactory::new(
            SetupLoadOutcome::RecoveryRequired(recovery.clone()),
            Ok(SetupState::default()),
        ));
        let sink = Arc::new(RecordingSink::default());
        let (handle, runtime) = ApplicationRuntime::start(Arc::clone(&factory), Arc::clone(&sink))
            .expect("recoverable corruption should publish a usable runtime");
        let initial = sink.wait_for(0);

        assert_eq!(initial.global_state_recovery(), Some(&recovery));
        let receipt = handle
            .submit(Intent::EditProfileOverlay {
                expected_revision: initial.revision(),
                edit: ProfileOverlayEdit::SetDryRun(true),
            })
            .expect("the blocked edit should enter the intent queue");
        let blocked = sink.wait_for(1);

        assert_eq!(blocked.global_state_recovery(), Some(&recovery));
        assert_eq!(
            blocked.last_intent(),
            Some(&IntentOutcome::Rejected {
                receipt,
                rejection: IntentRejection::ValidationBlocked,
            })
        );
        assert_eq!(factory.persist_count(), 0);
        runtime
            .shutdown()
            .expect("the recovery runtime should shut down cleanly");
    }

    #[test]
    fn profile_intents_isolate_reset_and_mask_overlays_through_snapshots() {
        let factory = Arc::new(FakeStateFactory::new(
            SetupLoadOutcome::Ready(SetupState::default()),
            Ok(SetupState::default()),
        ));
        let sink = Arc::new(RecordingSink::default());
        let (handle, runtime) = ApplicationRuntime::start(Arc::clone(&factory), Arc::clone(&sink))
            .expect("application runtime should start");
        let initial = sink.wait_for(0);

        handle
            .submit(Intent::EditProfileOverlay {
                expected_revision: initial.revision(),
                edit: ProfileOverlayEdit::SetDryRun(true),
            })
            .expect("the SSE edit should enter the queue");
        let sse_edited = sink.wait_for(1);
        handle
            .submit(Intent::SelectProfile {
                expected_revision: sse_edited.revision(),
                profile_id: ActiveProfileId::Fo4,
            })
            .expect("the FO4 selection should enter the queue");
        let fo4_selected = sink.wait_for(2);

        assert_eq!(fo4_selected.setup().active_profile(), ActiveProfileId::Fo4);
        assert!(!fo4_selected.setup().profile_overlay().dry_run());
        assert!(
            fo4_selected
                .setup()
                .overlay_for(ActiveProfileId::Sse)
                .dry_run()
        );

        handle
            .submit(Intent::EditProfileOverlay {
                expected_revision: fo4_selected.revision(),
                edit: ProfileOverlayEdit::SetAnimationOptimization(true),
            })
            .expect("the dormant FO4 choice should enter the queue");
        let fo4_edited = sink.wait_for(3);
        assert!(fo4_edited.setup().profile_overlay().animations().optimize());
        assert_eq!(
            fo4_edited.setup().effective_profile_overlay().animations(),
            None
        );

        handle
            .submit(Intent::ResetProfileOverlay {
                expected_revision: fo4_edited.revision(),
            })
            .expect("the FO4 reset should enter the queue");
        let fo4_reset = sink.wait_for(4);
        assert_eq!(
            fo4_reset.setup().profile_overlay(),
            &ProfileOverlay::default()
        );

        handle
            .submit(Intent::SelectProfile {
                expected_revision: fo4_reset.revision(),
                profile_id: ActiveProfileId::Sse,
            })
            .expect("the SSE selection should enter the queue");
        let sse_reselected = sink.wait_for(5);
        assert!(sse_reselected.setup().profile_overlay().dry_run());
        assert!(
            sse_reselected
                .setup()
                .effective_profile_overlay()
                .animations()
                .is_some()
        );
        assert_eq!(factory.persist_count(), 5);
        runtime
            .shutdown()
            .expect("the profile runtime should shut down cleanly");
    }

    #[test]
    fn restoring_global_state_clears_recovery_and_publishes_restored_setup() {
        let restored = SetupState::default()
            .with_profile_overlay(ProfileOverlay::default().with_dry_run(true));
        let factory = Arc::new(FakeStateFactory::new(
            SetupLoadOutcome::RecoveryRequired(corrupt_recovery(true)),
            Ok(restored.clone()),
        ));
        let sink = Arc::new(RecordingSink::default());
        let (handle, runtime) = ApplicationRuntime::start(Arc::clone(&factory), Arc::clone(&sink))
            .expect("recoverable corruption should publish a usable runtime");
        let initial = sink.wait_for(0);

        let receipt = handle
            .submit(Intent::RecoverGlobalState {
                expected_revision: initial.revision(),
                action: GlobalStateRecoveryAction::RestoreBackup,
            })
            .expect("restore should enter the intent queue");
        let recovered = sink.wait_for(1);

        assert_eq!(recovered.global_state_recovery(), None);
        assert_eq!(recovered.setup(), &restored);
        assert_eq!(
            recovered.last_intent(),
            Some(&IntentOutcome::Applied(receipt))
        );
        assert_eq!(
            factory.recovery_actions(),
            [GlobalStateRecoveryAction::RestoreBackup]
        );
        runtime
            .shutdown()
            .expect("the restored runtime should shut down cleanly");
    }

    #[test]
    fn failed_global_state_reset_preserves_recovery_requirement() {
        let recovery = corrupt_recovery(false);
        let failure = PortFailure::new(
            PortId::PortableState,
            OperationId::ResetGlobalState,
            FailureKind::Io,
            "injected reset failure",
        );
        let factory = Arc::new(FakeStateFactory::new(
            SetupLoadOutcome::RecoveryRequired(recovery.clone()),
            Err(failure.clone()),
        ));
        let sink = Arc::new(RecordingSink::default());
        let (handle, runtime) = ApplicationRuntime::start(Arc::clone(&factory), Arc::clone(&sink))
            .expect("recoverable corruption should publish a usable runtime");
        let initial = sink.wait_for(0);

        let receipt = handle
            .submit(Intent::RecoverGlobalState {
                expected_revision: initial.revision(),
                action: GlobalStateRecoveryAction::Reset,
            })
            .expect("reset should enter the intent queue");
        let failed = sink.wait_for(1);

        assert_eq!(failed.global_state_recovery(), Some(&recovery));
        assert_eq!(
            failed.last_intent(),
            Some(&IntentOutcome::Failed { receipt, failure })
        );
        assert_eq!(
            factory.recovery_actions(),
            [GlobalStateRecoveryAction::Reset]
        );
        runtime
            .shutdown()
            .expect("the recovery runtime should shut down after a failed reset");
    }

    #[test]
    fn reset_without_a_backup_clears_recovery_after_commit() {
        let reset = SetupState::default()
            .with_profile_overlay(ProfileOverlay::default().with_dry_run(true));
        let factory = Arc::new(FakeStateFactory::new(
            SetupLoadOutcome::RecoveryRequired(corrupt_recovery(false)),
            Ok(reset.clone()),
        ));
        let sink = Arc::new(RecordingSink::default());
        let (handle, runtime) = ApplicationRuntime::start(Arc::clone(&factory), Arc::clone(&sink))
            .expect("resettable corruption should publish a usable runtime");
        let initial = sink.wait_for(0);

        handle
            .submit(Intent::RecoverGlobalState {
                expected_revision: initial.revision(),
                action: GlobalStateRecoveryAction::Reset,
            })
            .expect("reset should enter the intent queue");
        let recovered = sink.wait_for(1);

        assert_eq!(recovered.global_state_recovery(), None);
        assert_eq!(recovered.setup(), &reset);
        assert_eq!(
            factory.recovery_actions(),
            [GlobalStateRecoveryAction::Reset]
        );
        runtime
            .shutdown()
            .expect("the reset runtime should shut down cleanly");
    }

    #[test]
    fn stale_or_unavailable_restore_is_rejected_before_port_dispatch() {
        let factory = Arc::new(FakeStateFactory::new(
            SetupLoadOutcome::RecoveryRequired(corrupt_recovery(false)),
            Ok(SetupState::default()),
        ));
        let sink = Arc::new(RecordingSink::default());
        let (handle, runtime) = ApplicationRuntime::start(Arc::clone(&factory), Arc::clone(&sink))
            .expect("recoverable corruption should publish a usable runtime");
        let initial = sink.wait_for(0);

        let unavailable_receipt = handle
            .submit(Intent::RecoverGlobalState {
                expected_revision: initial.revision(),
                action: GlobalStateRecoveryAction::RestoreBackup,
            })
            .expect("the unavailable restore should enter the intent queue");
        let unavailable = sink.wait_for(1);
        assert_eq!(
            unavailable.last_intent(),
            Some(&IntentOutcome::Rejected {
                receipt: unavailable_receipt,
                rejection: IntentRejection::ValidationBlocked,
            })
        );

        let stale_receipt = handle
            .submit(Intent::RecoverGlobalState {
                expected_revision: initial.revision(),
                action: GlobalStateRecoveryAction::RestoreBackup,
            })
            .expect("the stale restore should enter the intent queue");
        let stale = sink.wait_for(2);
        assert_eq!(
            stale.last_intent(),
            Some(&IntentOutcome::Rejected {
                receipt: stale_receipt,
                rejection: IntentRejection::StaleRevision,
            })
        );
        assert_eq!(
            stale.global_state_recovery(),
            initial.global_state_recovery()
        );
        assert!(factory.recovery_actions().is_empty());
        runtime
            .shutdown()
            .expect("the blocked recovery runtime should shut down cleanly");
    }

    #[test]
    fn recovery_intent_is_blocked_when_global_state_is_ready() {
        let factory = Arc::new(FakeStateFactory::new(
            SetupLoadOutcome::Ready(SetupState::default()),
            Ok(SetupState::default()),
        ));
        let sink = Arc::new(RecordingSink::default());
        let (handle, runtime) = ApplicationRuntime::start(Arc::clone(&factory), Arc::clone(&sink))
            .expect("ready setup should start normally");
        let initial = sink.wait_for(0);

        let receipt = handle
            .submit(Intent::RecoverGlobalState {
                expected_revision: initial.revision(),
                action: GlobalStateRecoveryAction::Reset,
            })
            .expect("the invalid recovery should enter the intent queue");
        let rejected = sink.wait_for(1);

        assert_eq!(
            rejected.last_intent(),
            Some(&IntentOutcome::Rejected {
                receipt,
                rejection: IntentRejection::ValidationBlocked,
            })
        );
        assert!(factory.recovery_actions().is_empty());
        runtime
            .shutdown()
            .expect("the ready runtime should shut down cleanly");
    }
}
