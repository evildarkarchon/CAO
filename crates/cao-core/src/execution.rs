//! Asset execution: the Asset Execution Backend seam and the Asset Executor.
//!
//! Ported from the C++ `cao::execution` module (`src/AssetExecution`). The
//! executor carries a Routed Asset's facts to one backend without reclassifying
//! it, and reports one Operation Failure shape for every unsuccessful attempt.
//!
//! Apply-mode persistence goes through staged publication under Temporary
//! Ownership, which arrives with #491. Until then an Apply attempt that would
//! change an Asset stops at the staging boundary with an unsafe
//! [`AssetExecutionFailure::StagingFailed`] and mutates nothing; Dry Run is
//! complete.

use std::path::{Path, PathBuf};

use crate::routing::{
    AssetIdentity, AssetOperation, AssetOperations, ExecutionMode, MeshVariant, OptimizerTarget,
    RoutedAsset, TextureVariant,
};
use crate::run::{RunFailure, RunPhase};

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
    LoadFailed,
    OperationFailed,
    SaveFailed,
    SourceRemovalFailed,
    StagingFailed,
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

/// The observable outcome of executing one Routed Asset.
///
/// A failed result is an Operation Failure: it records its mutation state and
/// whether the run can safely continue. Partial or unknown mutation is always
/// unsafe, whatever the producer claimed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetExecutionResult {
    failure: Option<AssetExecutionFailure>,
    message: String,
    mutation: MutationState,
    safe_to_continue: bool,
    path: PathBuf,
    operation: String,
    service_detail: String,
    cleanup_failures: Vec<RunFailure>,
}

impl AssetExecutionResult {
    /// A completed attempt that committed exactly `mutation`.
    pub fn success(mutation: MutationState) -> Self {
        Self {
            failure: None,
            message: String::new(),
            mutation,
            safe_to_continue: mutation != MutationState::PartialOrUnknown,
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
            failure: Some(failure),
            message: message.into(),
            mutation: MutationState::None,
            safe_to_continue: true,
            path: PathBuf::new(),
            operation: "execute_asset".to_owned(),
            service_detail: String::new(),
            cleanup_failures: Vec::new(),
        }
    }

    /// Sets the durable mutation known at the failed boundary.
    pub fn with_mutation(mut self, mutation: MutationState) -> Self {
        self.mutation = mutation;
        self
    }

