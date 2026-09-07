#pragma once

#include "AssetRouting/AssetRouter.h"
#include "Run/TemporaryArtifactRegistry.h"

#include <filesystem>
#include <optional>
#include <span>
#include <string>
#include <vector>

namespace cao::execution {
/// Closed failure categories reported without rewriting an earlier Routing Decision.
enum class AssetExecutionFailure {
    UnsupportedTarget,
    IdentityMismatch,
    LoadFailed,
    OperationFailed,
    SaveFailed,
    SourceRemovalFailed,
    StagingFailed,
    CommitFailed,
    BackendException,
    CleanupFailed
};

/// Filesystem effects on durable Assets, excluding registry-owned temporary bytes.
enum class MutationState { None, Committed, PartialOrUnknown };

/// Stable boundary categories; adapters never need to parse backend diagnostics.
enum class ExecutionFailureCategory { Backend, Filesystem, Contract };

/// Result of one backend operation against the currently loaded Asset.
class OperationResult final {
   public:
    /// Reports a successful operation that changed, or would change, the loaded Asset.
    [[nodiscard]] static OperationResult changed() noexcept;

    /// Reports a successful operation that found no change to apply.
    [[nodiscard]] static OperationResult unchanged() noexcept;

    /// Reports a failed operation with a caller-presentable diagnostic.
    [[nodiscard]] static OperationResult failed(std::string message);

    /// Reports whether the backend operation completed successfully.
    [[nodiscard]] bool succeeded() const noexcept;

    /// Reports whether Apply mode must persist the loaded Asset after this operation.
    [[nodiscard]] bool wouldChange() const noexcept;

    /// Returns the backend diagnostic, or an empty string after success.
    [[nodiscard]] const std::string& message() const noexcept;

   private:
    OperationResult(bool succeeded, bool wouldChange, std::string message);

    bool _succeeded;
    bool _wouldChange;
    std::string _message;
};

/// Observable outcome of executing one Routed Asset.
class AssetExecutionResult final {
   public:
    /// Reports a completed execution.
    [[nodiscard]] static AssetExecutionResult success(
        MutationState mutation = MutationState::None) noexcept;

    /// Reports a failed execution without altering the Routed Asset supplied by the caller.
    /// message is human-readable; serviceDetail retains optional raw backend or native error text.
    [[nodiscard]] static AssetExecutionResult failed(AssetExecutionFailure failure,
                                                     std::string message,
                                                     MutationState mutation = MutationState::None,
                                                     bool safeToContinue = true,
                                                     std::filesystem::path path = {},
                                                     std::string operation = "execute_asset",
                                                     std::string serviceDetail = {});

    /// Reports whether every carried operation completed successfully.
    [[nodiscard]] bool succeeded() const noexcept;

    /// Returns the stable failure category, or no value after success.
    [[nodiscard]] std::optional<AssetExecutionFailure> failure() const noexcept;

    /// Returns the execution diagnostic, or an empty string after success.
    [[nodiscard]] const std::string& message() const noexcept;

    /// Borrows backend-specific diagnostic text, kept separate from the human-readable message.
    [[nodiscard]] const std::string& serviceDetail() const noexcept { return _serviceDetail; }

    /// Returns the exact durable mutation known at the failed or completed boundary.
    [[nodiscard]] MutationState mutationState() const noexcept { return _mutation; }
    /// Partial/unknown mutation is always unsafe, regardless of the supplied safety decision.
    [[nodiscard]] bool safeToContinue() const noexcept { return _safeToContinue; }
    /// Borrows the failing boundary's affected source or destination path.
    [[nodiscard]] const std::filesystem::path& affectedPath() const noexcept { return _path; }
    /// Borrows a stable operation name, separate from backend-specific diagnostic text.
    [[nodiscard]] const std::string& operation() const noexcept { return _operation; }
    /// Identifies Safety Cleanup for cleanup failures and Processing Assets for optimizer results.
    [[nodiscard]] run::RunPhase phase() const noexcept {
        return _failure == AssetExecutionFailure::CleanupFailed ? run::RunPhase::SafetyCleanup
                                                                : run::RunPhase::ProcessingAssets;
    }
    /// Returns a stable category for a failed operation, or no value after success.
    [[nodiscard]] std::optional<ExecutionFailureCategory> failureCategory() const noexcept;
    /// Borrows all cleanup failures without replacing the primary operation failure.
    [[nodiscard]] std::span<const run::RunFailure> cleanupFailures() const noexcept {
        return _cleanupFailures;
    }

