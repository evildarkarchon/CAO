#include "AssetRouter.h"

#include <array>
#include <cstddef>
#include <optional>
#include <string_view>
#include <utility>

namespace
{
using cao::routing::AssetKind;
using cao::routing::AssetVariant;
using cao::routing::MeshVariant;
using cao::routing::ProfileCapability;
using cao::routing::RequestedWork;
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

/// Compares the terminal extension using the ASCII case rules that define supported Asset extensions.
bool hasNativeTextureExtension(const std::filesystem::path &executionPath)
{
    using PathCharacter = std::filesystem::path::value_type;
    constexpr std::array expectedExtension{
        static_cast<PathCharacter>('.'),
        static_cast<PathCharacter>('d'),
        static_cast<PathCharacter>('d'),
        static_cast<PathCharacter>('s')};

    const auto extension = executionPath.filename().extension().native();
    if (extension.size() != expectedExtension.size())
        return false;

    for (std::size_t index = 0; index < extension.size(); ++index) {
        auto character = extension[index];
        if (character >= static_cast<PathCharacter>('A')
            && character <= static_cast<PathCharacter>('Z')) {
            character += static_cast<PathCharacter>('a' - 'A');
        }

        if (character != expectedExtension[index])
            return false;
    }

    return true;
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
}

namespace cao::routing
{
RunRequest RunRequest::forWork(const ExecutionMode mode,
                               const std::initializer_list<RequestedWork> work) noexcept
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
    auto profile = ProfileCapabilities();
    profile._archiveExtension = std::move(archiveExtension);
    for (const auto capability : capabilities)
        include(profile._capabilities, capability);

    return profile;
}

ProfileCapabilities ProfileCapabilities::withoutArchiveExtension(
    const std::initializer_list<ProfileCapability> capabilities) noexcept
{
    auto profile = ProfileCapabilities();
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

RoutedAsset::RoutedAsset(std::filesystem::path executionPath, const TextureAsset texture)
    : _executionPath(std::move(executionPath))
    , _texture(texture)
{}

const std::filesystem::path &RoutedAsset::executionPath() const noexcept
{
    return _executionPath;
}

const TextureAsset &RoutedAsset::texture() const noexcept
{
    return _texture;
}

AssetRouter::AssetRouter(RoutingPolicy policy) noexcept
    : _policy(std::move(policy))
{}

RoutingDecision AssetRouter::route(const std::filesystem::path &executionPath) const
{
    if (!hasNativeTextureExtension(executionPath))
        return UnsupportedDecision{};

    return RoutedAsset(executionPath, TextureAsset(TextureVariant::Native));
}
}
