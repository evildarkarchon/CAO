#include "AssetExecution/AssetExecutor.h"

#include <exception>
#include <array>
#include <cstring>
#include <fstream>
#include <memory>
#include <system_error>
#include <utility>

#ifdef _WIN32
#include <Windows.h>
#endif

namespace cao::execution {
namespace {
/// Captures readable bytes without retaining a potentially large Asset in memory.
/// Removal may continue after failure only when the loaded source and saved output still match.
std::optional<std::pair<std::uint64_t, std::uint64_t>> assetFingerprint(
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

#ifdef _WIN32
struct SourceIdentity {
    std::uint64_t volume{};
    std::array<std::byte, 16> file{};
    bool fullFileId{};
    bool operator==(const SourceIdentity&) const = default;
};

/// Reads a regular, unlinked file's stable Win32 identity without following a reparse point.
std::optional<SourceIdentity> sourceIdentity(HANDLE handle) noexcept {
    BY_HANDLE_FILE_INFORMATION info{};
    if (!GetFileInformationByHandle(handle, &info) ||
        (info.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)) != 0 ||
        info.nNumberOfLinks != 1)
        return std::nullopt;
    SourceIdentity identity;
    FILE_ID_INFO fileId{};
    if (GetFileInformationByHandleEx(handle, FileIdInfo, &fileId, sizeof(fileId))) {
        identity.volume = fileId.VolumeSerialNumber;
        std::memcpy(identity.file.data(), fileId.FileId.Identifier, identity.file.size());
        identity.fullFileId = true;
    } else {
        // Older file systems can lack FileIdInfo; match the registry's 64-bit fallback.
        identity.volume = info.dwVolumeSerialNumber;
        const auto index = (static_cast<std::uint64_t>(info.nFileIndexHigh) << 32) |
                           info.nFileIndexLow;
        std::memcpy(identity.file.data(), &index, sizeof(index));
    }
    return identity;
}

/// Pins the source read before loading and deletes only that opened file after publication.
/// Delete sharing lets readers and renames proceed; denied write sharing protects loaded bytes.
/// Delete access is acquired later so backends that do not share it can still read the source.
class PinnedConvertibleSource final {
   public:
    /// Opens an ordinary source for stable read access and records its original readable bytes.
    explicit PinnedConvertibleSource(const std::filesystem::path& path) : _path(path) {
        _handle = CreateFileW(path.c_str(), FILE_READ_DATA | FILE_READ_ATTRIBUTES,
                              FILE_SHARE_READ | FILE_SHARE_DELETE, nullptr,
                              OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT, nullptr);
        if (_handle == INVALID_HANDLE_VALUE)
            throw std::system_error(static_cast<int>(GetLastError()), std::system_category());
        _identity = sourceIdentity(_handle);
        _bytes = fingerprint();
        if (!_identity || !_bytes) {
            close();
            throw std::invalid_argument("Convertible Texture source is not an ordinary readable file");
        }
    }
    PinnedConvertibleSource(const PinnedConvertibleSource&) = delete;
    PinnedConvertibleSource& operator=(const PinnedConvertibleSource&) = delete;
    /// Releases the source handle without deleting when publication or removal did not complete.
    ~PinnedConvertibleSource() { close(); }

    /// Reports whether the original path still names the opened, unmodified source file.
    [[nodiscard]] bool unchangedAtPath() const noexcept {
        if (_handle == INVALID_HANDLE_VALUE) return false;
        const auto current = CreateFileW(_path.c_str(), FILE_READ_ATTRIBUTES,
                                         FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                                         nullptr, OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT,
                                         nullptr);
        if (current == INVALID_HANDLE_VALUE) return false;
        const auto identity = sourceIdentity(current);
        CloseHandle(current);
        return identity && identity == _identity && fingerprint() == _bytes;
    }

    /// Opens delete authority for the same verified file and closes both handles after disposition.
    [[nodiscard]] bool removeVerified() noexcept {
        if (!unchangedAtPath()) return false;
        const auto deletion = CreateFileW(_path.c_str(), DELETE | FILE_READ_ATTRIBUTES,
                                          FILE_SHARE_READ | FILE_SHARE_DELETE, nullptr,
                                          OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT, nullptr);
        if (deletion == INVALID_HANDLE_VALUE) return false;
        if (sourceIdentity(deletion) != _identity || !unchangedAtPath()) {
            CloseHandle(deletion);
            return false;
        }
        FILE_DISPOSITION_INFO disposition{};
        disposition.DeleteFile = TRUE;
        if (!SetFileInformationByHandle(deletion, FileDispositionInfo, &disposition,
                                        sizeof(disposition))) {
            CloseHandle(deletion);
            return false;
        }
        CloseHandle(deletion);
        close();
        return true;
    }

