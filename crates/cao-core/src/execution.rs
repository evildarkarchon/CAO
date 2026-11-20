//! Asset execution: the Asset Execution Backend seam and the Asset Executor.
//!
//! Ported from the C++ `cao::execution` module (`src/AssetExecution`). The
//! executor carries a Routed Asset's facts to one backend without reclassifying
//! it, and reports one Operation Failure shape for every unsuccessful attempt.
//!
//! Apply-mode persistence goes through staged publication under the run's
//! Temporary Ownership: the executor captures each destination before the
//! backend loads the original bytes, has the backend save into a durable
//! staged sibling, and publishes it with `Replace`. The publication result's
//! mutation fact becomes the attempt's. Dry Run never stages anything.
//!
//! A Texture conversion is the one producer whose output is a different file
//! from its input, and it never replaces: when a file already stands at its
//! DDS destination the attempt completes without touching either file (see
//! `conversion_withheld`), and otherwise it publishes with `NoReplace`.
//!
//! [`quarantine_failed_load`] is the adapter step C++ `MainOptimizer` ran after
//! each attempt: in Apply, a Texture or Mesh whose content the backend cannot
//! decode becomes `.caobad`. One it could not read at all ([`LoadFailure`])
//! is left where it is.

use std::fs::File;
use std::io::{Seek, SeekFrom};
use std::path::{Path, PathBuf};

use cao_winfs::{
    Access, FileFacts, FileIdentity, Open, RenameMode, Share, delete_by_handle, rename_by_handle,
};

use crate::attempt::{AttemptOutcome, CompletedMutation, OperationFailure};
use crate::routing::{
    AssetIdentity, AssetOperation, AssetOperations, ExecutionMode, MeshVariant, OptimizerTarget,
    RoutedAsset, TextureVariant,
};
use crate::run::{
    PublicationPolicy, PublicationReceipt, PublicationResult, PublicationState, RunFailure,
    RunPhase, StagingError, TemporaryArtifactRegistry,
};
use crate::run::{fingerprint, ineligibility, probe_ineligibility};

/// Filesystem effects on durable Assets, excluding registry-owned temporary bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MutationState {
    #[default]
    None,
    Committed,
    PartialOrUnknown,
}

/// Closed failure categories, reported without rewriting the Routing Decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssetExecutionFailure {
    UnsupportedTarget,
    IdentityMismatch,
    /// The backend read the Asset and cannot decode its content; Apply
    /// quarantines it.
    LoadFailed,
    /// The backend could not read the Asset, or ran out of memory decoding
    /// it, so nothing is known about its content. It is never quarantined,
    /// and the run may continue.
    ReadFailed,
    OperationFailed,
    SaveFailed,
    SourceRemovalFailed,
    StagingFailed,
    /// The Asset is a link or a hard-linked file, which CAO never replaces.
    /// Found before anything is staged, in Apply and in Dry Run alike, so
    /// nothing changed: it is never quarantined, and the run may continue.
    Ineligible,
    CommitFailed,
    /// The backend panicked, or failed in a way that leaves mutation unknown.
    BackendException,
    CleanupFailed,
}

/// Stable boundary categories, so adapters never parse backend diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExecutionFailureCategory {
    Backend,
    Filesystem,
    Contract,
}

/// The result of one backend operation against the loaded Asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationResult {
    succeeded: bool,
    would_change: bool,
    message: String,
}

impl OperationResult {
    /// A successful operation that changed, or in Dry Run would change, the loaded Asset.
    pub fn changed() -> Self {
        Self {
            succeeded: true,
            would_change: true,
            message: String::new(),
        }
    }

    /// A successful operation that found nothing to change.
    pub fn unchanged() -> Self {
        Self {
            succeeded: true,
            would_change: false,
            message: String::new(),
        }
    }

    /// A failed operation with a presentable diagnostic.
    pub fn failed(message: impl Into<String>) -> Self {
        Self {
            succeeded: false,
            would_change: false,
            message: message.into(),
        }
    }

    /// Reports whether the operation completed successfully.
    pub fn succeeded(&self) -> bool {
        self.succeeded
    }

    /// Reports whether Apply must persist the loaded Asset after this operation.
    pub fn would_change(&self) -> bool {
        self.would_change
    }

    /// The backend diagnostic; empty after success.
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Why a backend could not load an Asset, which decides whether Apply
/// quarantines it.
///
/// Quarantine renames the user's file, so it is kept for files whose content
/// is the problem. A file that merely could not be read this time, because
/// another program holds it or access is denied, is left alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadFailure {
    /// The file could not be read, or decoding it ran out of memory: nothing
    /// is known about its content. The attempt fails with
    /// [`AssetExecutionFailure::ReadFailed`], is never quarantined, and the
    /// run may continue.
    Unreadable(String),
    /// The file was read and its content cannot be decoded. The attempt fails
    /// with [`AssetExecutionFailure::LoadFailed`], which Apply quarantines.
    Undecodable(String),
}

impl LoadFailure {
    /// The backend's detail, which becomes the attempt's service detail.
    pub fn detail(&self) -> &str {
        match self {
            Self::Unreadable(detail) | Self::Undecodable(detail) => detail,
        }
    }
}

/// The observable outcome of executing one Routed Asset.
///
/// It embeds the attempt's [`AttemptOutcome`], which owns the mutation state,
/// the Operation Failure (category plus presentable message) and the
/// safe-to-continue verdict; partial or unknown mutation is always unsafe,
/// whatever the producer claimed. The Asset-only failure context (operation,
/// affected path, service detail and attempt-local cleanup failures) stays on
/// the result itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetExecutionResult {
    outcome: AttemptOutcome<AssetExecutionFailure>,
    path: PathBuf,
    operation: String,
    service_detail: String,
    cleanup_failures: Vec<RunFailure>,
}

