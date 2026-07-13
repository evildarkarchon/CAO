#![forbid(unsafe_code)]
//! Shared parity evidence and replay harness types.

use cao_application::{
    FailureKind, OperationId, PortFailure, PortId, PortableState, PortableStateFactory,
    ProfileOverlay, SetupState, SnapshotSink, WorkbenchSnapshot,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const WAIT_TIMEOUT: Duration = Duration::from_secs(5);

/// Application-owned portable-state operation at which a deterministic fault may fire.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FaultPoint {
    /// Opening the supervisor-owned portable-state session.
    Open,
    /// Loading authoritative setup during application startup.
    LoadSetup,
    /// Persisting a complete setup candidate.
    PersistSetup,
}

#[derive(Clone)]
struct PlannedFault {
    point: FaultPoint,
    fixture_path: Option<PathBuf>,
    failure: PortFailure,
}

/// Operation-keyed faults consumed deterministically by verification adapters.
#[derive(Clone, Default)]
pub struct FaultPlan {
    faults: Vec<PlannedFault>,
}

impl FaultPlan {
    /// Creates a plan that fails the first matching operation exactly once.
    #[must_use]
    pub fn fail_once(point: FaultPoint, failure: PortFailure) -> Self {
        Self {
            faults: vec![PlannedFault {
                point,
                fixture_path: None,
                failure,
            }],
        }
    }

    /// Creates a plan that fails one operation on a matching fixture-relative path.
    #[must_use]
    pub fn fail_once_at(
        point: FaultPoint,
        fixture_path: impl Into<PathBuf>,
        failure: PortFailure,
    ) -> Self {
        Self {
            faults: vec![PlannedFault {
                point,
                fixture_path: Some(fixture_path.into()),
                failure,
            }],
        }
    }

    /// Removes the next fault matching both operation and optional fixture path.
    fn take(&mut self, point: FaultPoint, actual_path: Option<&Path>) -> Option<PortFailure> {
        let index = self.faults.iter().position(|fault| {
            fault.point == point
                && fault.fixture_path.as_ref().is_none_or(|fixture_path| {
                    actual_path.is_some_and(|actual_path| actual_path.ends_with(fixture_path))
                })
        })?;
        Some(self.faults.remove(index).failure)
    }
}

#[derive(Default)]
struct PersistenceGateState {
    entered: bool,
    released: bool,
}

/// Reusable deterministic implementation of the application portable-state factory port.
pub struct DeterministicStateFactory {
    shared: Arc<DeterministicStateShared>,
}

struct DeterministicStateShared {
    setup: Mutex<SetupState>,
    state_file: Option<PathBuf>,
    gate: (Mutex<PersistenceGateState>, Condvar),
    faults: Mutex<FaultPlan>,
}

impl DeterministicStateShared {
    /// Consumes one fault that matches the operation and this adapter's fixture path.
    fn take_fault(&self, point: FaultPoint) -> Option<PortFailure> {
        self.faults
            .lock()
            .expect("fault plan lock was poisoned")
            .take(point, self.state_file.as_deref())
    }
}

impl Default for DeterministicStateFactory {
    fn default() -> Self {
        Self::with_faults(SetupState::default(), FaultPlan::default())
    }
}

impl DeterministicStateFactory {
    /// Creates an adapter with authoritative setup and an operation-keyed fault script.
    #[must_use]
    pub fn with_faults(setup: SetupState, faults: FaultPlan) -> Self {
        Self {
            shared: Arc::new(DeterministicStateShared {
                setup: Mutex::new(setup),
                state_file: None,
                gate: (Mutex::new(PersistenceGateState::default()), Condvar::new()),
                faults: Mutex::new(faults),
            }),
        }
    }

    /// Creates a deterministic portable state tree rooted in a fresh verification sandbox.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the sandbox directory or initial state cannot be written.
    pub fn in_sandbox(
        portable_state_tree: &Path,
        setup: SetupState,
        faults: FaultPlan,
    ) -> std::io::Result<Self> {
        std::fs::create_dir_all(portable_state_tree)?;
        let state_file = portable_state_tree.join("setup.state");
        write_setup_file(&state_file, &setup)?;
        Ok(Self {
            shared: Arc::new(DeterministicStateShared {
                setup: Mutex::new(setup),
                state_file: Some(state_file),
                gate: (Mutex::new(PersistenceGateState::default()), Condvar::new()),
                faults: Mutex::new(faults),
            }),
        })
    }

