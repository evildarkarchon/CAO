#include "AssetRouter.h"

#include <algorithm>
#include <array>
#include <cstddef>
#include <optional>
#include <string_view>
#include <type_traits>
#include <utility>

namespace
{
using cao::routing::AssetKind;
using cao::routing::AssetVariant;
using cao::routing::MeshVariant;
using cao::routing::ProfileCapability;
using cao::routing::RequestedWork;
using cao::routing::RoutedAsset;
using cao::routing::RoutedAssetReferences;
using cao::routing::TextureVariant;

struct RequestedWorkDefinition final
{
    RequestedWork work;
    ProfileCapability requiredCapability;
    std::optional<AssetVariant> variant;
};

constexpr std::array requestedWorkDefinitions{
    RequestedWorkDefinition{RequestedWork::NativeTextureOptimization,
                            ProfileCapability::NativeTextureOptimization,
                            TextureVariant::Native},
    RequestedWorkDefinition{RequestedWork::ConvertibleTextureConversion,
                            ProfileCapability::ConvertibleTextureConversion,
                            TextureVariant::Convertible},
    RequestedWorkDefinition{RequestedWork::StandardMeshOptimization,
                            ProfileCapability::StandardMeshOptimization,
                            MeshVariant::Standard},
    RequestedWorkDefinition{RequestedWork::TerrainMeshOptimization,
                            ProfileCapability::TerrainMeshOptimization,
                            MeshVariant::Terrain},
    RequestedWorkDefinition{RequestedWork::AnimationOptimization,
                            ProfileCapability::AnimationOptimization,
                            std::nullopt},
    RequestedWorkDefinition{RequestedWork::ArchiveExtraction,
                            ProfileCapability::ArchiveExtraction,
                            std::nullopt}};

struct ProfileCapabilityDefinition final
{
    ProfileCapability capability;
    AssetKind kind;
};

constexpr std::array profileCapabilityDefinitions{
    ProfileCapabilityDefinition{ProfileCapability::NativeTextureOptimization,
                                AssetKind::Texture},
    ProfileCapabilityDefinition{ProfileCapability::ConvertibleTextureConversion,
                                AssetKind::Texture},
    ProfileCapabilityDefinition{ProfileCapability::StandardMeshOptimization,
                                AssetKind::Mesh},
    ProfileCapabilityDefinition{ProfileCapability::TerrainMeshOptimization,
                                AssetKind::Mesh},
    ProfileCapabilityDefinition{ProfileCapability::AnimationOptimization,
                                AssetKind::Animation},
    ProfileCapabilityDefinition{ProfileCapability::ArchiveExtraction,
                                AssetKind::Archive},
    ProfileCapabilityDefinition{ProfileCapability::MeshReferenceMaintenance,
                                AssetKind::Mesh}};

struct BuiltInExtension final
{
    std::string_view extension;
    AssetKind kind;
};

constexpr std::array builtInExtensions{
    BuiltInExtension{".dds", AssetKind::Texture},
    BuiltInExtension{".tga", AssetKind::Texture},
    BuiltInExtension{".nif", AssetKind::Mesh},
    BuiltInExtension{".btr", AssetKind::Mesh},
    BuiltInExtension{".bto", AssetKind::Mesh},
    BuiltInExtension{".hkx", AssetKind::Animation}};

/// Returns the terminal extension using the ASCII case rules that define supported Asset extensions.
std::string normalizedTerminalExtension(const std::filesystem::path &executionPath)
{
    const auto extension = executionPath.filename().extension().native();
    std::string normalized;
    normalized.reserve(extension.size());
    for (auto character : extension) {
        if (character < 0 || character > 127)
            return {};
        if (character >= static_cast<decltype(character)>('A')
            && character <= static_cast<decltype(character)>('Z')) {
            character += static_cast<decltype(character)>('a' - 'A');
        }
        normalized.push_back(static_cast<char>(character));
    }

    return normalized;
}

/// Returns the Asset Kind encoded by one kind-specific identity alternative.
AssetKind assetKindOf(const cao::routing::AssetIdentity &identity) noexcept
{
    using cao::routing::AnimationAsset;
    using cao::routing::ArchiveAsset;
    using cao::routing::MeshAsset;
    using cao::routing::TextureAsset;

    return std::visit([](const auto &asset) {
        using Identity = std::decay_t<decltype(asset)>;
        if constexpr (std::is_same_v<Identity, TextureAsset>)
            return AssetKind::Texture;
        else if constexpr (std::is_same_v<Identity, MeshAsset>)
            return AssetKind::Mesh;
        else if constexpr (std::is_same_v<Identity, AnimationAsset>)
            return AssetKind::Animation;
        else
            return AssetKind::Archive;
    }, identity);
}

/// Safely reads a closed enum's bit from its compact value set.
template<typename Enum, std::size_t Size>
bool contains(const std::array<bool, Size> &values, const Enum value) noexcept
{
    const auto index = static_cast<std::size_t>(value);
    return index < values.size() && values[index];
}

/// Safely adds a closed enum to its compact value set.
template<typename Enum, std::size_t Size>
void include(std::array<bool, Size> &values, const Enum value) noexcept
{
    const auto index = static_cast<std::size_t>(value);
    if (index < values.size())
        values[index] = true;
}

/// Applies the routing module's deterministic ASCII-only case folding.
std::string asciiLowercase(std::string value)
{
    for (auto &character : value) {
        if (character >= 'A' && character <= 'Z')
            character += 'a' - 'A';
    }

    return value;
}

/// Reports whether the profile supports any behavior within an Asset Kind.
bool supportsAssetKind(const std::array<bool, 7> &capabilities, const AssetKind kind) noexcept
{
    for (const auto &definition : profileCapabilityDefinitions) {
        if (contains(capabilities, definition.capability) && definition.kind == kind)
            return true;
    }

    return false;
}

/// Returns the Asset Kind defined for a closed Profile Capability.
AssetKind assetKindFor(const ProfileCapability capability) noexcept
{
    for (const auto &definition : profileCapabilityDefinitions) {
        if (definition.capability == capability)
            return definition.kind;
    }

    return AssetKind::Texture;
}

/// Validates the profile Archive extension grammar without interpreting it as a filesystem path.
std::optional<cao::routing::MalformedArchiveExtensionReason> malformedReason(
    const std::string_view extension) noexcept
{
    using cao::routing::MalformedArchiveExtensionReason;

    if (extension.front() != '.')
        return MalformedArchiveExtensionReason::MissingLeadingPeriod;
    if (extension.size() == 1)
        return MalformedArchiveExtensionReason::EmptySuffix;

    for (const auto character : extension.substr(1)) {
        const bool isAsciiLetter = (character >= 'A' && character <= 'Z')
            || (character >= 'a' && character <= 'z');
        const bool isDigit = character >= '0' && character <= '9';
        if (!isAsciiLetter && !isDigit)
            return MalformedArchiveExtensionReason::InvalidCharacter;
    }

    return std::nullopt;
}

/// Selects const Routed Asset references by one carried execution fact while preserving ledger order.
template<typename Projection, typename Value>
RoutedAssetReferences matchingRoutedAssets(const std::span<const RoutedAsset> routedAssets,
                                           Projection projection,
                                           const Value expected)
{
    RoutedAssetReferences matches;
    matches.reserve(routedAssets.size());
    for (const auto &routedAsset : routedAssets) {
        if (std::invoke(projection, routedAsset) == expected)
            matches.emplace_back(std::cref(routedAsset));
    }

    return matches;
}
}