impl AssetExecutionResult {
    /// A completed attempt that left exactly `mutation`: no mutation or a
    /// Committed Mutation, never partial or unknown.
    pub fn success(mutation: CompletedMutation) -> Self {
        Self {
            outcome: AttemptOutcome::success(mutation),
            path: PathBuf::new(),
            operation: String::new(),
            service_detail: String::new(),
            cleanup_failures: Vec::new(),
        }
    }

    /// A failed attempt with no mutation, safe to continue, at the generic
    /// `execute_asset` boundary. Refine it with the `with_*` builders.
    pub fn failed(failure: AssetExecutionFailure, message: impl Into<String>) -> Self {
        Self {
            outcome: AttemptOutcome::failed(
                OperationFailure::new(failure, message),
                MutationState::None,
                true,
            ),
            path: PathBuf::new(),
            operation: "execute_asset".to_owned(),
            service_detail: String::new(),
            cleanup_failures: Vec::new(),
        }
    }

    /// Sets the durable mutation known at the failed boundary.
    ///
    /// Failure-only: on a successful result this trips a debug assertion and
    /// is otherwise ignored, because a success cannot carry partial or
    /// unknown mutation.
    pub fn with_mutation(mut self, mutation: MutationState) -> Self {
        match &mut self.outcome {
            AttemptOutcome::Failed { mutation: held, .. } => *held = mutation,
            AttemptOutcome::Succeeded { .. } => {
                debug_assert!(false, "failure-only builder `with_mutation` on a success");
            }
        }
        self
    }

    /// Sets the producer's continuation claim; partial or unknown mutation
    /// still makes the verdict `false`.
    ///
    /// Failure-only: on a successful result this trips a debug assertion and
    /// is otherwise ignored, because a success is always safe to continue.
    pub fn with_safe_to_continue(mut self, safe_to_continue: bool) -> Self {
        match &mut self.outcome {
            AttemptOutcome::Failed {
                claimed_safe_to_continue,
                ..
            } => *claimed_safe_to_continue = safe_to_continue,
            AttemptOutcome::Succeeded { .. } => {
                debug_assert!(
                    false,
                    "failure-only builder `with_safe_to_continue` on a success"
                );
            }
        }
        self
    }

    /// Sets the failing boundary's affected source or destination path.
    pub fn with_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = path.into();
        self
    }

    /// Sets the stable operation name, such as `load_texture`.
    pub fn with_operation(mut self, operation: impl Into<String>) -> Self {
        self.operation = operation.into();
        self
    }

    /// Sets backend-specific detail, kept apart from the presentable message.
    pub fn with_service_detail(mut self, detail: impl Into<String>) -> Self {
        self.service_detail = detail.into();
        self
    }

    /// The attempt's Attempt Outcome: its mutation state, its Operation
    /// Failure if it failed, and the safe-to-continue verdict.
    pub fn outcome(&self) -> &AttemptOutcome<AssetExecutionFailure> {
        &self.outcome
    }

    /// Reports whether every carried operation completed.
    pub fn succeeded(&self) -> bool {
        self.outcome.succeeded()
    }

    /// The stable failure category, or `None` after success.
    pub fn failure(&self) -> Option<AssetExecutionFailure> {
        self.outcome.failure_category()
    }

    /// The presentable diagnostic, which is the Operation Failure's detail;
    /// empty after success.
    pub fn message(&self) -> &str {
        self.outcome.failure().map_or("", OperationFailure::detail)
    }

    /// Backend-specific detail, kept apart from the message.
    pub fn service_detail(&self) -> &str {
        &self.service_detail
    }

    /// The exact durable mutation known at the failed or completed boundary.
    pub fn mutation_state(&self) -> MutationState {
        self.outcome.mutation()
    }

    /// Whether the run may continue: the Attempt Outcome's verdict, so partial
    /// or unknown mutation is never safe.
    pub fn safe_to_continue(&self) -> bool {
        self.outcome.safe_to_continue()
    }

    /// The failing boundary's affected path.
    pub fn affected_path(&self) -> &Path {
        &self.path
    }

    /// The stable operation name, apart from backend detail.
    pub fn operation(&self) -> &str {
        &self.operation
    }

    /// Safety Cleanup for cleanup failures, Processing Assets for everything else.
    pub fn phase(&self) -> RunPhase {
        if self.failure() == Some(AssetExecutionFailure::CleanupFailed) {
            RunPhase::SafetyCleanup
        } else {
            RunPhase::ProcessingAssets
        }
    }

    /// The stable category of a failed attempt, or `None` after success.
    pub fn failure_category(&self) -> Option<ExecutionFailureCategory> {
        use AssetExecutionFailure::*;
        self.failure().map(|failure| match failure {
            UnsupportedTarget | IdentityMismatch | BackendException => {
                ExecutionFailureCategory::Contract
            }
            ReadFailed | SaveFailed | SourceRemovalFailed | StagingFailed | Ineligible
            | CommitFailed | CleanupFailed => ExecutionFailureCategory::Filesystem,
            LoadFailed | OperationFailed => ExecutionFailureCategory::Backend,
        })
    }

    /// Attempt-local cleanup failures, kept apart from the primary failure.
    pub fn cleanup_failures(&self) -> &[RunFailure] {
        &self.cleanup_failures
    }
}

