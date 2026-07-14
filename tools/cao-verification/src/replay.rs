//! Manifest-driven replay of the setup tracer through the public application seam.

use crate::{DeterministicStateFactory, FaultPlan, RecordingSink};
use cao_application::{
    ApplicationRuntime, FailureKind, GlobalStateRecovery, GlobalStateRecoveryAction, Intent,
    IntentOutcome, OperationId, PortFailure, PortId, ProfileOverlay, ProfileOverlayEdit,
    SetupState, WorkbenchSnapshot,
};
use serde::Deserialize;
use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

static NEXT_SANDBOX_ID: AtomicU64 = AtomicU64::new(1);
const GOVERNED_SETUP_MANIFEST: &str = "verification/tracers/setup/manifest.json";

/// Successful result of replaying every case named by a governed setup manifest.
#[derive(Debug)]
pub struct ReplayReport {
    cases: Vec<CaseReport>,
}

impl ReplayReport {
    /// Returns case identifiers in manifest order.
    #[must_use]
    pub fn case_ids(&self) -> Vec<&str> {
        self.cases.iter().map(|case| case.id.as_str()).collect()
    }

    /// Reports whether every case ran inside a different fresh sandbox root.
    #[must_use]
    pub fn sandbox_roots_are_distinct(&self) -> bool {
        let roots: HashSet<_> = self.cases.iter().map(|case| &case.sandbox_root).collect();
        roots.len() == self.cases.len()
    }
}

#[derive(Debug)]
struct CaseReport {
    id: String,
    sandbox_root: PathBuf,
}

/// Failure to validate, materialize, or replay a governed setup tracer.
#[derive(Debug)]
pub enum ReplayError {
    /// A filesystem or process operation failed.
    Io(std::io::Error),
    /// A governed JSON document did not match the runner's typed contract.
    Json(serde_json::Error),
    /// The offline repository validator rejected the exact sandbox copy.
    Validation(String),
    /// A manifest path escaped the directory that owns it.
    EscapingArtifact(PathBuf),
    /// Fixture and evidence identities or case sets disagree.
    InvalidManifest(String),
    /// Observed public-seam behavior disagreed with the governed evidence.
    EvidenceMismatch { case_id: String, detail: String },
}

impl fmt::Display for ReplayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Json(error) => write!(formatter, "{error}"),
            Self::Validation(diagnostic) => {
                write!(
                    formatter,
                    "setup replay integrity validation failed: {diagnostic}"
                )
            }
            Self::EscapingArtifact(path) => {
                write!(
                    formatter,
                    "setup replay artifact escapes its manifest: {}",
                    path.display()
                )
            }
            Self::InvalidManifest(detail) => {
                write!(formatter, "invalid setup replay manifest: {detail}")
            }
            Self::EvidenceMismatch { case_id, detail } => {
                write!(
                    formatter,
                    "setup replay {case_id} disagreed with evidence: {detail}"
                )
            }
        }
    }
}

impl std::error::Error for ReplayError {}

