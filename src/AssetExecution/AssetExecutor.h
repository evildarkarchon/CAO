#pragma once

#include "AssetRouting/AssetRouter.h"

#include <filesystem>
#include <optional>
#include <string>

namespace cao::execution
{
/// Closed failure categories reported without rewriting an earlier Routing Decision.
enum class AssetExecutionFailure
{
    UnsupportedTarget,
    IdentityMismatch,
    LoadFailed,
    OperationFailed,
    SaveFailed,
    SourceRemovalFailed
};

/// Result of one backend operation against the currently loaded Asset.
class OperationResult final
{
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
    [[nodiscard]] const std::string &message() const noexcept;

private:
    OperationResult(bool succeeded, bool wouldChange, std::string message);

    bool _succeeded;
    bool _wouldChange;
    std::string _message;
};

/// Observable outcome of executing one Routed Asset.
class AssetExecutionResult final
{
public:
    /// Reports a completed execution.
    [[nodiscard]] static AssetExecutionResult success() noexcept;

    /// Reports a failed execution without altering the Routed Asset supplied by the caller.
    [[nodiscard]] static AssetExecutionResult failed(AssetExecutionFailure failure,
                                                     std::string message);

    /// Reports whether every carried operation completed successfully.
    [[nodiscard]] bool succeeded() const noexcept;

    /// Returns the stable failure category, or no value after success.
    [[nodiscard]] std::optional<AssetExecutionFailure> failure() const noexcept;

    /// Returns the execution diagnostic, or an empty string after success.
    [[nodiscard]] const std::string &message() const noexcept;

private:
    AssetExecutionResult(std::optional<AssetExecutionFailure> failure, std::string message);

    std::optional<AssetExecutionFailure> _failure;
    std::string _message;
};

/// Stateful adapter used internally by Asset Executor to operate on loaded optimizer data.
class AssetExecutionBackend
{
public:
    virtual ~AssetExecutionBackend() = default;

    /// Loads one Texture according to its carried Variant, without classifying its path.
    virtual bool loadTexture(const std::filesystem::path &path,
                             routing::TextureVariant variant) = 0;

    /// Applies or evaluates the closed Texture operation set against the loaded Texture.
    virtual OperationResult optimizeTexture(const routing::AssetOperations &operations,
                                            routing::ExecutionMode mode) = 0;

    /// Persists the currently loaded Texture to the supplied execution output path.
    virtual bool saveTexture(const std::filesystem::path &path) = 0;

    /// Removes a converted source Texture after its replacement was saved successfully.
    virtual bool removeTexture(const std::filesystem::path &path) = 0;

    /// Loads one Mesh according to its carried Variant, without classifying its path.
    virtual bool loadMesh(const std::filesystem::path &path, routing::MeshVariant variant) = 0;

    /// Applies or evaluates ordinary optimization against the currently loaded Mesh.
    virtual OperationResult optimizeMesh(const std::filesystem::path &path,
                                         routing::ExecutionMode mode) = 0;

    /// Applies or evaluates Mesh Reference Maintenance against the currently loaded Mesh.
    virtual OperationResult maintainMeshReferences(routing::ExecutionMode mode) = 0;

    /// Persists the currently loaded Mesh once after all carried operations complete.
    virtual bool saveMesh(const std::filesystem::path &path) = 0;

    /// Applies or evaluates the carried Animation optimization operation.
    virtual OperationResult optimizeAnimation(const std::filesystem::path &path,
                                              routing::ExecutionMode mode) = 0;
};

/// Executes carried Routed Asset facts through one optimizer adapter without reclassification.
class AssetExecutor final
{
public:
    /// Uses the supplied stateful adapter for all execution; the adapter must outlive this executor.
    explicit AssetExecutor(AssetExecutionBackend &backend) noexcept;

    /// Executes the carried target, Variant, operations, path, and mode as one transaction.
    [[nodiscard]] AssetExecutionResult execute(const routing::RoutedAsset &asset) const;

private:
    /// Executes one carried Texture transaction, including conversion output replacement in Apply mode.
    [[nodiscard]] AssetExecutionResult executeTexture(const routing::RoutedAsset &asset) const;

    /// Executes independent Mesh operations through one load and at most one Apply-mode save.
    [[nodiscard]] AssetExecutionResult executeMesh(const routing::RoutedAsset &asset) const;

    /// Executes the carried Animation operation or reports a backend failure.
    [[nodiscard]] AssetExecutionResult executeAnimation(const routing::RoutedAsset &asset) const;

    AssetExecutionBackend &_backend;
};
}