/// The stateful adapter one Asset Executor drives: one trait over all optimizers.
///
/// A backend holds at most one loaded Asset at a time. Load and optimize calls
/// may change only that loaded state; durable files change only through the
/// save, remove and Animation output calls, and only at paths the executor
/// supplies.
///
/// Every call reports its own failure with its detail, which becomes the
/// attempt's service detail. A failure a backend can describe, a C++
/// exception inside a native library included, is reported that way. A panic
/// is for bugs: it is contained by the run and becomes an Operation Failure
/// with unknown mutation that stops the run.
pub trait AssetExecutionBackend {
    /// Loads one Texture according to its Variant, without mutating durable files.
    ///
    /// # Errors
    /// [`LoadFailure::Unreadable`] when the file cannot be read or decoding
    /// runs out of memory, and [`LoadFailure::Undecodable`] when its content
    /// is not a Texture the backend can decode.
    fn load_texture(&mut self, path: &Path, variant: TextureVariant) -> Result<(), LoadFailure>;

    /// Applies or evaluates the Texture operations against the loaded Texture.
    fn optimize_texture(
        &mut self,
        operations: AssetOperations,
        mode: ExecutionMode,
    ) -> OperationResult;

    /// Writes the loaded Texture to the supplied staging path, closing every
    /// handle before returning.
    ///
    /// # Errors
    /// The detail of why the Texture could not be encoded or written.
    fn save_texture(&mut self, path: &Path) -> Result<(), String>;

    /// Requests removal of a converted source after its replacement is
    /// committed. The backend must delete through `remove_verified`, which
    /// removes only the pinned source identity; pathname removal is never
    /// authorized. `Ok` alone never counts as a removal: the executor also
    /// requires `remove_verified` to have removed the source.
    ///
    /// # Errors
    /// The detail of why the source was not removed.
    fn remove_texture(
        &mut self,
        path: &Path,
        remove_verified: &mut dyn FnMut() -> bool,
    ) -> Result<(), String>;

    /// Loads one Mesh according to its Variant, without mutating durable files.
    ///
    /// # Errors
    /// As for [`load_texture`](Self::load_texture), for a Mesh.
    fn load_mesh(&mut self, path: &Path, variant: MeshVariant) -> Result<(), LoadFailure>;

    /// Applies or evaluates ordinary optimization against the loaded Mesh; `path`
    /// is context, never a destination.
    ///
    /// `mod_root` is the canonical Mod Root the attempt is attributed to, or
    /// empty for a standalone call without one. A backend matches the Mesh's
    /// game path within it, such as against Headpart Meshes (deviation 17).
    fn optimize_mesh(
        &mut self,
        path: &Path,
        mod_root: &Path,
        mode: ExecutionMode,
    ) -> OperationResult;

    /// Applies or evaluates Mesh Reference Maintenance against the loaded Mesh.
    fn maintain_mesh_references(&mut self, mode: ExecutionMode) -> OperationResult;

    /// Writes the loaded Mesh to the supplied staging path after all operations.
    ///
    /// # Errors
    /// The detail of why the Mesh could not be written whole.
    fn save_mesh(&mut self, path: &Path) -> Result<(), String>;

    /// Reads the source Animation and, in Apply, writes changed output only to
    /// `output_path`. Dry Run receives no output path and must not write.
    fn optimize_animation(
        &mut self,
        path: &Path,
        output_path: Option<&Path>,
        mode: ExecutionMode,
    ) -> OperationResult;

    /// Notes that the attempt at `asset` finished with the executor's own
    /// `result`, before any Quarantine changes it and before the next Asset
    /// executes. Does nothing by default.
    ///
    /// A backend whose later attempts depend on earlier outcomes, such as one
    /// withholding Mesh reference rewrites to a Texture whose conversion
    /// failed, records them here.
    fn attempt_finished(&mut self, _asset: &RoutedAsset, _result: &AssetExecutionResult) {}
}

/// The result of a capture before anything is staged: pinning a converted
/// Texture's source, or capturing a destination.
///
/// Nothing has been staged or changed, and the next Asset does not depend on
/// this one, so it is safe to continue whatever the error. A link or a
/// hard-linked file is [`AssetExecutionFailure::Ineligible`]; any other error,
/// such as a sharing violation or a changed parent, is `StagingFailed`.
/// Dry Run reports the same `Ineligible` result through
/// [`probe_ineligibility`], so it predicts what Apply would refuse.
fn capture_failed(
    kind: &str,
    boundary: &str,
    path: &Path,
    error: &StagingError,
) -> AssetExecutionResult {
    let (category, message) = match error {
        StagingError::Ineligible { .. } => (
            AssetExecutionFailure::Ineligible,
            format!("{kind} is a link or a hard-linked file, which CAO does not replace."),
        ),
        _ => (
            AssetExecutionFailure::StagingFailed,
            format!("Failed to prepare {kind} staging."),
        ),
    };
    AssetExecutionResult::failed(category, message)
        .with_path(path)
        .with_operation(boundary)
        .with_service_detail(error.to_string())
}

/// Abandons a staged file the attempt will not publish, so its sibling and
/// manifest record do not outlive the attempt. A failure only logs: what
/// remains stays owned, for Safety Cleanup, and the attempt's own result is
/// what the run needs.
fn abandon(receipt: PublicationReceipt, staged: &Path) {
    if let Err(error) = receipt.abandon() {
        log::warn!("{} is left for Safety Cleanup: {error}", staged.display());
    }
}

