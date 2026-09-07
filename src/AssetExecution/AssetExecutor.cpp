#include "AssetExecution/AssetExecutor.h"

#include <exception>
#include <array>
#include <fstream>
#include <utility>

#ifdef _WIN32
#include <Windows.h>
#endif

namespace cao::execution {
namespace {
/// Captures readable bytes without retaining a potentially large Texture in memory.
/// Removal may continue after failure only when the loaded source and saved output still match.
std::optional<std::pair<std::uint64_t, std::uint64_t>> textureFingerprint(
    const std::filesystem::path& path) {
    std::error_code error;
    if (!std::filesystem::is_regular_file(std::filesystem::symlink_status(path, error)) || error)
        return std::nullopt;
    std::ifstream input(path, std::ios::binary);
    if (!input) return std::nullopt;
    std::array<char, 8192> bytes{};
    std::uint64_t hash = 14695981039346656037ULL;
    std::uint64_t size = 0;
    while (input.read(bytes.data(), bytes.size()) || input.gcount() > 0) {
        for (std::streamsize index = 0; index < input.gcount(); ++index) {
            hash ^= static_cast<unsigned char>(bytes[static_cast<std::size_t>(index)]);
            hash *= 1099511628211ULL;
        }
        size += static_cast<std::uint64_t>(input.gcount());
    }
    if (!input.eof() || input.bad() || size == 0) return std::nullopt;
    return std::pair{size, hash};
}

/// Flushes staged bytes and replaces a same-volume destination without a cross-volume copy fallback.
std::error_code commitTexture(const std::filesystem::path& staged,
                              const std::filesystem::path& destination) {
#ifdef _WIN32
    const auto file = CreateFileW(staged.c_str(), GENERIC_WRITE, FILE_SHARE_READ, nullptr,
                                  OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT, nullptr);
    if (file == INVALID_HANDLE_VALUE)
        return {static_cast<int>(GetLastError()), std::system_category()};
    // Durable ownership must never get ahead of a destination whose bytes are still buffered.
    const bool flushed = FlushFileBuffers(file) != 0;
    const auto error = GetLastError();
    CloseHandle(file);
    if (!flushed) return {static_cast<int>(error), std::system_category()};
    if (!MoveFileExW(staged.c_str(), destination.c_str(),
                     MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH))
        return {static_cast<int>(GetLastError()), std::system_category()};
    return {};
#else
    std::error_code error;
    std::filesystem::rename(staged, destination, error);
    return error;
#endif
}

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

AssetExecutionResult AssetExecutionResult::success(const MutationState mutation) noexcept {
    auto result = AssetExecutionResult(std::nullopt, {});
    result._mutation = mutation;
    result._safeToContinue = mutation != MutationState::PartialOrUnknown;
    return result;
}

AssetExecutionResult AssetExecutionResult::failed(const AssetExecutionFailure failure,
                                                  std::string message, const MutationState mutation,
                                                  const bool safeToContinue,
                                                  std::filesystem::path path, std::string operation,
                                                  std::string serviceDetail) {
    auto result = AssetExecutionResult(failure, std::move(message));
    result._mutation = mutation;
    result._safeToContinue = safeToContinue && mutation != MutationState::PartialOrUnknown;
    result._path = std::move(path);
    result._operation = std::move(operation);
    result._serviceDetail = std::move(serviceDetail);
    return result;
}

std::optional<ExecutionFailureCategory> AssetExecutionResult::failureCategory() const noexcept {
    if (!_failure) return std::nullopt;
    switch (*_failure) {
        case AssetExecutionFailure::UnsupportedTarget:
        case AssetExecutionFailure::IdentityMismatch:
        case AssetExecutionFailure::BackendException:
            return ExecutionFailureCategory::Contract;
        case AssetExecutionFailure::SaveFailed:
        case AssetExecutionFailure::SourceRemovalFailed:
        case AssetExecutionFailure::StagingFailed:
        case AssetExecutionFailure::CommitFailed:
        case AssetExecutionFailure::CleanupFailed:
            return ExecutionFailureCategory::Filesystem;
        default:
            return ExecutionFailureCategory::Backend;
    }
}

bool AssetExecutionResult::succeeded() const noexcept { return !_failure.has_value(); }

std::optional<AssetExecutionFailure> AssetExecutionResult::failure() const noexcept {
    return _failure;
}

const std::string& AssetExecutionResult::message() const noexcept { return _message; }

AssetExecutor::AssetExecutor(AssetExecutionBackend& backend) noexcept : _backend(backend) {}

AssetExecutionResult AssetExecutor::execute(const routing::RoutedAsset& asset,
                                            const std::filesystem::path& modRoot) const {
    run::TemporaryArtifactRegistry artifacts;
    auto result = execute(asset, artifacts, modRoot);
    result._cleanupFailures = run::collectSafetyCleanupFailures(artifacts);
    for (const auto& failure : result._cleanupFailures) {
        // A cleanup service exception cannot establish that every owned artifact was attempted.
        if (failure.code() == run::RunFailureCode::SafetyCleanupServiceFailed)
            result._safeToContinue = false;
    }
    if (!result._cleanupFailures.empty() && result.succeeded()) {
        result._failure = AssetExecutionFailure::CleanupFailed;
        result._message = result._cleanupFailures.front().detail();
        result._path = result._cleanupFailures.front().path();
        result._operation = "cleanup_texture_staging";
    }
    return result;
}

AssetExecutionResult AssetExecutor::execute(const routing::RoutedAsset& asset,
                                            run::TemporaryArtifactRegistry& artifacts,
                                            const std::filesystem::path& modRoot) const {
    try {
        switch (asset.target()) {
            case routing::OptimizerTarget::Texture:
                return executeTexture(asset, artifacts, modRoot);
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
        // If Texture recovery itself throws, no exact mutation boundary remains trustworthy.
        if (asset.target() == routing::OptimizerTarget::Texture)
            return AssetExecutionResult::failed(
                AssetExecutionFailure::BackendException,
                "Texture execution could not recover from an exception.",
                MutationState::PartialOrUnknown, false, asset.executionPath(), "execute_texture",
                error.what());
        return AssetExecutionResult::failed(AssetExecutionFailure::OperationFailed, error.what());
    } catch (...) {
        if (asset.target() == routing::OptimizerTarget::Texture)
            return AssetExecutionResult::failed(
                AssetExecutionFailure::BackendException,
                "Texture execution could not recover from an unknown exception.",
                MutationState::PartialOrUnknown, false, asset.executionPath(), "execute_texture");
        return AssetExecutionResult::failed(AssetExecutionFailure::OperationFailed,
                                            "Unknown optimizer execution failure.");
    }

    return AssetExecutionResult::failed(AssetExecutionFailure::UnsupportedTarget,
                                        "Unknown optimizer target.");
}

AssetExecutionResult AssetExecutor::executeTexture(const routing::RoutedAsset& asset,
                                                   run::TemporaryArtifactRegistry& artifacts,
                                                   const std::filesystem::path& modRoot) const {
    const auto* texture = std::get_if<routing::TextureAsset>(&asset.identity());
    if (texture == nullptr) {
        return AssetExecutionResult::failed(AssetExecutionFailure::IdentityMismatch,
                                            "Texture target does not carry a Texture identity.",
                                            MutationState::None, false, asset.executionPath());
    }

    auto outputPath = asset.executionPath();
    if (texture->variant() == routing::TextureVariant::Convertible)
        outputPath.replace_extension(".dds");
    auto affectedPath = asset.executionPath();
    std::string boundary = "load_texture";
    auto mutation = MutationState::None;
    bool removingSource = false;
    decltype(textureFingerprint(outputPath)) sourceBefore, outputBefore;
    // A removal backend may fail after changing either path. Retain byte fingerprints so a
    // readable but truncated/replaced file cannot be mistaken for a safely retained original.
    const auto retainedFilesUsable = [&] {
        return sourceBefore && outputBefore &&
               sourceBefore == textureFingerprint(asset.executionPath()) &&
               outputBefore == textureFingerprint(outputPath);
    };
    try {
        if (!_backend.loadTexture(asset.executionPath(), texture->variant())) {
            return AssetExecutionResult::failed(
                AssetExecutionFailure::LoadFailed, "Failed to load Texture.", mutation, true,
                affectedPath, boundary, _backend.textureFailureDetail());
        }
        boundary = "optimize_texture";
        const auto operation = _backend.optimizeTexture(asset.operations(), asset.executionMode());
        if (!operation.succeeded())
            return AssetExecutionResult::failed(AssetExecutionFailure::OperationFailed,
                                                "Failed to optimize Texture.", mutation, true,
                                                affectedPath, boundary, operation.message());
        if (asset.executionMode() == routing::ExecutionMode::DryRun || !operation.wouldChange())
            return AssetExecutionResult::success();

        boundary = "stage_texture";
        affectedPath = outputPath;
        const auto staging = artifacts.stageFile(
            modRoot.empty() ? std::filesystem::absolute(outputPath).parent_path()
                            : std::filesystem::absolute(modRoot),
            std::filesystem::absolute(outputPath));
        const auto& staged = staging.path;
        const auto registration = staging.registration;
        boundary = "save_texture";
        if (!_backend.saveTexture(staged)) {
            return AssetExecutionResult::failed(
                AssetExecutionFailure::SaveFailed, "Failed to save Texture.", mutation, true,
                affectedPath, boundary, _backend.textureFailureDetail());
        }
        outputBefore = textureFingerprint(staged);
        if (!outputBefore)
            return AssetExecutionResult::failed(AssetExecutionFailure::SaveFailed,
                                                "Saved Texture is not a usable regular file.",
                                                mutation, true, affectedPath, boundary);
        boundary = "commit_texture";
        if (const auto error = commitTexture(staged, outputPath)) {
            return AssetExecutionResult::failed(AssetExecutionFailure::CommitFailed,
                                                "Failed to commit Texture output.", mutation, true,
                                                affectedPath, boundary, error.message());
        }
        mutation = MutationState::Committed;
        artifacts.commit(registration);
        if (asset.operations().contains(routing::AssetOperation::Conversion) &&
            texture->variant() == routing::TextureVariant::Convertible) {
            boundary = "remove_texture_source";
            affectedPath = asset.executionPath();
            sourceBefore = textureFingerprint(affectedPath);
            if (!retainedFilesUsable())
                return AssetExecutionResult::failed(
                    AssetExecutionFailure::SourceRemovalFailed,
                    "Cannot verify conversion files before removal.", mutation, false, affectedPath,
                    boundary);
            removingSource = true;
            if (!_backend.removeTexture(affectedPath)) {
                const bool usable = retainedFilesUsable();
                return AssetExecutionResult::failed(
                    AssetExecutionFailure::SourceRemovalFailed,
                    "Failed to remove converted Texture source.",
                    usable ? mutation : MutationState::PartialOrUnknown, usable, affectedPath,
                    boundary, _backend.textureFailureDetail());
            }
        }
        return AssetExecutionResult::success(mutation);
    } catch (const std::filesystem::filesystem_error& error) {
        if (removingSource && !retainedFilesUsable()) mutation = MutationState::PartialOrUnknown;
        const bool stagingFailure = boundary == "stage_texture";
        return AssetExecutionResult::failed(
            stagingFailure ? AssetExecutionFailure::StagingFailed
                           : AssetExecutionFailure::BackendException,
            stagingFailure ? "Failed to prepare Texture staging."
                           : "Texture backend raised a filesystem exception.",
            mutation, stagingFailure, affectedPath, boundary, error.what());
    } catch (const std::exception& error) {
        if (removingSource && !retainedFilesUsable()) mutation = MutationState::PartialOrUnknown;
        return AssetExecutionResult::failed(
            boundary == "stage_texture" ? AssetExecutionFailure::StagingFailed
                                        : AssetExecutionFailure::BackendException,
            boundary == "stage_texture" ? "Failed to prepare Texture staging."
                                        : "Texture backend raised an exception.",
            mutation, false, affectedPath, boundary, error.what());
    } catch (...) {
        if (removingSource && !retainedFilesUsable()) mutation = MutationState::PartialOrUnknown;
        return AssetExecutionResult::failed(AssetExecutionFailure::BackendException,
                                            "Unknown Texture backend exception.", mutation, false,
                                            affectedPath, boundary);
    }
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