   private:
    /// Hashes the pinned file object so a pathname replacement cannot supply the comparison bytes.
    [[nodiscard]] std::optional<std::pair<std::uint64_t, std::uint64_t>> fingerprint() const noexcept {
        LARGE_INTEGER start{};
        if (_handle == INVALID_HANDLE_VALUE ||
            !SetFilePointerEx(_handle, start, nullptr, FILE_BEGIN))
            return std::nullopt;
        std::array<char, 8192> buffer{};
        std::uint64_t hash = 14695981039346656037ULL;
        std::uint64_t size = 0;
        DWORD count = 0;
        while (ReadFile(_handle, buffer.data(), static_cast<DWORD>(buffer.size()), &count,
                        nullptr)) {
            if (count == 0) {
                if (size == 0) return std::nullopt;
                return std::pair{size, hash};
            }
            for (DWORD index = 0; index < count; ++index) {
                hash ^= static_cast<unsigned char>(buffer[index]);
                hash *= 1099511628211ULL;
            }
            size += count;
        }
        return std::nullopt;
    }

    /// Closes a live source handle once, including after disposition has marked it for deletion.
    void close() noexcept {
        if (_handle != INVALID_HANDLE_VALUE) CloseHandle(_handle);
        _handle = INVALID_HANDLE_VALUE;
    }