    /// Waits until the supervisor has begun the durable setup write.
    pub fn wait_until_persisting(&self) {
        let (state, changed) = &self.shared.gate;
        let deadline = Instant::now() + WAIT_TIMEOUT;
        let mut state = state.lock().expect("persistence gate lock was poisoned");
        while !state.entered {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .expect("supervisor did not begin persistence before the timeout");
            (state, _) = changed
                .wait_timeout(state, remaining)
                .expect("persistence gate lock was poisoned while waiting");
        }
    }

    /// Releases the deterministic setup write after receipt assertions complete.
    pub fn release_persistence(&self) {
        let (state, changed) = &self.shared.gate;
        let mut state = state.lock().expect("persistence gate lock was poisoned");
        state.released = true;
        changed.notify_all();
    }

    /// Returns the setup currently held by the deterministic persistence adapter.
    #[must_use]
    pub fn persisted_setup(&self) -> SetupState {
        self.shared
            .setup
            .lock()
            .expect("deterministic setup lock was poisoned")
            .clone()
    }
}

impl PortableStateFactory for DeterministicStateFactory {
    fn open(&self) -> Result<Box<dyn PortableState>, PortFailure> {
        if let Some(failure) = self.shared.take_fault(FaultPoint::Open) {
            return Err(failure);
        }
        Ok(Box::new(DeterministicState {
            shared: Arc::clone(&self.shared),
        }))
    }
}

struct DeterministicState {
    shared: Arc<DeterministicStateShared>,
}

impl PortableState for DeterministicState {
    fn load_setup(&mut self) -> Result<SetupState, PortFailure> {
        if let Some(failure) = self.shared.take_fault(FaultPoint::LoadSetup) {
            return Err(failure);
        }
        if let Some(state_file) = &self.shared.state_file {
            let loaded = read_setup_file(state_file)?;
            *self
                .shared
                .setup
                .lock()
                .expect("deterministic setup lock was poisoned") = loaded;
        }
        Ok(self
            .shared
            .setup
            .lock()
            .expect("deterministic setup lock was poisoned")
            .clone())
    }

    fn persist_setup(&mut self, setup: &SetupState) -> Result<(), PortFailure> {
        if let Some(failure) = self.shared.take_fault(FaultPoint::PersistSetup) {
            return Err(failure);
        }

        let (state, changed) = &self.shared.gate;
        let mut state = state.lock().expect("persistence gate lock was poisoned");
        state.entered = true;
        changed.notify_all();

        // The barrier makes transport acceptance observable before durable confirmation.
        while !state.released {
            state = changed
                .wait(state)
                .expect("persistence gate lock was poisoned while blocked");
        }
        drop(state);

        if let Some(state_file) = &self.shared.state_file {
            write_setup_file(state_file, setup).map_err(|error| {
                PortFailure::new(
                    PortId::PortableState,
                    OperationId::PersistSetup,
                    FailureKind::Io,
                    error.to_string(),
                )
            })?;
        }

        *self
            .shared
            .setup
            .lock()
            .expect("deterministic setup lock was poisoned") = setup.clone();
        Ok(())
    }
}

/// Writes the complete deterministic setup representation used inside verification sandboxes.
fn write_setup_file(path: &Path, setup: &SetupState) -> std::io::Result<()> {
    let dry_run = u8::from(setup.profile_overlay().dry_run());
    std::fs::write(path, format!("dry_run={dry_run}\n"))
}

/// Loads the strict deterministic setup representation without accepting implicit defaults.
fn read_setup_file(path: &Path) -> Result<SetupState, PortFailure> {
    let content = std::fs::read_to_string(path).map_err(|error| {
        PortFailure::new(
            PortId::PortableState,
            OperationId::LoadSetup,
            FailureKind::Io,
            error.to_string(),
        )
    })?;
    let dry_run = match content.as_str() {
        "dry_run=0\n" => false,
        "dry_run=1\n" => true,
        _ => {
            return Err(PortFailure::new(
                PortId::PortableState,
                OperationId::LoadSetup,
                FailureKind::CorruptData,
                "deterministic setup state is malformed",
            ));
        }
    };
    Ok(SetupState::default().with_profile_overlay(ProfileOverlay::default().with_dry_run(dry_run)))
}

mod replay;

pub use replay::{ReplayError, ReplayReport, run_setup_replay};

/// Thread-safe recorder for immutable snapshots published through the application seam.
#[derive(Default)]
pub struct RecordingSink {
    snapshots: Mutex<Vec<Arc<WorkbenchSnapshot>>>,
    changed: Condvar,
}