   private:
    friend class AssetExecutor;
    AssetExecutionResult(std::optional<AssetExecutionFailure> failure, std::string message);

    std::optional<AssetExecutionFailure> _failure;
    std::string _message;
    MutationState _mutation{MutationState::None};
    bool _safeToContinue{true};
    std::filesystem::path _path;
    std::string _operation;
    std::string _serviceDetail;
    std::vector<run::RunFailure> _cleanupFailures;
};

/// Stateful adapter used internally by Asset Executor to operate on loaded optimizer data.
class AssetExecutionBackend {
   public:
    virtual ~AssetExecutionBackend() = default;

    /// Loads one Texture according to its carried Variant, without classifying its path.
    /// May read the source and change loaded state, but must not mutate durable files.
    virtual bool loadTexture(const std::filesystem::path& path,
                             routing::TextureVariant variant) = 0;

    /// Applies or evaluates the closed Texture operation set against the loaded Texture.
    /// May change only loaded state; persistence is exclusively performed through saveTexture.
    virtual OperationResult optimizeTexture(const routing::AssetOperations& operations,
                                            routing::ExecutionMode mode) = 0;

    /// Writes the loaded Texture only to the supplied registered staging path.
    /// Closes every output handle before returning so the executor can commit the staged file.
    virtual bool saveTexture(const std::filesystem::path& path) = 0;

    /// Removes a converted source Texture after its replacement was saved successfully.
    /// May delete only the supplied source; false reports failure and the executor verifies bytes.
    virtual bool removeTexture(const std::filesystem::path& path) = 0;

    /// Returns service detail from the most recent Texture load, save, or removal failure.
    /// Backends without a service diagnostic may return an empty string.
    virtual std::string textureFailureDetail() const { return {}; }

    /// Loads one Mesh according to its carried Variant, without classifying its path.
    virtual bool loadMesh(const std::filesystem::path& path, routing::MeshVariant variant) = 0;

    /// Applies or evaluates ordinary optimization against the currently loaded Mesh.
    virtual OperationResult optimizeMesh(const std::filesystem::path& path,
                                         routing::ExecutionMode mode) = 0;

    /// Applies or evaluates Mesh Reference Maintenance against the currently loaded Mesh.
    virtual OperationResult maintainMeshReferences(routing::ExecutionMode mode) = 0;

    /// Persists the currently loaded Mesh once after all carried operations complete.
    virtual bool saveMesh(const std::filesystem::path& path) = 0;

    /// Applies or evaluates the carried Animation optimization operation.
    virtual OperationResult optimizeAnimation(const std::filesystem::path& path,
                                              routing::ExecutionMode mode) = 0;
};

/// Executes carried Routed Asset facts through one optimizer adapter without reclassification.
class AssetExecutor final {
   public:
    /// Uses the supplied stateful adapter for all execution; the adapter must outlive this
    /// executor.
    explicit AssetExecutor(AssetExecutionBackend& backend) noexcept;

    /// Executes one attempt and cleans its temporary artifacts before returning to legacy callers.
    /// Supply the selected Mod Root; an omitted root treats the Asset's parent as a standalone mod.
    [[nodiscard]] AssetExecutionResult execute(const routing::RoutedAsset& asset,
                                               const std::filesystem::path& modRoot = {}) const;

    /// Executes using the run's registry, which must outlive the attempt and receive Safety Cleanup.
    /// Texture saves durably register same-volume staging before creation and commit before removal.
    /// Supply the selected Mod Root, or omit it for a standalone Asset in its parent directory.
    [[nodiscard]] AssetExecutionResult execute(const routing::RoutedAsset& asset,
                                               run::TemporaryArtifactRegistry& artifacts,
                                               const std::filesystem::path& modRoot = {}) const;

   private:
    /// Executes one carried Texture transaction, including conversion output replacement in Apply
    /// mode.
    [[nodiscard]] AssetExecutionResult executeTexture(const routing::RoutedAsset& asset,
                                                      run::TemporaryArtifactRegistry& artifacts,
                                                      const std::filesystem::path& modRoot) const;

    /// Executes independent Mesh operations through one load and at most one Apply-mode save.
    [[nodiscard]] AssetExecutionResult executeMesh(const routing::RoutedAsset& asset) const;

    /// Executes the carried Animation operation or reports a backend failure.
    [[nodiscard]] AssetExecutionResult executeAnimation(const routing::RoutedAsset& asset) const;

    AssetExecutionBackend& _backend;
};
}  // namespace cao::execution