    std::filesystem::path _path;
    HANDLE _handle{INVALID_HANDLE_VALUE};
    std::optional<SourceIdentity> _identity;
    std::optional<std::pair<std::uint64_t, std::uint64_t>> _bytes;
};
#endif

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
        result._operation =
            asset.target() == routing::OptimizerTarget::Mesh        ? "cleanup_mesh_staging"
            : asset.target() == routing::OptimizerTarget::Animation ? "cleanup_animation_staging"
                                                                    : "cleanup_texture_staging";
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
                return executeMesh(asset, artifacts, modRoot);
            case routing::OptimizerTarget::Animation:
                return executeAnimation(asset, artifacts, modRoot);
            case routing::OptimizerTarget::Archive:
                return AssetExecutionResult::failed(
                    AssetExecutionFailure::UnsupportedTarget,
                    "Archive extraction and packing are owned by run orchestration.");
        }
    } catch (const std::exception& error) {
        // If staged execution recovery itself throws, its mutation boundary is no longer
        // trustworthy.
        if (asset.target() == routing::OptimizerTarget::Texture ||
            asset.target() == routing::OptimizerTarget::Mesh ||
            asset.target() == routing::OptimizerTarget::Animation)
            return AssetExecutionResult::failed(
                AssetExecutionFailure::BackendException,
                "Asset execution could not recover from an exception.",
                MutationState::PartialOrUnknown, false, asset.executionPath(),
                asset.target() == routing::OptimizerTarget::Mesh        ? "execute_mesh"
                : asset.target() == routing::OptimizerTarget::Animation ? "execute_animation"
                                                                        : "execute_texture",
                error.what());
        return AssetExecutionResult::failed(AssetExecutionFailure::OperationFailed, error.what());
    } catch (...) {
        if (asset.target() == routing::OptimizerTarget::Texture ||
            asset.target() == routing::OptimizerTarget::Mesh ||
            asset.target() == routing::OptimizerTarget::Animation)
            return AssetExecutionResult::failed(
                AssetExecutionFailure::BackendException,
                "Asset execution could not recover from an unknown exception.",
                MutationState::PartialOrUnknown, false, asset.executionPath(),
                asset.target() == routing::OptimizerTarget::Mesh        ? "execute_mesh"
                : asset.target() == routing::OptimizerTarget::Animation ? "execute_animation"
                                                                        : "execute_texture");
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
    decltype(assetFingerprint(outputPath)) sourceBefore, outputBefore;
    std::optional<run::TemporaryArtifactRegistry::PublicationTarget> target;
#ifdef _WIN32
    std::unique_ptr<PinnedConvertibleSource> sourcePin;
#endif
    // A removal backend may fail after changing either path. The source's opened identity and
    // saved output bytes must both survive before such a failure can be considered recoverable.
    const auto retainedFilesUsable = [&] {
        if (!outputBefore || outputBefore != assetFingerprint(outputPath)) return false;
#ifdef _WIN32
        return sourcePin && sourcePin->unchangedAtPath();
#else
        return sourceBefore && sourceBefore == assetFingerprint(asset.executionPath());
#endif
    };
    try {
        if (asset.executionMode() == routing::ExecutionMode::Apply) {
            if (asset.operations().contains(routing::AssetOperation::Conversion) &&
                texture->variant() == routing::TextureVariant::Convertible) {
                boundary = "pin_texture_source";
#ifdef _WIN32
                sourcePin = std::make_unique<PinnedConvertibleSource>(asset.executionPath());
#else
                sourceBefore = assetFingerprint(asset.executionPath());
#endif
            }
            boundary = "capture_texture_destination";
            const auto absoluteOutput = std::filesystem::absolute(outputPath);
            target.emplace(artifacts.capturePublicationTarget(
                modRoot.empty() ? absoluteOutput.parent_path() : std::filesystem::absolute(modRoot),
                absoluteOutput));
        }
        boundary = "load_texture";
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
        const auto absoluteOutput = std::filesystem::absolute(outputPath);
        auto receipt = artifacts.stageFileForPublication(std::move(*target));
        const auto staged = receipt.path();
        boundary = "save_texture";
        if (!_backend.saveTexture(staged)) {
            return AssetExecutionResult::failed(
                AssetExecutionFailure::SaveFailed, "Failed to save Texture.", mutation, true,
                affectedPath, boundary, _backend.textureFailureDetail());
        }
        outputBefore = assetFingerprint(staged);
        if (!outputBefore)
            return AssetExecutionResult::failed(AssetExecutionFailure::SaveFailed,
                                                "Saved Texture is not a usable regular file.",
                                                mutation, true, affectedPath, boundary);
        boundary = "commit_texture";
        const auto publication = receipt.publish(absoluteOutput, run::PublicationPolicy::Replace);
        if (publication.state != run::PublicationState::PublishedAndReleased) {
            // Publication commits the destination before durable ownership release. Its result
            // carries that fact even when the release fails after the native move.
            const bool published = publication.state == run::PublicationState::PublishedStillOwned;
            mutation = published ? MutationState::Committed : MutationState::None;
            return AssetExecutionResult::failed(AssetExecutionFailure::CommitFailed,
                                                "Failed to publish Texture output.", mutation, !published,
                                                affectedPath, boundary, publication.errorDetail);
        }
        mutation = MutationState::Committed;
        if (asset.operations().contains(routing::AssetOperation::Conversion) &&
            texture->variant() == routing::TextureVariant::Convertible) {
            boundary = "remove_texture_source";
            affectedPath = asset.executionPath();
            if (!retainedFilesUsable())
                return AssetExecutionResult::failed(
                    AssetExecutionFailure::SourceRemovalFailed,
                    "Cannot verify conversion files before removal.", mutation, false, affectedPath,
                    boundary);
            removingSource = true;
            bool removalAttempted = false;
            bool removalSucceeded = false;
            // A backend success alone cannot authorize deleting by pathname or claiming removal.
            const std::function<bool()> removeVerified = [&] {
                if (removalAttempted) return false;
                removalAttempted = true;
#ifdef _WIN32
                removalSucceeded = sourcePin && sourcePin->removeVerified();
#else
                removalSucceeded = std::filesystem::remove(affectedPath);
#endif
                return removalSucceeded;
            };
            const bool reportedRemoved = _backend.removeTexture(affectedPath, removeVerified);
            if (!reportedRemoved || !removalSucceeded) {
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
        const bool stagingFailure = boundary == "stage_texture" ||
                                    boundary == "capture_texture_destination" ||
                                    boundary == "pin_texture_source";
        return AssetExecutionResult::failed(
            stagingFailure ? AssetExecutionFailure::StagingFailed
                           : AssetExecutionFailure::BackendException,
            stagingFailure ? "Failed to prepare Texture staging."
                           : "Texture backend raised a filesystem exception.",
            mutation, stagingFailure, affectedPath, boundary, error.what());
    } catch (const std::exception& error) {
        if (removingSource && !retainedFilesUsable()) mutation = MutationState::PartialOrUnknown;
        return AssetExecutionResult::failed(
            boundary == "stage_texture" || boundary == "capture_texture_destination" ||
                    boundary == "pin_texture_source"
                ? AssetExecutionFailure::StagingFailed
                : AssetExecutionFailure::BackendException,
            boundary == "stage_texture" || boundary == "capture_texture_destination" ||
                    boundary == "pin_texture_source"
                ? "Failed to prepare Texture staging."
                : "Texture backend raised an exception.",
            mutation, false, affectedPath, boundary, error.what());
    } catch (...) {
        if (removingSource && !retainedFilesUsable()) mutation = MutationState::PartialOrUnknown;
        return AssetExecutionResult::failed(AssetExecutionFailure::BackendException,
                                            "Unknown Texture backend exception.", mutation, false,
                                            affectedPath, boundary);
    }
}

AssetExecutionResult AssetExecutor::executeMesh(const routing::RoutedAsset& asset,
                                                run::TemporaryArtifactRegistry& artifacts,
                                                const std::filesystem::path& modRoot) const {
    const auto* mesh = std::get_if<routing::MeshAsset>(&asset.identity());
    if (mesh == nullptr) {
        return AssetExecutionResult::failed(AssetExecutionFailure::IdentityMismatch,
                                            "Mesh target does not carry a Mesh identity.",
                                            MutationState::None, false, asset.executionPath());
    }

    const auto& path = asset.executionPath();
    std::string boundary = "load_mesh";
    auto mutation = MutationState::None;
    try {
        std::optional<run::TemporaryArtifactRegistry::PublicationTarget> target;
        if (asset.executionMode() == routing::ExecutionMode::Apply) {
            boundary = "capture_mesh_destination";
            const auto absolutePath = std::filesystem::absolute(path);
            target.emplace(artifacts.capturePublicationTarget(
                modRoot.empty() ? absolutePath.parent_path() : std::filesystem::absolute(modRoot),
                absolutePath));
        }
        boundary = "load_mesh";
        if (!_backend.loadMesh(path, mesh->variant()))
            return AssetExecutionResult::failed(AssetExecutionFailure::LoadFailed,
                                                "Failed to load Mesh.", mutation, true, path,
                                                boundary);

        bool wouldChange = false;
        if (asset.operations().contains(routing::AssetOperation::Optimization)) {
            boundary = "optimize_mesh";
            const auto optimization = _backend.optimizeMesh(path, asset.executionMode());
            if (!optimization.succeeded())
                return AssetExecutionResult::failed(AssetExecutionFailure::OperationFailed,
                                                    "Failed to optimize Mesh.", mutation, true,
                                                    path, boundary, optimization.message());
            wouldChange = optimization.wouldChange();
        }
        if (asset.operations().contains(routing::AssetOperation::MeshReferenceMaintenance)) {
            boundary = "maintain_mesh_references";
            const auto maintenance = _backend.maintainMeshReferences(asset.executionMode());
            if (!maintenance.succeeded())
                return AssetExecutionResult::failed(AssetExecutionFailure::OperationFailed,
                                                    "Failed to maintain Mesh references.", mutation,
                                                    true, path, boundary, maintenance.message());
            wouldChange = wouldChange || maintenance.wouldChange();
        }

        // Dry Run evaluates both operations against the loaded Mesh but never persists their
        // results.
        if (asset.executionMode() == routing::ExecutionMode::DryRun || !wouldChange)
            return AssetExecutionResult::success();

        boundary = "stage_mesh";
        const auto absolutePath = std::filesystem::absolute(path);
        auto receipt = artifacts.stageFileForPublication(std::move(*target));
        const auto staged = receipt.path();
        boundary = "save_mesh";
        if (!_backend.saveMesh(staged) || !assetFingerprint(staged))
            return AssetExecutionResult::failed(AssetExecutionFailure::SaveFailed,
                                                "Failed to save a usable Mesh staging file.",
                                                mutation, true, path, boundary);
        boundary = "commit_mesh";
        const auto publication = receipt.publish(absolutePath, run::PublicationPolicy::Replace);
        if (publication.state != run::PublicationState::PublishedAndReleased) {
            // Publication commits the replacement before releasing Temporary Ownership. Retain
            // that fact when the release fails so Run Evidence reports the committed Mesh.
            const bool published = publication.state == run::PublicationState::PublishedStillOwned;
            mutation = published ? MutationState::Committed : MutationState::None;
            return AssetExecutionResult::failed(AssetExecutionFailure::CommitFailed,
                                                "Failed to publish Mesh output.", mutation,
                                                !published, path, boundary, publication.errorDetail);
        }
        mutation = MutationState::Committed;
        return AssetExecutionResult::success(mutation);
    } catch (const std::filesystem::filesystem_error& error) {
        const bool stagingFailure = boundary == "stage_mesh" ||
                                    boundary == "capture_mesh_destination";
        return AssetExecutionResult::failed(
            stagingFailure ? AssetExecutionFailure::StagingFailed
                           : AssetExecutionFailure::BackendException,
            stagingFailure ? "Failed to prepare Mesh staging."
                           : "Mesh backend raised a filesystem exception.",
            mutation, stagingFailure, path, boundary, error.what());
    } catch (const std::exception& error) {
        return AssetExecutionResult::failed(
            boundary == "stage_mesh" || boundary == "capture_mesh_destination"
                ? AssetExecutionFailure::StagingFailed
                : AssetExecutionFailure::BackendException,
            "Mesh execution raised an exception.", mutation, false, path, boundary, error.what());
    } catch (...) {
        return AssetExecutionResult::failed(AssetExecutionFailure::BackendException,
                                            "Unknown Mesh backend exception.", mutation, false,
                                            path, boundary);
    }
}

AssetExecutionResult AssetExecutor::executeAnimation(const routing::RoutedAsset& asset,
                                                     run::TemporaryArtifactRegistry& artifacts,
                                                     const std::filesystem::path& modRoot) const {
    const auto& path = asset.executionPath();
    if (!std::holds_alternative<routing::AnimationAsset>(asset.identity())) {
        return AssetExecutionResult::failed(
            AssetExecutionFailure::IdentityMismatch,
            "Animation target does not carry an Animation identity.", MutationState::None, false,
            path, "execute_animation");
    }
    if (!asset.operations().contains(routing::AssetOperation::Optimization)) {
        return AssetExecutionResult::failed(AssetExecutionFailure::OperationFailed,
                                            "Animation does not carry optimization work.",
                                            MutationState::None, false, path, "execute_animation");
    }

    auto mutation = MutationState::None;
    std::string boundary = "optimize_animation";
    try {
        std::optional<run::TemporaryArtifactRegistry::PublicationReceipt> receipt;
        std::filesystem::path destination;
        if (asset.executionMode() == routing::ExecutionMode::Apply) {
            boundary = "stage_animation";
            destination = std::filesystem::absolute(path);
            receipt.emplace(artifacts.stageFileForPublication(
                modRoot.empty() ? destination.parent_path() : std::filesystem::absolute(modRoot),
                destination));
        }
        boundary = "optimize_animation";
        const auto operation = _backend.optimizeAnimation(
            path, receipt ? receipt->path() : std::filesystem::path{}, asset.executionMode());
        if (!operation.succeeded())
            return AssetExecutionResult::failed(AssetExecutionFailure::OperationFailed,
                                                "Failed to optimize Animation.", mutation, true,
                                                path, boundary, operation.message());
        if (!receipt || !operation.wouldChange()) return AssetExecutionResult::success();
        boundary = "save_animation";
        if (!assetFingerprint(receipt->path()))
            return AssetExecutionResult::failed(AssetExecutionFailure::SaveFailed,
                                                "Animation output is not a usable regular file.",
                                                mutation, true, path, boundary);
        boundary = "commit_animation";
        const auto publication = receipt->publish(destination, run::PublicationPolicy::Replace);
        if (publication.state != run::PublicationState::PublishedAndReleased) {
            // The native replacement precedes ownership release; a release failure still leaves a
            // Committed Mutation and is unsafe for this run to continue.
            const bool published = publication.state == run::PublicationState::PublishedStillOwned;
            mutation = published ? MutationState::Committed : MutationState::None;
            return AssetExecutionResult::failed(AssetExecutionFailure::CommitFailed,
                                                "Failed to publish Animation output.", mutation,
                                                !published, path, boundary, publication.errorDetail);
        }
        mutation = MutationState::Committed;
        return AssetExecutionResult::success(mutation);
    } catch (const std::filesystem::filesystem_error& error) {
        const bool stagingFailure = boundary == "stage_animation";
        return AssetExecutionResult::failed(stagingFailure
                                                ? AssetExecutionFailure::StagingFailed
                                                : AssetExecutionFailure::BackendException,
                                            "Animation execution raised a filesystem exception.",
                                            mutation, stagingFailure, path, boundary, error.what());
    } catch (const std::exception& error) {
        return AssetExecutionResult::failed(boundary == "stage_animation"
                                                ? AssetExecutionFailure::StagingFailed
                                                : AssetExecutionFailure::BackendException,
                                            "Animation execution raised an exception.", mutation,
                                            false, path, boundary, error.what());
    } catch (...) {
        return AssetExecutionResult::failed(AssetExecutionFailure::BackendException,
                                            "Unknown Animation backend exception.", mutation, false,
                                            path, boundary);
    }
}
}  // namespace cao::execution