/// The `StagingFailed` result of a producer whose staging boundary failed,
/// from staging onward.
///
/// Only a [`StagingError::is_lookup`] failure is safe to continue: it comes
/// before the ownership manifest changes. Any other may leave the run's
/// Temporary Ownership scope where the next staging operation would fail the
/// same way, so it stops the run.
fn staging_failed(
    kind: &str,
    boundary: &str,
    path: &Path,
    mutation: MutationState,
    error: &StagingError,
) -> AssetExecutionResult {
    AssetExecutionResult::failed(
        AssetExecutionFailure::StagingFailed,
        format!("Failed to prepare {kind} staging."),
    )
    .with_mutation(mutation)
    .with_safe_to_continue(error.is_lookup())
    .with_path(path)
    .with_operation(boundary)
    .with_service_detail(error.to_string())
}

/// The result of a load the backend refused, with the backend's detail.
///
/// Content the backend cannot decode is `LoadFailed`, which Apply
/// quarantines; a file it could not read is `ReadFailed`, which it does not.
/// Both leave nothing changed and are safe to continue: the next Asset does
/// not depend on this one.
fn load_failed(
    kind: &str,
    operation: &str,
    path: &Path,
    failure: LoadFailure,
) -> AssetExecutionResult {
    let (category, message) = match &failure {
        LoadFailure::Undecodable(_) => (
            AssetExecutionFailure::LoadFailed,
            format!("Failed to load {kind}."),
        ),
        LoadFailure::Unreadable(_) => (
            AssetExecutionFailure::ReadFailed,
            format!("Failed to read {kind}."),
        ),
    };
    AssetExecutionResult::failed(category, message)
        .with_path(path)
        .with_operation(operation)
        .with_service_detail(failure.detail())
}

/// The `CommitFailed` result of a publication that did not release its
/// temporary name, or `None` when it completed. The mutation and the
/// continuation verdict are the receipt's.
fn publication_failure(
    publication: &PublicationResult,
    message: &str,
    path: &Path,
    boundary: &str,
) -> Option<AssetExecutionResult> {
    if publication.state == PublicationState::PublishedAndReleased {
        return None;
    }
    Some(
        AssetExecutionResult::failed(AssetExecutionFailure::CommitFailed, message)
            .with_mutation(publication.mutation())
            .with_safe_to_continue(publication.safe_to_continue())
            .with_path(path)
            .with_operation(boundary)
            .with_service_detail(publication.error_detail.clone()),
    )
}

/// The completed, mutation-free result of a Texture conversion whose DDS
/// destination is already occupied, logged so the user learns why the source
/// is still there.
///
/// The DDS already in place is the file the game loads; the convertible source
/// beside it is whatever its author left behind. Converting would replace the
/// first with a re-encode of the second and then delete the second, and
/// nothing could undo that. So neither file is touched.
///
/// This is a success, not an Operation Failure: nothing went wrong, and a
/// backend that withholds Mesh reference rewrites after a failed conversion
/// must not withhold this one, because the DDS the reference would name
/// exists.
fn conversion_withheld(source: &Path, output: &Path) -> AssetExecutionResult {
    log::warn!(
        "{} was not converted: {} already exists and was kept.",
        source.display(),
        output.display()
    );
    AssetExecutionResult::success(CompletedMutation::None)
}

/// Whether a file stands at `path`, read without following a link.
///
/// Dry Run's stand-in for [`PublicationTarget::is_occupied`]: it captures
/// nothing, yet must report the same decision Apply would make. A directory
/// does not count, as in the capture.
///
/// [`PublicationTarget::is_occupied`]: crate::run::PublicationTarget::is_occupied
fn file_stands_at(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| !metadata.is_dir())
}

/// The absolute form of an execution path; discovery already yields absolute
/// paths, so this only guards a relative one from a test or adapter.
fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The Mod Root a publication is confined to: the attributed root, or the
/// Asset's own folder for a standalone call without one.
fn publication_root(mod_root: &Path, destination: &Path) -> PathBuf {
    if mod_root.as_os_str().is_empty() {
        destination.parent().unwrap_or(destination).to_path_buf()
    } else {
        absolute(mod_root)
    }
}

/// The size and FNV-1a hash of a readable, non-empty regular file, without
/// following a link. `None` when it is anything else.
fn asset_fingerprint(path: &Path) -> Option<(u64, u64)> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let mut file = File::open(path).ok()?;
    let (size, hash) = fingerprint(&mut file).ok()?;
    (size != 0).then_some((size, hash))
}

/// A converted Texture's source, pinned before loading so that only that
/// file object can be deleted once its replacement is published.
///
/// Write sharing is denied, so the loaded bytes cannot change underneath it;
/// delete sharing is granted, so readers, renames and the later delete-access
/// open still work.
struct PinnedSource {
    path: PathBuf,
    file: Option<File>,
    identity: FileIdentity,
    bytes: (u64, u64),
}

impl PinnedSource {
    /// Pins an ordinary, single-link, readable source and records its bytes.
    /// A link or a hard-linked source is [`StagingError::Ineligible`].
    fn open(path: &Path) -> Result<Self, StagingError> {
        let io = |context: &'static str| StagingError::io(context, path);
        let file = Open::new(
            Access::READ_DATA | Access::READ_ATTRIBUTES,
            Share::READ | Share::DELETE,
        )
        .open(path)
        .map_err(io("Pin Texture source"))?;
        let facts = FileFacts::of(&file).map_err(io("Inspect"))?;
        if let Some(ineligible) = ineligibility(path, &facts) {
            return Err(ineligible);
        }
        let identity = FileIdentity::of(&file).map_err(io("Identify"))?;
        let bytes = Self::hash(&file);
        match bytes {
            Some(bytes) if facts.is_ordinary_file() && facts.link_count() == 1 => Ok(Self {
                path: path.to_path_buf(),
                file: Some(file),
                identity,
                bytes,
            }),
            _ => Err(StagingError::Invalid(
                "Convertible Texture source is not an ordinary readable file.".to_owned(),
            )),
        }
    }