impl From<std::io::Error> for ReplayError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for ReplayError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayManifest {
    schema_version: u64,
    baseline_revision: u64,
    id: String,
    revision: u64,
    fixture: ArtifactReference,
    evidence: ArtifactReference,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactReference {
    path: PathBuf,
    sha256: String,
    size_bytes: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetupFixture {
    schema_version: u64,
    baseline_revision: u64,
    id: String,
    revision: u64,
    cases: Vec<SetupCase>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetupCase {
    id: String,
    initial_state: InitialStateSpec,
    intent: Option<IntentSpec>,
    fault: Option<FaultSpec>,
}

#[derive(Clone, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case", deny_unknown_fields)]
enum InitialStateSpec {
    Ready {
        dry_run: bool,
    },
    RecoveryRequired {
        recovered_dry_run: bool,
        backup_available: bool,
        failure: FailureSpec,
    },
}

#[derive(Clone, Copy, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum IntentSpec {
    EditProfileOverlay { dry_run: bool },
    RecoverGlobalState { action: RecoveryActionSpec },
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum RecoveryActionSpec {
    RestoreBackup,
    Reset,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FaultSpec {
    point: FaultPointSpec,
    fixture_path: PathBuf,
    failure: FailureSpec,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum FaultPointSpec {
    Open,
    LoadSetup,
    PersistSetup,
    RestoreGlobalState,
    ResetGlobalState,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetupEvidence {
    schema_version: u64,
    baseline_revision: u64,
    id: String,
    revision: u64,
    fixture_ref: VersionedReference,
    cases: Vec<ExpectedCase>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VersionedReference {
    id: String,
    revision: u64,
}

#[derive(Deserialize)]
struct ExpectedCase {
    id: String,
    #[serde(flatten)]
    startup: StartupExpectation,
}

#[derive(Deserialize)]
#[serde(tag = "startup", rename_all = "kebab-case")]
enum StartupExpectation {
    Started {
        snapshots: Vec<SnapshotExpectation>,
        persisted_dry_run: bool,
        restart_dry_run: bool,
    },
    Failed {
        failure: FailureSpec,
        snapshot_count: usize,
        persisted_dry_run: bool,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotExpectation {
    revision: u64,
    dry_run: bool,
    recovery: Option<RecoveryExpectation>,
    last_intent: IntentExpectation,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryExpectation {
    backup_available: bool,
    failure: FailureSpec,
}

#[derive(Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
enum IntentExpectation {
    None,
    Applied,
    Failed { failure: FailureSpec },
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FailureSpec {
    port: PortSpec,
    operation: OperationSpec,
    kind: FailureKindSpec,
    subject: Option<String>,
    diagnostic: String,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum PortSpec {
    PortableState,
    ApplicationRuntime,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum OperationSpec {
    Open,
    LoadSetup,
    PersistSetup,
    MigrateState,
    RestoreGlobalState,
    ResetGlobalState,
    StartSupervisor,
    StopSupervisor,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum FailureKindSpec {
    InvalidInput,
    Unsupported,
    Unavailable,
    NotFound,
    PermissionDenied,
    Conflict,
    ResourceExhausted,
    CorruptData,
    Io,
    Protocol,
    BackendRejected,
    BackendCrashed,
    Integrity,
    Internal,
}

impl FailureSpec {
    /// Builds the stable application-owned failure injected at a selected port operation.
    fn to_failure(&self) -> PortFailure {
        let failure = PortFailure::new(
            self.port.to_port_id(),
            self.operation.to_operation_id(),
            self.kind.to_failure_kind(),
            &self.diagnostic,
        );
        match &self.subject {
            Some(subject) => failure.with_subject(subject),
            None => failure,
        }
    }

    /// Compares a public failure without depending on an adapter's private source error.
    fn matches(&self, failure: &PortFailure) -> bool {
        failure.port() == self.port.to_port_id()
            && failure.operation() == self.operation.to_operation_id()
            && failure.kind() == self.kind.to_failure_kind()
            && failure.subject().map(|subject| subject.as_str()) == self.subject.as_deref()
            && failure.diagnostic().as_str() == self.diagnostic
    }
}

impl PortSpec {
    const fn to_port_id(self) -> PortId {
        match self {
            Self::PortableState => PortId::PortableState,
            Self::ApplicationRuntime => PortId::ApplicationRuntime,
        }
    }
}

impl OperationSpec {
    const fn to_operation_id(self) -> OperationId {
        match self {
            Self::Open => OperationId::Open,
            Self::LoadSetup => OperationId::LoadSetup,
            Self::PersistSetup => OperationId::PersistSetup,
            Self::MigrateState => OperationId::MigrateState,
            Self::RestoreGlobalState => OperationId::RestoreGlobalState,
            Self::ResetGlobalState => OperationId::ResetGlobalState,
            Self::StartSupervisor => OperationId::StartSupervisor,
            Self::StopSupervisor => OperationId::StopSupervisor,
        }
    }
}

impl FailureKindSpec {
    const fn to_failure_kind(self) -> FailureKind {
        match self {
            Self::InvalidInput => FailureKind::InvalidInput,
            Self::Unsupported => FailureKind::Unsupported,
            Self::Unavailable => FailureKind::Unavailable,
            Self::NotFound => FailureKind::NotFound,
            Self::PermissionDenied => FailureKind::PermissionDenied,
            Self::Conflict => FailureKind::Conflict,
            Self::ResourceExhausted => FailureKind::ResourceExhausted,
            Self::CorruptData => FailureKind::CorruptData,
            Self::Io => FailureKind::Io,
            Self::Protocol => FailureKind::Protocol,
            Self::BackendRejected => FailureKind::BackendRejected,
            Self::BackendCrashed => FailureKind::BackendCrashed,
            Self::Integrity => FailureKind::Integrity,
            Self::Internal => FailureKind::Internal,
        }
    }
}

impl FaultPointSpec {
    /// Returns the application-owned operation key used by deterministic fault injection.
    const fn to_operation_id(self) -> OperationId {
        match self {
            Self::Open => OperationId::Open,
            Self::LoadSetup => OperationId::LoadSetup,
            Self::PersistSetup => OperationId::PersistSetup,
            Self::RestoreGlobalState => OperationId::RestoreGlobalState,
            Self::ResetGlobalState => OperationId::ResetGlobalState,
        }
    }
}

impl InitialStateSpec {
    /// Returns the deterministic setup stored behind this startup condition.
    fn setup(&self) -> SetupState {
        let dry_run = match self {
            Self::Ready { dry_run } => *dry_run,
            Self::RecoveryRequired {
                recovered_dry_run, ..
            } => *recovered_dry_run,
        };
        SetupState::default().with_profile_overlay(ProfileOverlay::default().with_dry_run(dry_run))
    }

    /// Builds the recovery projection required by corrupt initial state, when present.
    fn recovery(&self) -> Option<GlobalStateRecovery> {
        match self {
            Self::Ready { .. } => None,
            Self::RecoveryRequired {
                backup_available,
                failure,
                ..
            } => Some(GlobalStateRecovery::new(
                failure.to_failure(),
                *backup_available,
            )),
        }
    }
}

impl RecoveryActionSpec {
    /// Converts fixture vocabulary to the public application recovery choice.
    const fn to_recovery_action(self) -> GlobalStateRecoveryAction {
        match self {
            Self::RestoreBackup => GlobalStateRecoveryAction::RestoreBackup,
            Self::Reset => GlobalStateRecoveryAction::Reset,
        }
    }
}

/// Replays all cases in a governed setup manifest after validating each fresh copy.
///
/// The repository validator is run against the exact copied `verification/` tree before
/// every case. Replay drives only `ApplicationRuntime`, `ApplicationHandle`, and snapshots.
///
/// # Errors
///
/// Returns [`ReplayError`] when governed bytes fail validation, sandbox creation fails,
/// or observed public-seam behavior differs from the evidence document.
pub fn run_setup_replay(
    repository_root: &Path,
    manifest_path: &Path,
) -> Result<ReplayReport, ReplayError> {
    let repository_root = repository_root.canonicalize()?;
    let manifest_path = manifest_path.canonicalize()?;
    let manifest_relative = manifest_path
        .strip_prefix(&repository_root)
        .map_err(|_| ReplayError::EscapingArtifact(manifest_path.clone()))?
        .to_path_buf();
    if manifest_relative != Path::new(GOVERNED_SETUP_MANIFEST) {
        return Err(ReplayError::InvalidManifest(format!(
            "setup replay must use the governed {GOVERNED_SETUP_MANIFEST}"
        )));
    }
    let source_manifest: ReplayManifest = read_json(&manifest_path)?;
    touch_manifest_identity(&source_manifest);
    let source_fixture_path = resolve_artifact(&manifest_path, &source_manifest.fixture)?;
    let source_fixture: SetupFixture = read_json(&source_fixture_path)?;
    touch_fixture_identity(&source_fixture);
    let case_ids: Vec<_> = source_fixture
        .cases
        .iter()
        .map(|case| case.id.clone())
        .collect();
    reject_duplicate_ids("fixture", &case_ids)?;

    let mut cases = Vec::with_capacity(case_ids.len());
    for case_id in case_ids {
        let sandbox = FreshSandbox::create()?;
        let sandbox_repository = sandbox.path().join("repository");
        copy_verification_tree(&repository_root, &sandbox_repository)?;
        validate_sandbox(&repository_root, &sandbox_repository)?;

        let sandbox_manifest_path = sandbox_repository.join(&manifest_relative);
        let manifest: ReplayManifest = read_json(&sandbox_manifest_path)?;
        touch_manifest_identity(&manifest);
        let fixture_path = resolve_artifact(&sandbox_manifest_path, &manifest.fixture)?;
        let evidence_path = resolve_artifact(&sandbox_manifest_path, &manifest.evidence)?;
        let fixture: SetupFixture = read_json(&fixture_path)?;
        let evidence: SetupEvidence = read_json(&evidence_path)?;
        validate_document_links(&fixture, &evidence)?;

        let fixture_case = find_case(&fixture.cases, &case_id, "fixture")?;
        let expected_case = find_expected_case(&evidence.cases, &case_id)?;
        let portable_state_tree = sandbox.path().join("portable-state");
        replay_case(fixture_case, expected_case, &portable_state_tree)?;
        cases.push(CaseReport {
            id: case_id,
            sandbox_root: sandbox.path().to_path_buf(),
        });
    }

    Ok(ReplayReport { cases })
}

/// Retains manifest identity fields so typed parsing rejects absent or malformed governance.
fn touch_manifest_identity(manifest: &ReplayManifest) {
    let _ = (
        manifest.schema_version,
        manifest.baseline_revision,
        manifest.id.as_str(),
        manifest.revision,
        manifest.fixture.sha256.as_str(),
        manifest.fixture.size_bytes,
        manifest.evidence.sha256.as_str(),
        manifest.evidence.size_bytes,
    );
}

/// Retains fixture identity fields so the runner consumes the same governed contract.
fn touch_fixture_identity(fixture: &SetupFixture) {
    let _ = (
        fixture.schema_version,
        fixture.baseline_revision,
        fixture.id.as_str(),
        fixture.revision,
    );
}

/// Confirms that evidence describes the exact versioned setup fixture loaded by the runner.
fn validate_document_links(
    fixture: &SetupFixture,
    evidence: &SetupEvidence,
) -> Result<(), ReplayError> {
    let _ = (
        evidence.schema_version,
        evidence.baseline_revision,
        evidence.id.as_str(),
        evidence.revision,
    );
    if evidence.fixture_ref.id != fixture.id || evidence.fixture_ref.revision != fixture.revision {
        return Err(ReplayError::InvalidManifest(
            "evidence fixture_ref does not match the loaded fixture".to_owned(),
        ));
    }
    let fixture_ids: Vec<_> = fixture.cases.iter().map(|case| case.id.clone()).collect();
    let evidence_ids: Vec<_> = evidence.cases.iter().map(|case| case.id.clone()).collect();
    reject_duplicate_ids("fixture", &fixture_ids)?;
    reject_duplicate_ids("evidence", &evidence_ids)?;
    if fixture_ids != evidence_ids {
        return Err(ReplayError::InvalidManifest(
            "fixture and evidence case order or membership differs".to_owned(),
        ));
    }
    Ok(())
}

/// Rejects ambiguous case lookup before any application runtime is started.
fn reject_duplicate_ids(source: &str, ids: &[String]) -> Result<(), ReplayError> {
    let mut unique = HashSet::new();
    for id in ids {
        if !unique.insert(id) {
            return Err(ReplayError::InvalidManifest(format!(
                "duplicate {source} case id {id}"
            )));
        }
    }
    Ok(())
}

/// Finds one fixture case after duplicate identifiers have been rejected.
fn find_case<'a>(
    cases: &'a [SetupCase],
    id: &str,
    source: &str,
) -> Result<&'a SetupCase, ReplayError> {
    cases
        .iter()
        .find(|case| case.id == id)
        .ok_or_else(|| ReplayError::InvalidManifest(format!("missing {source} case {id}")))
}

/// Finds one evidence case corresponding to a fixture case.
fn find_expected_case<'a>(
    cases: &'a [ExpectedCase],
    id: &str,
) -> Result<&'a ExpectedCase, ReplayError> {
    cases
        .iter()
        .find(|case| case.id == id)
        .ok_or_else(|| ReplayError::InvalidManifest(format!("missing evidence case {id}")))
}

/// Runs one fixture case and compares only stable public snapshots and port failures.
fn replay_case(
    fixture: &SetupCase,
    expected: &ExpectedCase,
    portable_state_tree: &Path,
) -> Result<(), ReplayError> {
    let setup = fixture.initial_state.setup();
    let faults = fixture
        .fault
        .as_ref()
        .map_or_else(FaultPlan::default, |fault| {
            FaultPlan::fail_once_at(
                fault.point.to_operation_id(),
                &fault.fixture_path,
                fault.failure.to_failure(),
            )
        });
    let factory = Arc::new(match fixture.initial_state.recovery() {
        Some(recovery) => DeterministicStateFactory::in_sandbox_requiring_recovery(
            portable_state_tree,
            setup,
            recovery,
            faults,
        )?,
        None => DeterministicStateFactory::in_sandbox(portable_state_tree, setup, faults)?,
    });
    factory.release_persistence();
    let sink = Arc::new(RecordingSink::default());
    let started = ApplicationRuntime::start(Arc::clone(&factory), Arc::clone(&sink));

    match (&expected.startup, started) {
        (
            StartupExpectation::Failed {
                failure,
                snapshot_count,
                persisted_dry_run,
            },
            Err(actual_failure),
        ) => {
            ensure(
                &fixture.id,
                failure.matches(&actual_failure),
                "startup failure did not match stable evidence",
            )?;
            ensure(
                &fixture.id,
                sink.len() == *snapshot_count,
                "startup published an unexpected snapshot count",
            )?;
            ensure(
                &fixture.id,
                factory.persisted_setup().profile_overlay().dry_run() == *persisted_dry_run,
                "startup fault changed persisted setup",
            )
        }
        (StartupExpectation::Failed { .. }, Ok((_handle, runtime))) => {
            // Cleanup is best effort; the evidence mismatch remains the authoritative failure.
            let _ = runtime.shutdown();
            mismatch(
                &fixture.id,
                "startup succeeded when evidence requires failure",
            )
        }
        (
            StartupExpectation::Started {
                snapshots,
                persisted_dry_run,
                restart_dry_run,
            },
            Ok((handle, runtime)),
        ) => {
            ensure(
                &fixture.id,
                snapshots.len() == 2,
                "started setup cases must govern initial and confirmed snapshots",
            )?;
            compare_snapshot(&fixture.id, &snapshots[0], &sink.wait_for(0), None)?;
            let intent = fixture.intent.ok_or_else(|| {
                ReplayError::InvalidManifest(format!("started case {} has no intent", fixture.id))
            })?;
            let expected_revision = sink.wait_for(0).revision();
            let intent = match intent {
                IntentSpec::EditProfileOverlay { dry_run } => Intent::EditProfileOverlay {
                    expected_revision,
                    edit: ProfileOverlayEdit::SetDryRun(dry_run),
                },
                IntentSpec::RecoverGlobalState { action } => Intent::RecoverGlobalState {
                    expected_revision,
                    action: action.to_recovery_action(),
                },
            };
            let receipt =
                handle
                    .submit(intent)
                    .map_err(|rejection| ReplayError::EvidenceMismatch {
                        case_id: fixture.id.clone(),
                        detail: format!("setup intent was rejected at transport: {rejection:?}"),
                    })?;
            compare_snapshot(&fixture.id, &snapshots[1], &sink.wait_for(1), Some(receipt))?;
            runtime
                .shutdown()
                .map_err(|failure| ReplayError::EvidenceMismatch {
                    case_id: fixture.id.clone(),
                    detail: format!("runtime shutdown failed: {:?}", failure.kind()),
                })?;
            ensure(
                &fixture.id,
                factory.persisted_setup().profile_overlay().dry_run() == *persisted_dry_run,
                "persisted setup did not match evidence",
            )?;

            let restart_sink = Arc::new(RecordingSink::default());
            let (_restart_handle, restart_runtime) =
                ApplicationRuntime::start(Arc::clone(&factory), Arc::clone(&restart_sink))
                    .map_err(|failure| ReplayError::EvidenceMismatch {
                        case_id: fixture.id.clone(),
                        detail: format!("restart failed: {:?}", failure.kind()),
                    })?;
            let restarted = restart_sink.wait_for(0);
            ensure(
                &fixture.id,
                restarted.setup().profile_overlay().dry_run() == *restart_dry_run,
                "restarted setup did not match evidence",
            )?;
            restart_runtime
                .shutdown()
                .map_err(|failure| ReplayError::EvidenceMismatch {
                    case_id: fixture.id.clone(),
                    detail: format!("restarted runtime shutdown failed: {:?}", failure.kind()),
                })
        }
        (StartupExpectation::Started { .. }, Err(failure)) => mismatch(
            &fixture.id,
            &format!("startup failed unexpectedly with {:?}", failure.kind()),
        ),
    }
}

/// Compares one immutable snapshot and correlates an accepted receipt when applicable.
fn compare_snapshot(
    case_id: &str,
    expected: &SnapshotExpectation,
    actual: &WorkbenchSnapshot,
    receipt: Option<cao_application::IntentReceipt>,
) -> Result<(), ReplayError> {
    ensure(
        case_id,
        actual.revision().get() == expected.revision,
        "snapshot revision differs",
    )?;
    ensure(
        case_id,
        actual.setup().profile_overlay().dry_run() == expected.dry_run,
        "snapshot dry-run setup differs",
    )?;
    match (&expected.recovery, actual.global_state_recovery()) {
        (None, None) => {}
        (Some(expected), Some(actual))
            if expected.backup_available == actual.backup_available()
                && expected.failure.matches(actual.corrupt_failure()) => {}
        _ => return mismatch(case_id, "global-state recovery projection differs"),
    }
    match (&expected.last_intent, actual.last_intent(), receipt) {
        (IntentExpectation::None, None, None) => Ok(()),
        (IntentExpectation::Applied, Some(IntentOutcome::Applied(actual)), Some(expected))
            if *actual == expected =>
        {
            Ok(())
        }
        (
            IntentExpectation::Failed { failure },
            Some(IntentOutcome::Failed {
                receipt: actual_receipt,
                failure: actual_failure,
            }),
            Some(expected_receipt),
        ) if *actual_receipt == expected_receipt && failure.matches(actual_failure) => Ok(()),
        _ => mismatch(case_id, "last intent outcome differs"),
    }
}

/// Converts a boolean evidence assertion into a case-scoped replay error.
fn ensure(case_id: &str, condition: bool, detail: &str) -> Result<(), ReplayError> {
    if condition {
        Ok(())
    } else {
        mismatch(case_id, detail)
    }
}

/// Creates a case-scoped mismatch while keeping diagnostics stable and concise.
fn mismatch<T>(case_id: &str, detail: &str) -> Result<T, ReplayError> {
    Err(ReplayError::EvidenceMismatch {
        case_id: case_id.to_owned(),
        detail: detail.to_owned(),
    })
}

/// Deserializes one governed JSON document from disk.
fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, ReplayError> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

/// Resolves one manifest-owned artifact while rejecting absolute and escaping paths.
fn resolve_artifact(
    manifest_path: &Path,
    reference: &ArtifactReference,
) -> Result<PathBuf, ReplayError> {
    if reference.path.is_absolute()
        || reference
            .path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(ReplayError::EscapingArtifact(reference.path.clone()));
    }
    let owner = manifest_path.parent().ok_or_else(|| {
        ReplayError::InvalidManifest("manifest has no owning directory".to_owned())
    })?;
    let resolved = owner.join(&reference.path).canonicalize()?;
    let owner = owner.canonicalize()?;
    if !resolved.starts_with(&owner) {
        return Err(ReplayError::EscapingArtifact(reference.path.clone()));
    }
    Ok(resolved)
}

/// Copies governed verification inputs into the exact repo-shaped sandbox being validated.
fn copy_verification_tree(source_root: &Path, destination_root: &Path) -> Result<(), ReplayError> {
    let source = source_root.join("verification");
    let destination = destination_root.join("verification");
    copy_tree(&source, &destination)
}

/// Recursively copies governed files while excluding every local oracle kit payload.
fn copy_tree(source: &Path, destination: &Path) -> Result<(), ReplayError> {
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        if entry.file_name() == "local-oracle-kit" {
            continue;
        }
        let destination_path = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&source_path, &destination_path)?;
        } else {
            std::fs::copy(source_path, destination_path)?;
        }
    }
    Ok(())
}

/// Runs the repository's public offline validator against the exact sandbox bytes.
fn validate_sandbox(source_root: &Path, sandbox_root: &Path) -> Result<(), ReplayError> {
    let python = std::env::var_os("CAO_PYTHON_COMMAND").unwrap_or_else(|| "python".into());
    let output = Command::new(python)
        .arg(source_root.join("tools/verify_baseline.py"))
        .arg("--root")
        .arg(sandbox_root)
        .output()?;
    if output.status.success() {
        return Ok(());
    }
    let diagnostic = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(ReplayError::Validation(diagnostic))
}

/// Unique temporary directory removed on a best-effort basis after one replay case.
struct FreshSandbox {
    path: PathBuf,
}

impl FreshSandbox {
    /// Creates a collision-resistant empty directory for exactly one replay case.
    fn create() -> Result<Self, ReplayError> {
        let parent = std::env::temp_dir();
        for _ in 0..100 {
            let id = NEXT_SANDBOX_ID.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(
                "tracetide-setup-replay-{}-{id}",
                std::process::id()
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(ReplayError::Io(error)),
            }
        }
        Err(ReplayError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not allocate a unique setup replay sandbox",
        )))
    }

    /// Returns the unique root owned by this replay case.
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for FreshSandbox {
    fn drop(&mut self) {
        // Cleanup cannot change replay evidence and must not mask the primary result.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