namespace cao::routing
{
RunRequest RunRequest::forWork(const ExecutionMode mode,
                               const std::initializer_list<RequestedWork> work) noexcept
{
    return fromWork(mode, work);
}

RunRequest RunRequest::forWork(const ExecutionMode mode,
                               const std::vector<RequestedWork> &work) noexcept
{
    return fromWork(mode, work);
}

RunRequest RunRequest::fromWork(const ExecutionMode mode,
                                const std::span<const RequestedWork> work) noexcept
{
    auto request = RunRequest();
    request._executionMode = mode;
    for (const auto choice : work)
        include(request._work, choice);

    return request;
}

RunRequest RunRequest::optimizeNativeTextures() noexcept
{
    return RunRequest::forWork(ExecutionMode::Apply,
                               {RequestedWork::NativeTextureOptimization});
}

ProfileCapabilities ProfileCapabilities::define(
    std::string archiveExtension,
    const std::initializer_list<ProfileCapability> capabilities)
{
    return fromDefinition(std::move(archiveExtension), capabilities);
}

ProfileCapabilities ProfileCapabilities::define(
    std::string archiveExtension,
    const std::vector<ProfileCapability> &capabilities)
{
    return fromDefinition(std::move(archiveExtension), capabilities);
}

ProfileCapabilities ProfileCapabilities::withoutArchiveExtension(
    const std::initializer_list<ProfileCapability> capabilities) noexcept
{
    return fromDefinition(std::nullopt, capabilities);
}

ProfileCapabilities ProfileCapabilities::withoutArchiveExtension(
    const std::vector<ProfileCapability> &capabilities) noexcept
{
    return fromDefinition(std::nullopt, capabilities);
}

ProfileCapabilities ProfileCapabilities::fromDefinition(
    std::optional<std::string> archiveExtension,
    const std::span<const ProfileCapability> capabilities) noexcept
{
    auto profile = ProfileCapabilities();
    profile._archiveExtension = std::move(archiveExtension);
    for (const auto capability : capabilities)
        include(profile._capabilities, capability);

    return profile;
}

RoutingPolicyBuildResult RoutingPolicy::compile(RunRequest request, ProfileCapabilities capabilities)
{
    PolicyValidationErrors errors;
    errors.reserve(8);

    std::string normalizedArchiveExtension;
    if (!capabilities._archiveExtension.has_value() || capabilities._archiveExtension->empty()) {
        errors.emplace_back(MissingArchiveExtension{});
    } else if (const auto reason = malformedReason(*capabilities._archiveExtension)) {
        errors.emplace_back(MalformedArchiveExtension{*capabilities._archiveExtension, *reason});
    } else {
        normalizedArchiveExtension = asciiLowercase(*capabilities._archiveExtension);
        for (const auto &builtIn : builtInExtensions) {
            if (normalizedArchiveExtension == builtIn.extension) {
                errors.emplace_back(AmbiguousArchiveExtension{*capabilities._archiveExtension,
                                                              std::string(builtIn.extension),
                                                              builtIn.kind});
                break;
            }
        }
    }

    // Validate every explicit choice before derived work so adapters receive the complete conflict set in one pass.
    for (const auto &definition : requestedWorkDefinitions) {
        if (!contains(request._work, definition.work))
            continue;

        if (contains(capabilities._capabilities, definition.requiredCapability))
            continue;

        const auto kind = assetKindFor(definition.requiredCapability);
        if (!supportsAssetKind(capabilities._capabilities, kind)) {
            errors.emplace_back(UnsupportedRequestedAssetKind{definition.work, kind});
            continue;
        }

        if (definition.variant.has_value()) {
            errors.emplace_back(UnsupportedRequestedAssetVariant{definition.work,
                                                                 *definition.variant});
        } else {
            errors.emplace_back(UnsupportedRequestedAssetKind{definition.work, kind});
        }
    }

    const bool maintainsMeshReferences = contains(request._work,
                                                  RequestedWork::ConvertibleTextureConversion);
    if (maintainsMeshReferences
        && !contains(capabilities._capabilities,
                     ProfileCapability::MeshReferenceMaintenance)) {
        errors.emplace_back(UnsupportedDerivedOperation{
            RequestedWork::ConvertibleTextureConversion,
            AssetOperation::MeshReferenceMaintenance});
    }

    if (!errors.empty())
        return RoutingPolicyBuildResult(std::move(errors));

    return RoutingPolicyBuildResult(RoutingPolicy(request._executionMode,
                                                  request._work,
                                                  maintainsMeshReferences,
                                                  std::move(normalizedArchiveExtension)));
}

RoutingPolicy::RoutingPolicy(const ExecutionMode executionMode,
                             std::array<bool, 6> work,
                             const bool meshReferenceMaintenance,
                             std::string archiveExtension)
    : _executionMode(executionMode)
    , _work(work)
    , _meshReferenceMaintenance(meshReferenceMaintenance)
    , _archiveExtension(std::move(archiveExtension))
{}

ExecutionMode RoutingPolicy::executionMode() const noexcept
{
    return _executionMode;
}

bool RoutingPolicy::requests(const RequestedWork work) const noexcept
{
    return contains(_work, work);
}

bool RoutingPolicy::maintainsMeshReferences() const noexcept
{
    return _meshReferenceMaintenance;
}

const std::string &RoutingPolicy::archiveExtension() const noexcept
{
    return _archiveExtension;
}

RoutingPolicyBuildResult::RoutingPolicyBuildResult(RoutingPolicy policy)
    : _outcome(std::move(policy))
{}

RoutingPolicyBuildResult::RoutingPolicyBuildResult(PolicyValidationErrors errors)
    : _outcome(std::move(errors))
{}

bool RoutingPolicyBuildResult::hasPolicy() const noexcept
{
    return std::holds_alternative<RoutingPolicy>(_outcome);
}

const RoutingPolicy *RoutingPolicyBuildResult::policy() const noexcept
{
    return std::get_if<RoutingPolicy>(&_outcome);
}

std::span<const PolicyValidationError> RoutingPolicyBuildResult::errors() const noexcept
{
    const auto *errors = std::get_if<PolicyValidationErrors>(&_outcome);
    if (errors == nullptr)
        return {};

    return *errors;
}

TextureAsset::TextureAsset(const TextureVariant variant) noexcept
    : _variant(variant)
{}

TextureVariant TextureAsset::variant() const noexcept
{
    return _variant;
}

MeshAsset::MeshAsset(const MeshVariant variant) noexcept
    : _variant(variant)
{}

MeshVariant MeshAsset::variant() const noexcept
{
    return _variant;
}

bool AssetOperations::contains(const AssetOperation operation) const noexcept
{
    return ::contains(_operations, operation);
}

void AssetOperations::include(const AssetOperation operation) noexcept
{
    ::include(_operations, operation);
}

bool AssetOperations::empty() const noexcept
{
    return std::none_of(_operations.begin(), _operations.end(), [](const bool included) {
        return included;
    });
}

RoutedAsset::RoutedAsset(std::filesystem::path executionPath,
                         AssetIdentity identity,
                         const RunPhase phase,
                         const OptimizerTarget target,
                         const ExecutionMode executionMode,
                         AssetOperations operations)
    : _executionPath(std::move(executionPath))
    , _identity(std::move(identity))
    , _phase(phase)
    , _target(target)
    , _executionMode(executionMode)
    , _operations(operations)
{}

const std::filesystem::path &RoutedAsset::executionPath() const noexcept
{
    return _executionPath;
}

AssetKind RoutedAsset::kind() const noexcept
{
    return assetKindOf(_identity);
}

const AssetIdentity &RoutedAsset::identity() const noexcept
{
    return _identity;
}

RunPhase RoutedAsset::phase() const noexcept
{
    return _phase;
}

OptimizerTarget RoutedAsset::target() const noexcept
{
    return _target;
}

ExecutionMode RoutedAsset::executionMode() const noexcept
{
    return _executionMode;
}

const AssetOperations &RoutedAsset::operations() const noexcept
{
    return _operations;
}

SkippedAsset::SkippedAsset(std::filesystem::path executionPath,
                           AssetIdentity identity,
                           const SkipReason reason)
    : _executionPath(std::move(executionPath))
    , _identity(std::move(identity))
    , _reason(reason)
{}

const std::filesystem::path &SkippedAsset::executionPath() const noexcept
{
    return _executionPath;
}

AssetKind SkippedAsset::kind() const noexcept
{
    return assetKindOf(_identity);
}

const AssetIdentity &SkippedAsset::identity() const noexcept
{
    return _identity;
}

SkipReason SkippedAsset::reason() const noexcept
{
    return _reason;
}

RoutingLedger::RoutingLedger(std::vector<RoutedAsset> routedAssets,
                             std::map<SkipReason, std::size_t> skippedAssetCounts) noexcept
    : _routedAssets(std::move(routedAssets))
    , _skippedAssetCounts(std::move(skippedAssetCounts))
{}

std::span<const RoutedAsset> RoutingLedger::routedAssets() const noexcept
{
    return _routedAssets;
}

RoutedAssetReferences RoutingLedger::routedAssets(const RunPhase phase) const
{
    return matchingRoutedAssets(std::span<const RoutedAsset>(_routedAssets),
                                &RoutedAsset::phase,
                                phase);
}

RoutedAssetReferences RoutingLedger::routedAssets(const OptimizerTarget target) const
{
    return matchingRoutedAssets(std::span<const RoutedAsset>(_routedAssets),
                                &RoutedAsset::target,
                                target);
}

std::size_t RoutingLedger::skippedAssetCount(const SkipReason reason) const noexcept
{
    const auto count = _skippedAssetCounts.find(reason);
    return count == _skippedAssetCounts.end() ? 0 : count->second;
}

AssetRouter::AssetRouter(RoutingPolicy policy) noexcept
    : _policy(std::move(policy))
{}

RoutingDecision AssetRouter::route(const std::filesystem::path &executionPath) const
{
    const auto extension = normalizedTerminalExtension(executionPath);
    const auto kindHasWork = [this](const AssetKind kind) {
        switch (kind) {
        case AssetKind::Texture:
            return _policy.requests(RequestedWork::NativeTextureOptimization)
                || _policy.requests(RequestedWork::ConvertibleTextureConversion);
        case AssetKind::Mesh:
            return _policy.requests(RequestedWork::StandardMeshOptimization)
                || _policy.requests(RequestedWork::TerrainMeshOptimization)
                || _policy.maintainsMeshReferences();
        case AssetKind::Animation:
            return _policy.requests(RequestedWork::AnimationOptimization);
        case AssetKind::Archive:
            return _policy.requests(RequestedWork::ArchiveExtraction);
        }
        return false;
    };
    const auto finishDecision = [this, &executionPath, &kindHasWork](
                                    AssetIdentity identity,
                                    const RunPhase phase,
                                    const OptimizerTarget target,
                                    AssetOperations operations) -> RoutingDecision {
        const auto kind = assetKindOf(identity);
        // Dry Run disables Archive extraction before kind/variant eligibility is considered.
        if (phase == RunPhase::ArchiveExtraction
            && _policy.executionMode() == ExecutionMode::DryRun) {
            return SkippedAsset(executionPath, std::move(identity), SkipReason::DisabledPhase);
        }
        if (!operations.empty()) {
            return RoutedAsset(executionPath,
                               std::move(identity),
                               phase,
                               target,
                               _policy.executionMode(),
                               operations);
        }
        const auto reason = kindHasWork(kind)
            ? SkipReason::ExcludedAssetVariant
            : SkipReason::DisabledAssetKind;
        return SkippedAsset(executionPath, std::move(identity), reason);
    };

    if (extension == ".dds") {
        AssetOperations operations;
        if (_policy.requests(RequestedWork::NativeTextureOptimization))
            operations.include(AssetOperation::Optimization);
        return finishDecision(TextureAsset(TextureVariant::Native),
                              RunPhase::LooseAssetProcessing,
                              OptimizerTarget::Texture,
                              operations);
    }
    if (extension == ".tga") {
        AssetOperations operations;
        if (_policy.requests(RequestedWork::ConvertibleTextureConversion))
            operations.include(AssetOperation::Conversion);
        return finishDecision(TextureAsset(TextureVariant::Convertible),
                              RunPhase::LooseAssetProcessing,
                              OptimizerTarget::Texture,
                              operations);
    }
    if (extension == ".nif" || extension == ".btr" || extension == ".bto") {
        AssetOperations operations;
        const auto variant = extension == ".nif" ? MeshVariant::Standard : MeshVariant::Terrain;
        const auto requestedOptimization = variant == MeshVariant::Standard
            ? RequestedWork::StandardMeshOptimization
            : RequestedWork::TerrainMeshOptimization;
        if (_policy.requests(requestedOptimization))
            operations.include(AssetOperation::Optimization);
        // Convertible Texture conversion changes referenced names, so both Mesh Variants carry maintenance independently of optimization.
        if (_policy.maintainsMeshReferences())
            operations.include(AssetOperation::MeshReferenceMaintenance);
        return finishDecision(MeshAsset(variant),
                              RunPhase::LooseAssetProcessing,
                              OptimizerTarget::Mesh,
                              operations);
    }
    if (extension == ".hkx") {
        AssetOperations operations;
        if (_policy.requests(RequestedWork::AnimationOptimization))
            operations.include(AssetOperation::Optimization);
        return finishDecision(AnimationAsset{},
                              RunPhase::LooseAssetProcessing,
                              OptimizerTarget::Animation,
                              operations);
    }
    if (extension == _policy.archiveExtension()) {
        AssetOperations operations;
        if (_policy.requests(RequestedWork::ArchiveExtraction))
            operations.include(AssetOperation::Extraction);
        return finishDecision(ArchiveAsset{},
                              RunPhase::ArchiveExtraction,
                              OptimizerTarget::Archive,
                              operations);
    }

    return UnsupportedDecision{};
}

RoutingLedger AssetRouter::route(
    const std::span<const std::filesystem::path> executionPaths) const
{
    std::vector<RoutedAsset> routedAssets;
    routedAssets.reserve(executionPaths.size());
    std::map<SkipReason, std::size_t> skippedAssetCounts;
    for (const auto &executionPath : executionPaths) {
        auto decision = route(executionPath);
        if (auto *routedAsset = std::get_if<RoutedAsset>(&decision)) {
            routedAssets.push_back(std::move(*routedAsset));
        } else if (const auto *skippedAsset = std::get_if<SkippedAsset>(&decision)) {
            ++skippedAssetCounts[skippedAsset->reason()];
        }
        // Unsupported paths are intentionally absent because they are neither work nor recognized exclusions.
    }

    return RoutingLedger(std::move(routedAssets), skippedAssetCounts);
}
}