    /// Hashes the pinned file object, so a replacement at its path cannot
    /// supply the comparison bytes. `None` for an empty or unreadable file.
    fn hash(mut file: &File) -> Option<(u64, u64)> {
        file.seek(SeekFrom::Start(0)).ok()?;
        let (size, hash) = fingerprint(&mut file).ok()?;
        (size != 0).then_some((size, hash))
    }

    /// The identity of the ordinary single-link file `path` names now.
    fn identity_at(path: &Path, access: Access, share: Share) -> Option<(File, FileIdentity)> {
        let file = Open::new(access, share).open(path).ok()?;
        let facts = FileFacts::of(&file).ok()?;
        if !facts.is_ordinary_file() || facts.link_count() != 1 {
            return None;
        }
        let identity = FileIdentity::of(&file).ok()?;
        Some((file, identity))
    }

    /// Whether the original path still names the pinned, unmodified file.
    fn unchanged_at_path(&self) -> bool {
        let Some(file) = &self.file else {
            return false;
        };
        let current = Self::identity_at(
            &self.path,
            Access::READ_ATTRIBUTES,
            Share::READ | Share::WRITE | Share::DELETE,
        );
        current.is_some_and(|(_, identity)| identity == self.identity)
            && Self::hash(file) == Some(self.bytes)
    }

    /// Deletes the pinned file object, never whatever replaced it at its
    /// path, and releases the pin. Returns whether the delete was set.
    fn remove_verified(&mut self) -> bool {
        if !self.unchanged_at_path() {
            return false;
        }
        let Some((deletion, identity)) = Self::identity_at(
            &self.path,
            Access::DELETE | Access::READ_ATTRIBUTES,
            Share::READ | Share::DELETE,
        ) else {
            return false;
        };
        if identity != self.identity || !self.unchanged_at_path() {
            return false;
        }
        if delete_by_handle(&deletion).is_err() {
            return false;
        }
        // Closing both handles lets the disposition take effect.
        drop(deletion);
        self.file = None;
        true
    }
}

/// Executes carried Routed Asset facts through one backend, without reclassification.
pub struct AssetExecutor<'b> {
    backend: &'b mut dyn AssetExecutionBackend,
}

impl<'b> AssetExecutor<'b> {
    /// Borrows the backend for every attempt this executor makes.
    pub fn new(backend: &'b mut dyn AssetExecutionBackend) -> Self {
        Self { backend }
    }

    /// Executes one attempt under the run's Temporary Ownership.
    ///
    /// `mod_root` is the canonical Mod Root the attempt is attributed to; the
    /// run freezes it before any source can change, and every publication is
    /// confined to it. Only Apply stages anything in `artifacts`.
    pub fn execute(
        &mut self,
        asset: &RoutedAsset,
        artifacts: &mut TemporaryArtifactRegistry,
        mod_root: &Path,
    ) -> AssetExecutionResult {
        match asset.target() {
            OptimizerTarget::Texture => self.execute_texture(asset, artifacts, mod_root),
            OptimizerTarget::Mesh => self.execute_mesh(asset, artifacts, mod_root),
            OptimizerTarget::Animation => self.execute_animation(asset, artifacts, mod_root),
            OptimizerTarget::Archive => AssetExecutionResult::failed(
                AssetExecutionFailure::UnsupportedTarget,
                "Archive extraction and packing are owned by run orchestration.",
            ),
        }
    }