impl RecordingSink {
    /// Waits for a public snapshot at the requested zero-based position.
    #[must_use]
    pub fn wait_for(&self, index: usize) -> Arc<WorkbenchSnapshot> {
        let deadline = Instant::now() + WAIT_TIMEOUT;
        let mut snapshots = self
            .snapshots
            .lock()
            .expect("snapshot recorder lock was poisoned");
        while snapshots.len() <= index {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .expect("snapshot was not published before the timeout");
            (snapshots, _) = self
                .changed
                .wait_timeout(snapshots, remaining)
                .expect("snapshot recorder lock was poisoned while waiting");
        }
        Arc::clone(&snapshots[index])
    }

    /// Returns how many authoritative snapshots have been published.
    #[must_use]
    pub fn len(&self) -> usize {
        self.snapshots
            .lock()
            .expect("snapshot recorder lock was poisoned")
            .len()
    }

    /// Reports whether no authoritative snapshot has been published.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl SnapshotSink for RecordingSink {
    fn publish(&self, snapshot: Arc<WorkbenchSnapshot>) {
        self.snapshots
            .lock()
            .expect("snapshot recorder lock was poisoned")
            .push(snapshot);
        self.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DeterministicStateFactory, FaultPlan, FaultPoint, RecordingSink, run_setup_replay,
    };
    use cao_application::{
        ApplicationRuntime, BoundedText, FailureKind, Intent, IntentOutcome, IntentRejection,
        MAX_DIAGNOSTIC_BYTES, OperationId, PortFailure, PortId, ProfileOverlayEdit, SetupState,
        SnapshotRevision,
    };
    use std::path::PathBuf;
    use std::sync::Arc;

    #[test]
    fn accepted_setup_edit_is_persisted_and_confirmed_through_public_seam() {
        let factory = Arc::new(DeterministicStateFactory::default());
        let sink = Arc::new(RecordingSink::default());
        let (handle, runtime) = ApplicationRuntime::start(Arc::clone(&factory), Arc::clone(&sink))
            .expect("application runtime should start");
        let initial = sink.wait_for(0);
        assert_eq!(initial.revision(), SnapshotRevision::INITIAL);
        assert!(!initial.setup().profile_overlay().dry_run());
        assert_eq!(initial.last_intent(), None);

        let receipt = handle
            .submit(Intent::EditProfileOverlay {
                expected_revision: initial.revision(),
                edit: ProfileOverlayEdit::SetDryRun(true),
            })
            .expect("the bounded queue should accept one setup edit");

        factory.wait_until_persisting();
        assert_eq!(sink.len(), 1, "a queue receipt must not confirm state");
        assert!(!initial.setup().profile_overlay().dry_run());

        factory.release_persistence();
        let confirmed = sink.wait_for(1);
        assert_eq!(confirmed.revision().get(), 1);
        assert!(confirmed.revision() > initial.revision());
        assert!(confirmed.setup().profile_overlay().dry_run());
        assert_eq!(
            confirmed.last_intent(),
            Some(&IntentOutcome::Applied(receipt))
        );
        runtime
            .shutdown()
            .expect("application runtime should shut down cleanly");

        let restarted_sink = Arc::new(RecordingSink::default());
        let (_restarted_handle, restarted_runtime) =
            ApplicationRuntime::start(Arc::clone(&factory), Arc::clone(&restarted_sink))
                .expect("application runtime should reload deterministic state");
        let reloaded = restarted_sink.wait_for(0);
        assert!(reloaded.setup().profile_overlay().dry_run());
        assert_eq!(reloaded.last_intent(), None);
        restarted_runtime
            .shutdown()
            .expect("restarted runtime should shut down cleanly");
    }

    #[test]
    fn stale_setup_edit_is_rejected_through_an_authoritative_snapshot() {
        let factory = Arc::new(DeterministicStateFactory::default());
        factory.release_persistence();
        let sink = Arc::new(RecordingSink::default());
        let (handle, runtime) = ApplicationRuntime::start(Arc::clone(&factory), Arc::clone(&sink))
            .expect("application runtime should start");
        let initial = sink.wait_for(0);

        handle
            .submit(Intent::EditProfileOverlay {
                expected_revision: initial.revision(),
                edit: ProfileOverlayEdit::SetDryRun(true),
            })
            .expect("the first edit should enter the queue");
        let applied = sink.wait_for(1);
        let stale_receipt = handle
            .submit(Intent::EditProfileOverlay {
                expected_revision: initial.revision(),
                edit: ProfileOverlayEdit::SetDryRun(false),
            })
            .expect("queue acceptance should precede semantic rejection");
        let rejected = sink.wait_for(2);

        assert!(applied.setup().profile_overlay().dry_run());
        assert!(rejected.setup().profile_overlay().dry_run());
        assert_eq!(rejected.revision().get(), 2);
        assert_eq!(
            rejected.last_intent(),
            Some(&IntentOutcome::Rejected {
                receipt: stale_receipt,
                rejection: IntentRejection::StaleRevision,
            })
        );
        runtime
            .shutdown()
            .expect("application runtime should shut down cleanly");
    }

