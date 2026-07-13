#![forbid(unsafe_code)]
//! Application orchestration and inward-facing ports for Tracetide.

pub use cao_domain::{ProfileOverlay, SetupState};
use crossbeam_channel::{Receiver, Sender, TrySendError, bounded};
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

/// Supervisor-owned session for loading and atomically persisting setup state.
pub trait PortableState: Send + 'static {
    /// Loads the authoritative setup state at application startup.
    ///
    /// # Errors
    ///
    /// Returns a stable [`PortFailure`] when persisted state cannot be loaded.
    fn load_setup(&mut self) -> Result<SetupState, PortFailure>;

    /// Persists a complete candidate before it becomes authoritative in memory.
    ///
    /// # Errors
    ///
    /// Returns a stable [`PortFailure`] when the candidate cannot be committed.
    fn persist_setup(&mut self, setup: &SetupState) -> Result<(), PortFailure>;
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

    /// Returns the most recently processed intent outcome, if any.
    #[must_use]
    pub const fn last_intent(&self) -> Option<&IntentOutcome> {
        self.last_intent.as_ref()
    }

    /// Constructs one internally validated immutable projection.
    fn new(
        revision: SnapshotRevision,
        setup: SetupState,
        last_intent: Option<IntentOutcome>,
    ) -> Self {
        Self {
            revision,
            setup,
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
    /// Returns a stable [`PortFailure`] when state cannot be opened or loaded, or when
    /// the supervisor thread cannot be started.
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
    let mut setup = match portable_state.load_setup() {
        Ok(setup) => setup,
        Err(failure) => {
            // Returning after notification drops the failed portable-state session here.
            let _ = initialization_sender.send(Err(failure));
            return;
        }
    };
    let mut revision = SnapshotRevision::INITIAL;
    snapshot_sink.publish(Arc::new(WorkbenchSnapshot::new(
        revision,
        setup.clone(),
        None,
    )));
    if initialization_sender.send(Ok(())).is_err() {
        // The startup caller disappeared, so no runtime can own this state session.
        return;
    }

    while let Ok(message) = receiver.recv() {
        match message {
            SupervisorMessage::Intent(envelope) => {
                let Some(snapshot) =
                    process_intent(portable_state.as_mut(), &mut setup, &mut revision, envelope)
                else {
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
fn process_intent(
    portable_state: &mut dyn PortableState,
    setup: &mut SetupState,
    revision: &mut SnapshotRevision,
    envelope: IntentEnvelope,
) -> Option<WorkbenchSnapshot> {
    let outcome = match envelope.intent {
        Intent::EditProfileOverlay {
            expected_revision,
            edit: _,
        } if expected_revision != *revision => IntentOutcome::Rejected {
            receipt: envelope.receipt,
            rejection: IntentRejection::StaleRevision,
        },
        Intent::EditProfileOverlay { edit, .. } => {
            let profile_overlay = match edit {
                ProfileOverlayEdit::SetDryRun(dry_run) => {
                    setup.profile_overlay().with_dry_run(dry_run)
                }
            };
            let candidate = setup.with_profile_overlay(profile_overlay);
            match portable_state.persist_setup(&candidate) {
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
    };

    *revision = revision.next()?;
    Some(WorkbenchSnapshot::new(
        *revision,
        setup.clone(),
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