    /// Loads and optimizes one Texture and, in Apply, publishes its changed
    /// output. A conversion also removes its verified source afterwards.
    fn execute_texture(
        &mut self,
        asset: &RoutedAsset,
        artifacts: &mut TemporaryArtifactRegistry,
        mod_root: &Path,
    ) -> AssetExecutionResult {
        let path = asset.execution_path();
        let AssetIdentity::Texture(variant) = asset.identity() else {
            return AssetExecutionResult::failed(
                AssetExecutionFailure::IdentityMismatch,
                "Texture target does not carry a Texture identity.",
            )
            .with_safe_to_continue(false)
            .with_path(path);
        };
        let apply = asset.execution_mode() == ExecutionMode::Apply;
        let converting = variant == TextureVariant::Convertible
            && asset.operations().contains(AssetOperation::Conversion);
        // A Convertible Texture's output is a different file from its source,
        // so publishing it could replace a file this attempt never read.
        let separate_output = variant == TextureVariant::Convertible;
        let output = absolute(&if separate_output {
            path.with_extension("dds")
        } else {
            path.to_path_buf()
        });

        // A removal may fail after changing either path, so the source's
        // opened identity and the saved output's bytes are both pinned down
        // before anything is published.
        let mut source_pin = None;
        let mut target = None;
        if apply {
            if converting {
                match PinnedSource::open(path) {
                    Ok(pin) => source_pin = Some(pin),
                    Err(error) => {
                        return capture_failed("Texture", "pin_texture_source", path, &error);
                    }
                }
            }
            let root = publication_root(mod_root, &output);
            match artifacts.capture_publication_target(&root, &output) {
                // Before the backend loads anything: the source pin, if one
                // was taken, is dropped on the way out.
                Ok(captured) if separate_output && captured.is_occupied() => {
                    return conversion_withheld(path, &output);
                }
                Ok(captured) => target = Some(captured),
                Err(error) => {
                    return capture_failed("Texture", "capture_texture_destination", path, &error);
                }
            }
        } else {
            // Dry Run reports what Apply would do, in the order Apply's pin
            // and capture would find it.
            if converting && let Some(error) = probe_ineligibility(path) {
                return capture_failed("Texture", "pin_texture_source", path, &error);
            }
            if let Some(error) = probe_ineligibility(&output) {
                return capture_failed("Texture", "capture_texture_destination", path, &error);
            }
            if separate_output && file_stands_at(&output) {
                return conversion_withheld(path, &output);
            }
        }

        if let Err(failure) = self.backend.load_texture(path, variant) {
            return load_failed("Texture", "load_texture", path, failure);
        }
        let operation = self
            .backend
            .optimize_texture(asset.operations(), asset.execution_mode());
        if !operation.succeeded() {
            return AssetExecutionResult::failed(
                AssetExecutionFailure::OperationFailed,
                "Failed to optimize Texture.",
            )
            .with_path(path)
            .with_operation("optimize_texture")
            .with_service_detail(operation.message());
        }
        let Some(target) = target.filter(|_| operation.would_change()) else {
            return AssetExecutionResult::success(CompletedMutation::None);
        };

        let receipt = match artifacts.stage_file_for_publication(target) {
            Ok(receipt) => receipt,
            Err(error) => {
                return staging_failed(
                    "Texture",
                    "stage_texture",
                    &output,
                    MutationState::None,
                    &error,
                );
            }
        };
        let staged = receipt
            .path()
            .expect("the registry outlives this attempt")
            .to_path_buf();
        if let Err(detail) = self.backend.save_texture(&staged) {
            abandon(receipt, &staged);
            return AssetExecutionResult::failed(
                AssetExecutionFailure::SaveFailed,
                "Failed to save Texture.",
            )
            .with_path(&output)
            .with_operation("save_texture")
            .with_service_detail(detail);
        }
        let Some(output_before) = asset_fingerprint(&staged) else {
            abandon(receipt, &staged);
            return AssetExecutionResult::failed(
                AssetExecutionFailure::SaveFailed,
                "Saved Texture is not a usable regular file.",
            )
            .with_path(&output)
            .with_operation("save_texture");
        };

        // A separate output was free when it was captured, and `NoReplace` has
        // the rename itself refuse a file that appeared there since; an
        // in-place output replaces the original it was made from.
        let policy = if separate_output {
            PublicationPolicy::NoReplace
        } else {
            PublicationPolicy::Replace
        };
        // Publication commits the destination before releasing ownership, and
        // its result carries that fact even when the release fails.
        let publication = receipt.publish(&output, policy);
        let mutation = publication.mutation();
        if let Some(failure) = publication_failure(
            &publication,
            "Failed to publish Texture output.",
            &output,
            "commit_texture",
        ) {
            return failure;
        }
        // Past `publication_failure` the publication was released, so the
        // output is a Committed Mutation; `mutation` is that same fact, kept
        // as a three-state value for the source-removal failures below.
        if !converting {
            return AssetExecutionResult::success(CompletedMutation::Committed);
        }

        let retained_usable = |pin: &Option<PinnedSource>| {
            asset_fingerprint(&output) == Some(output_before)
                && pin.as_ref().is_some_and(PinnedSource::unchanged_at_path)
        };
        let removal_failed = |message: &str, mutation, safe| {
            AssetExecutionResult::failed(AssetExecutionFailure::SourceRemovalFailed, message)
                .with_mutation(mutation)
                .with_safe_to_continue(safe)
                .with_path(path)
                .with_operation("remove_texture_source")
        };
        if !retained_usable(&source_pin) {
            return removal_failed(
                "Cannot verify conversion files before removal.",
                mutation,
                false,
            );
        }
        let mut attempted = false;
        let mut removed = false;
        // A backend's success alone never authorizes deleting by pathname or
        // claiming the removal: only the pinned identity can be removed, once.
        let reported = self.backend.remove_texture(path, &mut || {
            if attempted {
                return false;
            }
            attempted = true;
            removed = source_pin
                .as_mut()
                .is_some_and(PinnedSource::remove_verified);
            removed
        });
        if reported.is_err() || !removed {
            let usable = retained_usable(&source_pin);
            let mutation = if usable {
                mutation
            } else {
                MutationState::PartialOrUnknown
            };
            // A backend that reported success without removing anything has
            // no detail to give.
            return removal_failed(
                "Failed to remove converted Texture source.",
                mutation,
                usable,
            )
            .with_service_detail(reported.err().unwrap_or_default());
        }
        AssetExecutionResult::success(CompletedMutation::Committed)
    }