    /// Sets whether the run may continue; partial mutation still forces `false`.
    pub fn with_safe_to_continue(mut self, safe_to_continue: bool) -> Self {
        self.safe_to_continue = safe_to_continue;
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

    /// Reports whether every carried operation completed.
    pub fn succeeded(&self) -> bool {
        self.failure.is_none()
    }

    /// The stable failure category, or `None` after success.
    pub fn failure(&self) -> Option<AssetExecutionFailure> {
        self.failure
    }

    /// The presentable diagnostic; empty after success.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Backend-specific detail, kept apart from the message.
    pub fn service_detail(&self) -> &str {
        &self.service_detail
    }

    /// The exact durable mutation known at the failed or completed boundary.
    pub fn mutation_state(&self) -> MutationState {
        self.mutation
    }

    /// Whether the run may continue. Partial or unknown mutation is never safe.
    pub fn safe_to_continue(&self) -> bool {
        self.safe_to_continue && self.mutation != MutationState::PartialOrUnknown
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
        if self.failure == Some(AssetExecutionFailure::CleanupFailed) {
            RunPhase::SafetyCleanup
        } else {
            RunPhase::ProcessingAssets
        }
    }

    /// The stable category of a failed attempt, or `None` after success.
    pub fn failure_category(&self) -> Option<ExecutionFailureCategory> {
        use AssetExecutionFailure::*;
        self.failure.map(|failure| match failure {
            UnsupportedTarget | IdentityMismatch | BackendException => {
                ExecutionFailureCategory::Contract
            }
            SaveFailed | SourceRemovalFailed | StagingFailed | CommitFailed | CleanupFailed => {
                ExecutionFailureCategory::Filesystem
            }
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
/// supplies. A panic from any call is contained by the run and becomes an
/// Operation Failure with unknown mutation that stops the run.
pub trait AssetExecutionBackend {
    /// Loads one Texture according to its Variant, without mutating durable files.
    fn load_texture(&mut self, path: &Path, variant: TextureVariant) -> bool;

    /// Applies or evaluates the Texture operations against the loaded Texture.
    fn optimize_texture(
        &mut self,
        operations: AssetOperations,
        mode: ExecutionMode,
    ) -> OperationResult;

    /// Writes the loaded Texture to the supplied staging path, closing every
    /// handle before returning.
    fn save_texture(&mut self, path: &Path) -> bool;

    /// Requests removal of a converted source after its replacement is
    /// committed. The backend must delete through `remove_verified`, which
    /// removes only the pinned source identity; pathname removal is never
    /// authorized.
    fn remove_texture(&mut self, path: &Path, remove_verified: &mut dyn FnMut() -> bool) -> bool;

    /// Service detail from the most recent Texture load, save or removal failure.
    fn texture_failure_detail(&self) -> String {
        String::new()
    }

    /// Loads one Mesh according to its Variant, without mutating durable files.
    fn load_mesh(&mut self, path: &Path, variant: MeshVariant) -> bool;

    /// Applies or evaluates ordinary optimization against the loaded Mesh; `path`
    /// is context, never a destination.
    fn optimize_mesh(&mut self, path: &Path, mode: ExecutionMode) -> OperationResult;

    /// Applies or evaluates Mesh Reference Maintenance against the loaded Mesh.
    fn maintain_mesh_references(&mut self, mode: ExecutionMode) -> OperationResult;

    /// Writes the loaded Mesh to the supplied staging path after all operations.
    fn save_mesh(&mut self, path: &Path) -> bool;

    /// Reads the source Animation and, in Apply, writes changed output only to
    /// `output_path`. Dry Run receives no output path and must not write.
    fn optimize_animation(
        &mut self,
        path: &Path,
        output_path: Option<&Path>,
        mode: ExecutionMode,
    ) -> OperationResult;
}

/// The failure every Apply attempt that must persist reports until staged
/// publication is ported (#491). It mutates nothing, but persisting outside
/// Temporary Ownership would be unsafe, so the run stops.
fn staging_unavailable(asset: &RoutedAsset, path: &Path, operation: &str) -> AssetExecutionResult {
    log::warn!(
        "Apply output for {} needs staged publication, which is not ported yet",
        asset.execution_path().display()
    );
    AssetExecutionResult::failed(
        AssetExecutionFailure::StagingFailed,
        "Staged publication is not available in this build.",
    )
    .with_safe_to_continue(false)
    .with_path(path)
    .with_operation(operation)
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

    /// Executes one attempt. `mod_root` is the canonical Mod Root the attempt is
    /// attributed to; the run freezes it before any source can change.
    pub fn execute(&mut self, asset: &RoutedAsset, mod_root: &Path) -> AssetExecutionResult {
        let _ = mod_root; // Scopes publication targets once staging lands (#491).
        match asset.target() {
            OptimizerTarget::Texture => self.execute_texture(asset),
            OptimizerTarget::Mesh => self.execute_mesh(asset),
            OptimizerTarget::Animation => self.execute_animation(asset),
            OptimizerTarget::Archive => AssetExecutionResult::failed(
                AssetExecutionFailure::UnsupportedTarget,
                "Archive extraction and packing are owned by run orchestration.",
            ),
        }
    }

    /// Loads, optimizes and, in Apply, would stage one Texture.
    fn execute_texture(&mut self, asset: &RoutedAsset) -> AssetExecutionResult {
        let path = asset.execution_path();
        let AssetIdentity::Texture(variant) = asset.identity() else {
            return AssetExecutionResult::failed(
                AssetExecutionFailure::IdentityMismatch,
                "Texture target does not carry a Texture identity.",
            )
            .with_safe_to_continue(false)
            .with_path(path);
        };
        if !self.backend.load_texture(path, variant) {
            return AssetExecutionResult::failed(
                AssetExecutionFailure::LoadFailed,
                "Failed to load Texture.",
            )
            .with_path(path)
            .with_operation("load_texture")
            .with_service_detail(self.backend.texture_failure_detail());
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
        if asset.execution_mode() == ExecutionMode::DryRun || !operation.would_change() {
            return AssetExecutionResult::success(MutationState::None);
        }
        let output = if variant == TextureVariant::Convertible {
            path.with_extension("dds")
        } else {
            path.to_path_buf()
        };
        staging_unavailable(asset, &output, "stage_texture")
    }

    /// Loads a Mesh and runs its independent operations; Apply would stage once.
    fn execute_mesh(&mut self, asset: &RoutedAsset) -> AssetExecutionResult {
        let path = asset.execution_path();
        let AssetIdentity::Mesh(variant) = asset.identity() else {
            return AssetExecutionResult::failed(
                AssetExecutionFailure::IdentityMismatch,
                "Mesh target does not carry a Mesh identity.",
            )
            .with_safe_to_continue(false)
            .with_path(path);
        };
        if !self.backend.load_mesh(path, variant) {
            return AssetExecutionResult::failed(
                AssetExecutionFailure::LoadFailed,
                "Failed to load Mesh.",
            )
            .with_path(path)
            .with_operation("load_mesh");
        }
        let mut would_change = false;
        if asset.operations().contains(AssetOperation::Optimization) {
            let optimization = self.backend.optimize_mesh(path, asset.execution_mode());
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
        // persists their results.
        if asset.execution_mode() == ExecutionMode::DryRun || !would_change {
            return AssetExecutionResult::success(MutationState::None);
        }
        staging_unavailable(asset, path, "stage_mesh")
    }

    /// Evaluates an Animation in Dry Run; Apply needs a staged output path first.
    fn execute_animation(&mut self, asset: &RoutedAsset) -> AssetExecutionResult {
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
        // Apply stages its output before the backend runs, so the staging
        // boundary comes first.
        if asset.execution_mode() == ExecutionMode::Apply {
            return staging_unavailable(asset, path, "stage_animation");
        }
        let operation = self
            .backend
            .optimize_animation(path, None, asset.execution_mode());
        if !operation.succeeded() {
            return AssetExecutionResult::failed(
                AssetExecutionFailure::OperationFailed,
                "Failed to optimize Animation.",
            )
            .with_path(path)
            .with_operation("optimize_animation")
            .with_service_detail(operation.message());
        }
        AssetExecutionResult::success(MutationState::None)
    }
}