    #[test]
    fn injected_persist_failure_is_confirmed_through_public_seam() {
        let failure = PortFailure::new(
            PortId::PortableState,
            OperationId::PersistSetup,
            FailureKind::Io,
            "injected durable write failure",
        );
        let factory = Arc::new(DeterministicStateFactory::with_faults(
            SetupState::default(),
            FaultPlan::fail_once(FaultPoint::PersistSetup, failure.clone()),
        ));
        let sink = Arc::new(RecordingSink::default());
        let (handle, runtime) = ApplicationRuntime::start(Arc::clone(&factory), Arc::clone(&sink))
            .expect("application runtime should start before the persist fault");
        let initial = sink.wait_for(0);

        let receipt = handle
            .submit(Intent::EditProfileOverlay {
                expected_revision: initial.revision(),
                edit: ProfileOverlayEdit::SetDryRun(true),
            })
            .expect("the faulted edit should still enter the bounded queue");
        let failed = sink.wait_for(1);

        assert!(!failed.setup().profile_overlay().dry_run());
        assert_eq!(
            failed.last_intent(),
            Some(&IntentOutcome::Failed { receipt, failure })
        );
        assert!(!factory.persisted_setup().profile_overlay().dry_run());
        runtime
            .shutdown()
            .expect("application runtime should shut down after a port fault");
    }

    #[test]
    fn committed_setup_manifest_replays_deterministic_and_fault_cases() {
        let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root should resolve");
        let manifest = repository_root.join("verification/tracers/setup/manifest.json");

        let report = run_setup_replay(&repository_root, &manifest)
            .expect("the governed setup tracer should replay");

        assert_eq!(
            report.case_ids(),
            [
                "persist-success",
                "open-unavailable",
                "load-corrupt-data",
                "persist-io",
            ]
        );
        assert!(report.sandbox_roots_are_distinct());
    }

    #[test]
    fn intent_queue_is_bounded_and_reports_busy_without_blocking() {
        let factory = Arc::new(DeterministicStateFactory::default());
        let sink = Arc::new(RecordingSink::default());
        let (handle, runtime) = ApplicationRuntime::start(Arc::clone(&factory), Arc::clone(&sink))
            .expect("application runtime should start");
        let initial = sink.wait_for(0);
        handle
            .submit(Intent::EditProfileOverlay {
                expected_revision: initial.revision(),
                edit: ProfileOverlayEdit::SetDryRun(true),
            })
            .expect("the supervisor should accept the blocking edit");
        factory.wait_until_persisting();

        let queued_results: Vec<_> = (0..64)
            .map(|_| {
                handle.submit(Intent::EditProfileOverlay {
                    expected_revision: initial.revision(),
                    edit: ProfileOverlayEdit::SetDryRun(true),
                })
            })
            .collect();
        let overflow_result = handle.submit(Intent::EditProfileOverlay {
            expected_revision: initial.revision(),
            edit: ProfileOverlayEdit::SetDryRun(true),
        });

        // Always unblock the supervisor before assertions so a failing test can shut down.
        factory.release_persistence();
        runtime
            .shutdown()
            .expect("application runtime should drain and shut down cleanly");
        assert!(queued_results.iter().all(Result::is_ok));
        assert_eq!(overflow_result, Err(IntentRejection::Busy));
    }

    #[test]
    fn public_failure_text_is_stable_owned_and_utf8_bounded() {
        let source = "é".repeat(MAX_DIAGNOSTIC_BYTES);
        let diagnostic = BoundedText::new(source);
        assert!(diagnostic.was_truncated());
        assert_eq!(diagnostic.as_str().len(), MAX_DIAGNOSTIC_BYTES);
        assert!(
            diagnostic
                .as_str()
                .is_char_boundary(diagnostic.as_str().len())
        );

        let failure = PortFailure::new(
            PortId::PortableState,
            OperationId::PersistSetup,
            FailureKind::Io,
            diagnostic.as_str(),
        );
        assert_eq!(failure.port(), PortId::PortableState);
        assert_eq!(failure.operation(), OperationId::PersistSetup);
        assert_eq!(failure.kind(), FailureKind::Io);
        assert_eq!(failure.diagnostic().as_str(), diagnostic.as_str());
    }
}