    /// Loads a Mesh, runs its independent operations, and in Apply publishes
    /// the result once.
    fn execute_mesh(
        &mut self,
        asset: &RoutedAsset,
        artifacts: &mut TemporaryArtifactRegistry,
        mod_root: &Path,
    ) -> AssetExecutionResult {
        let path = asset.execution_path();
        let AssetIdentity::Mesh(variant) = asset.identity() else {
            return AssetExecutionResult::failed(
                AssetExecutionFailure::IdentityMismatch,
                "Mesh target does not carry a Mesh identity.",
            )
            .with_safe_to_continue(false)
            .with_path(path);
        };
        let destination = absolute(path);
        let mut target = None;
        if asset.execution_mode() == ExecutionMode::Apply {
            let root = publication_root(mod_root, &destination);
            match artifacts.capture_publication_target(&root, &destination) {
                Ok(captured) => target = Some(captured),
                Err(error) => {
                    return capture_failed("Mesh", "capture_mesh_destination", path, &error);
                }
            }
        } else if let Some(error) = probe_ineligibility(&destination) {
            // Dry Run reports what Apply's capture would refuse.
            return capture_failed("Mesh", "capture_mesh_destination", path, &error);
        }
        if let Err(failure) = self.backend.load_mesh(path, variant) {
            return load_failed("Mesh", "load_mesh", path, failure);
        }
        let mut would_change = false;
        if asset.operations().contains(AssetOperation::Optimization) {
            let optimization = self
                .backend
                .optimize_mesh(path, mod_root, asset.execution_mode());
            if !optimization.succeeded() {
                return AssetExecutionResult::failed(
                    AssetExecutionFailure::OperationFailed,
                    "Failed to optimize Mesh.",
                )
                .with_path(path)
                .with_operation("optimize_mesh")
                .with_service_detail(optimization.message());
            }
            would_change = optimization.would_change();
        }
        if asset
            .operations()
            .contains(AssetOperation::MeshReferenceMaintenance)
        {
            let maintenance = self
                .backend
                .maintain_mesh_references(asset.execution_mode());
            if !maintenance.succeeded() {
                return AssetExecutionResult::failed(
                    AssetExecutionFailure::OperationFailed,
                    "Failed to maintain Mesh references.",
                )
                .with_path(path)
                .with_operation("maintain_mesh_references")
                .with_service_detail(maintenance.message());
            }
            would_change = would_change || maintenance.would_change();
        }
        // Dry Run evaluates both operations against the loaded Mesh but never
        // persists their results; it captured no target.
        let Some(target) = target.filter(|_| would_change) else {
            return AssetExecutionResult::success(CompletedMutation::None);
        };
        let receipt = match artifacts.stage_file_for_publication(target) {
            Ok(receipt) => receipt,
            Err(error) => {
                return staging_failed("Mesh", "stage_mesh", path, MutationState::None, &error);
            }
        };
        let staged = receipt
            .path()
            .expect("the registry outlives this attempt")
            .to_path_buf();
        if let Err(detail) = self.backend.save_mesh(&staged) {
            abandon(receipt, &staged);
            return AssetExecutionResult::failed(
                AssetExecutionFailure::SaveFailed,
                "Failed to save Mesh.",
            )
            .with_path(path)
            .with_operation("save_mesh")
            .with_service_detail(detail);
        }
        if asset_fingerprint(&staged).is_none() {
            abandon(receipt, &staged);
            return AssetExecutionResult::failed(
                AssetExecutionFailure::SaveFailed,
                "Saved Mesh is not a usable regular file.",
            )
            .with_path(path)
            .with_operation("save_mesh");
        }
        // Publication commits the replacement before releasing Temporary
        // Ownership; that fact is kept when the release fails.
        let publication = receipt.publish(&destination, PublicationPolicy::Replace);
        if let Some(failure) = publication_failure(
            &publication,
            "Failed to publish Mesh output.",
            path,
            "commit_mesh",
        ) {
            return failure;
        }
        // Past `publication_failure` the publication was released: a Committed
        // Mutation.
        AssetExecutionResult::success(CompletedMutation::Committed)
    }

    /// Optimizes an Animation. Apply stages its output before the backend
    /// runs, because `hkxcmd` writes straight to the path it is given.
    fn execute_animation(
        &mut self,
        asset: &RoutedAsset,
        artifacts: &mut TemporaryArtifactRegistry,
        mod_root: &Path,
    ) -> AssetExecutionResult {
        let path = asset.execution_path();
        if asset.identity() != AssetIdentity::Animation {
            return AssetExecutionResult::failed(
                AssetExecutionFailure::IdentityMismatch,
                "Animation target does not carry an Animation identity.",
            )
            .with_safe_to_continue(false)
            .with_path(path)
            .with_operation("execute_animation");
        }
        if !asset.operations().contains(AssetOperation::Optimization) {
            return AssetExecutionResult::failed(
                AssetExecutionFailure::OperationFailed,
                "Animation does not carry optimization work.",
            )
            .with_safe_to_continue(false)
            .with_path(path)
            .with_operation("execute_animation");
        }
        let destination = absolute(path);
        let mut receipt = None;
        if asset.execution_mode() == ExecutionMode::Apply {
            let root = publication_root(mod_root, &destination);
            // Captured and staged in two steps: a capture failure precedes
            // staging and is contained, while a staging failure is not.
            let target = match artifacts.capture_publication_target(&root, &destination) {
                Ok(target) => target,
                Err(error) => {
                    return capture_failed(
                        "Animation",
                        "capture_animation_destination",
                        path,
                        &error,
                    );
                }
            };
            match artifacts.stage_file_for_publication(target) {
                Ok(staged) => receipt = Some(staged),
                Err(error) => {
                    return staging_failed(
                        "Animation",
                        "stage_animation",
                        path,
                        MutationState::None,
                        &error,
                    );
                }
            }
        } else if let Some(error) = probe_ineligibility(&destination) {
            // Dry Run reports what Apply's capture would refuse.
            return capture_failed("Animation", "capture_animation_destination", path, &error);
        }
        let staged = receipt.as_ref().map(|receipt| {
            receipt
                .path()
                .expect("the registry outlives this attempt")
                .to_path_buf()
        });
        let operation =
            self.backend
                .optimize_animation(path, staged.as_deref(), asset.execution_mode());
        // Apply stages before the converter runs, so every path below that
        // does not publish abandons the staged file it may have written.
        let (receipt, staged) = match (receipt, staged) {
            (Some(receipt), Some(staged)) => (Some(receipt), staged),
            _ => (None, PathBuf::new()),
        };
        if !operation.succeeded() {
            if let Some(receipt) = receipt {
                abandon(receipt, &staged);
            }
            return AssetExecutionResult::failed(
                AssetExecutionFailure::OperationFailed,
                "Failed to optimize Animation.",
            )
            .with_path(path)
            .with_operation("optimize_animation")
            .with_service_detail(operation.message());
        }
        let Some(receipt) = receipt else {
            // Dry Run staged nothing.
            return AssetExecutionResult::success(CompletedMutation::None);
        };
        if !operation.would_change() {
            abandon(receipt, &staged);
            return AssetExecutionResult::success(CompletedMutation::None);
        }
        if asset_fingerprint(&staged).is_none() {
            abandon(receipt, &staged);
            return AssetExecutionResult::failed(
                AssetExecutionFailure::SaveFailed,
                "Animation output is not a usable regular file.",
            )
            .with_path(path)
            .with_operation("save_animation");
        }
        // The native replacement precedes the ownership release; a release
        // failure still leaves a Committed Mutation, unsafe to continue.
        let publication = receipt.publish(&destination, PublicationPolicy::Replace);
        if let Some(failure) = publication_failure(
            &publication,
            "Failed to publish Animation output.",
            path,
            "commit_animation",
        ) {
            return failure;
        }
        // Past `publication_failure` the publication was released: a Committed
        // Mutation.
        AssetExecutionResult::success(CompletedMutation::Committed)
    }
}

