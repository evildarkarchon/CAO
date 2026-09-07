#include "AssetExecution/AssetExecutor.h"

#include <exception>
#include <utility>

namespace cao::execution {
namespace {
AssetExecutionResult operationFailure(const OperationResult& operation) {
    return AssetExecutionResult::failed(AssetExecutionFailure::OperationFailed,
                                        operation.message());
}
}  // namespace

OperationResult::OperationResult(const bool succeeded, const bool wouldChange, std::string message)
    : _succeeded(succeeded), _wouldChange(wouldChange), _message(std::move(message)) {}

OperationResult OperationResult::changed() noexcept { return OperationResult(true, true, {}); }

OperationResult OperationResult::unchanged() noexcept { return OperationResult(true, false, {}); }

OperationResult OperationResult::failed(std::string message) {
    return OperationResult(false, false, std::move(message));
}

bool OperationResult::succeeded() const noexcept { return _succeeded; }

bool OperationResult::wouldChange() const noexcept { return _wouldChange; }

const std::string& OperationResult::message() const noexcept { return _message; }

AssetExecutionResult::AssetExecutionResult(std::optional<AssetExecutionFailure> failure,
                                           std::string message)
    : _failure(failure), _message(std::move(message)) {}

AssetExecutionResult AssetExecutionResult::success() noexcept {
    return AssetExecutionResult(std::nullopt, {});
}

AssetExecutionResult AssetExecutionResult::failed(const AssetExecutionFailure failure,
                                                  std::string message) {
    return AssetExecutionResult(failure, std::move(message));
}

bool AssetExecutionResult::succeeded() const noexcept { return !_failure.has_value(); }

std::optional<AssetExecutionFailure> AssetExecutionResult::failure() const noexcept {
    return _failure;
}

const std::string& AssetExecutionResult::message() const noexcept { return _message; }

AssetExecutor::AssetExecutor(AssetExecutionBackend& backend) noexcept : _backend(backend) {}

AssetExecutionResult AssetExecutor::execute(const routing::RoutedAsset& asset) const {
    try {
        switch (asset.target()) {
            case routing::OptimizerTarget::Texture:
                return executeTexture(asset);
            case routing::OptimizerTarget::Mesh:
                return executeMesh(asset);
            case routing::OptimizerTarget::Animation:
                return executeAnimation(asset);
            case routing::OptimizerTarget::Archive:
                return AssetExecutionResult::failed(
                    AssetExecutionFailure::UnsupportedTarget,
                    "Archive extraction and packing are owned by run orchestration.");
        }
    } catch (const std::exception& error) {
        return AssetExecutionResult::failed(AssetExecutionFailure::OperationFailed, error.what());
    } catch (...) {
        return AssetExecutionResult::failed(AssetExecutionFailure::OperationFailed,
                                            "Unknown optimizer execution failure.");
    }

    return AssetExecutionResult::failed(AssetExecutionFailure::UnsupportedTarget,
                                        "Unknown optimizer target.");
}

AssetExecutionResult AssetExecutor::executeTexture(const routing::RoutedAsset& asset) const {
    const auto* texture = std::get_if<routing::TextureAsset>(&asset.identity());
    if (texture == nullptr) {
        return AssetExecutionResult::failed(AssetExecutionFailure::IdentityMismatch,
                                            "Texture target does not carry a Texture identity.");
    }

    if (!_backend.loadTexture(asset.executionPath(), texture->variant())) {
        return AssetExecutionResult::failed(AssetExecutionFailure::LoadFailed,
                                            "Failed to load Texture.");
    }

    const bool convert = asset.operations().contains(routing::AssetOperation::Conversion);
    const auto operation = _backend.optimizeTexture(asset.operations(), asset.executionMode());
    if (!operation.succeeded()) return operationFailure(operation);

    if (asset.executionMode() == routing::ExecutionMode::DryRun || !operation.wouldChange())
        return AssetExecutionResult::success();

    auto outputPath = asset.executionPath();
    if (texture->variant() == routing::TextureVariant::Convertible)
        outputPath.replace_extension(".dds");

    if (!_backend.saveTexture(outputPath)) {
        return AssetExecutionResult::failed(AssetExecutionFailure::SaveFailed,
                                            "Failed to save Texture.");
    }
    if (convert && texture->variant() == routing::TextureVariant::Convertible &&
        !_backend.removeTexture(asset.executionPath())) {
        return AssetExecutionResult::failed(AssetExecutionFailure::SourceRemovalFailed,
                                            "Failed to remove converted Texture source.");
    }

    return AssetExecutionResult::success();
}

AssetExecutionResult AssetExecutor::executeMesh(const routing::RoutedAsset& asset) const {
    const auto* mesh = std::get_if<routing::MeshAsset>(&asset.identity());
    if (mesh == nullptr) {
        return AssetExecutionResult::failed(AssetExecutionFailure::IdentityMismatch,
                                            "Mesh target does not carry a Mesh identity.");
    }

    if (!_backend.loadMesh(asset.executionPath(), mesh->variant())) {
        return AssetExecutionResult::failed(AssetExecutionFailure::LoadFailed,
                                            "Failed to load Mesh.");
    }

    bool wouldChange = false;
    if (asset.operations().contains(routing::AssetOperation::Optimization)) {
        const auto optimization =
            _backend.optimizeMesh(asset.executionPath(), asset.executionMode());
        if (!optimization.succeeded()) return operationFailure(optimization);
        wouldChange = wouldChange || optimization.wouldChange();
    }
    if (asset.operations().contains(routing::AssetOperation::MeshReferenceMaintenance)) {
        const auto maintenance = _backend.maintainMeshReferences(asset.executionMode());
        if (!maintenance.succeeded()) return operationFailure(maintenance);
        wouldChange = wouldChange || maintenance.wouldChange();
    }

    // Dry Run evaluates both operations against the loaded Mesh but never persists their results.
    if (asset.executionMode() == routing::ExecutionMode::DryRun || !wouldChange)
        return AssetExecutionResult::success();

    if (!_backend.saveMesh(asset.executionPath())) {
        return AssetExecutionResult::failed(AssetExecutionFailure::SaveFailed,
                                            "Failed to save Mesh.");
    }
    return AssetExecutionResult::success();
}

AssetExecutionResult AssetExecutor::executeAnimation(const routing::RoutedAsset& asset) const {
    if (!std::holds_alternative<routing::AnimationAsset>(asset.identity())) {
        return AssetExecutionResult::failed(
            AssetExecutionFailure::IdentityMismatch,
            "Animation target does not carry an Animation identity.");
    }
    if (!asset.operations().contains(routing::AssetOperation::Optimization)) {
        return AssetExecutionResult::failed(AssetExecutionFailure::OperationFailed,
                                            "Animation does not carry optimization work.");
    }

    const auto operation = _backend.optimizeAnimation(asset.executionPath(), asset.executionMode());
    if (!operation.succeeded()) return operationFailure(operation);
    return AssetExecutionResult::success();
}
}  // namespace cao::execution