/// Finishes one attempt: logs a failure and, in Apply, quarantines a Texture
/// or Mesh whose content the backend could not decode (`LoadFailed`).
///
/// A `ReadFailed` Asset is left alone: the backend could not read it at all,
/// because another program holds it, access is denied, or decoding ran out of
/// memory, so nothing says the file is corrupt. Renaming it would hide a
/// sound file from the game for a passing condition. It stays packable, which
/// is right for a sound file, so the attempt stays safe to continue.
///
/// Quarantine renames the Asset to `<name>.caobad` (or `.caobad.1`, `.2`, …)
/// with a no-replace rename, so a later run and Archive creation leave it
/// alone, and records the rename as a Committed Mutation. An Asset that
/// cannot be renamed is still packable, so the run must not continue to
/// Archive Finalization: the attempt becomes unsafe. Dry Run only reports the
/// failure.
///
/// A Mesh is quarantined whichever operation routed it, so Texture conversion,
/// which routes every Mesh for Mesh Reference Maintenance, quarantines a Mesh
/// nifly cannot load, as C++ did.
pub fn quarantine_failed_load(
    asset: &RoutedAsset,
    result: AssetExecutionResult,
) -> AssetExecutionResult {
    if result.succeeded() {
        return result;
    }
    let path = asset.execution_path();
    log::error!(
        "Cannot process Routed Asset: {}\n{}",
        path.display(),
        result.message()
    );
    if !result.service_detail().is_empty() {
        log::error!("{}", result.service_detail());
    }
    // Only Textures and Meshes are loaded before they are processed; an
    // Animation never fails with LoadFailed.
    if asset.execution_mode() != ExecutionMode::Apply
        || !matches!(
            asset.target(),
            OptimizerTarget::Texture | OptimizerTarget::Mesh
        )
        || result.failure() != Some(AssetExecutionFailure::LoadFailed)
    {
        return result;
    }
    match quarantine(path) {
        Some(quarantined) => {
            log::error!(
                "{} was renamed to {}",
                path.display(),
                quarantined.display()
            );
            result.with_mutation(MutationState::Committed)
        }
        None => {
            log::error!("Please remove {}", path.display());
            result.with_safe_to_continue(false)
        }
    }
}

/// Renames `path` to the first free `.caobad` name, returning it.
///
/// Each candidate is tried with a no-replace rename of the opened file, so an
/// entry that appears at the candidate is never overwritten and the check
/// cannot race the rename (C++ checked, then renamed).
fn quarantine(path: &Path) -> Option<PathBuf> {
    const ERROR_FILE_EXISTS: i32 = 80;
    const ERROR_ALREADY_EXISTS: i32 = 183;
    let path = absolute(path);
    let file = Open::new(
        Access::DELETE | Access::READ_ATTRIBUTES,
        Share::READ | Share::WRITE | Share::DELETE,
    )
    .open(&path)
    .inspect_err(|error| log::error!("Cannot open {} to quarantine it: {error}", path.display()))
    .ok()?;
    let mut name = path.clone().into_os_string();
    name.push(".caobad");
    let mut candidate = PathBuf::from(name);
    for suffix in 1u64.. {
        match rename_by_handle(&file, &candidate, RenameMode::NoReplace) {
            Ok(()) => return Some(candidate),
            Err(error)
                if matches!(
                    error.raw_os_error(),
                    Some(ERROR_FILE_EXISTS | ERROR_ALREADY_EXISTS)
                ) =>
            {
                let mut name = path.clone().into_os_string();
                name.push(format!(".caobad.{suffix}"));
                candidate = PathBuf::from(name);
            }
            Err(error) => {
                log::error!("Cannot quarantine {}: {error}", path.display());
                return None;
            }
        }
    }
    None
}
