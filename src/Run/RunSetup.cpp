#include "RunSetup.h"

#include <vector>

namespace cao::run
{
namespace
{
template<class... Visitors>
struct Overloaded : Visitors...
{
    using Visitors::operator()...;
};

/// Returns the stable domain name used to present one closed Asset Kind.
std::string assetKindName(const routing::AssetKind kind)
{
    switch (kind) {
    case routing::AssetKind::Texture:
        return "Texture";
    case routing::AssetKind::Mesh:
        return "Mesh";
    case routing::AssetKind::Animation:
        return "Animation";
    case routing::AssetKind::Archive:
        return "Archive";
    }
    return "Asset";
}

/// Returns a user-facing phrase for one explicit closed work choice.
std::string requestedWorkName(const routing::RequestedWork work)
{
    switch (work) {
    case routing::RequestedWork::NativeTextureOptimization:
        return "native Texture optimization";
    case routing::RequestedWork::ConvertibleTextureConversion:
        return "convertible Texture conversion";
    case routing::RequestedWork::StandardMeshOptimization:
        return "standard Mesh optimization";
    case routing::RequestedWork::TerrainMeshOptimization:
        return "terrain Mesh optimization";
    case routing::RequestedWork::AnimationOptimization:
        return "Animation optimization";
    case routing::RequestedWork::ArchiveExtraction:
        return "Archive extraction";
    }
    return "requested work";
}

/// Returns the kind-qualified name carried by a typed Asset Variant.
std::string assetVariantName(const routing::AssetVariant &variant)
{
    return std::visit(Overloaded{
                          [](const routing::TextureVariant texture) {
                              return texture == routing::TextureVariant::Native
                                         ? std::string("native Texture")
                                         : std::string("convertible Texture");
                          },
                          [](const routing::MeshVariant mesh) {
                              return mesh == routing::MeshVariant::Standard
                                         ? std::string("standard Mesh")
                                         : std::string("terrain Mesh");
                          },
                      },
                      variant);
}

/// Explains the exact syntax constraint rejected for a profile Archive extension.
std::string malformedArchiveExtensionReason(
    const routing::MalformedArchiveExtensionReason reason)
{
    switch (reason) {
    case routing::MalformedArchiveExtensionReason::MissingLeadingPeriod:
        return "it must begin with a period";
    case routing::MalformedArchiveExtensionReason::EmptySuffix:
        return "it must include characters after the period";
    case routing::MalformedArchiveExtensionReason::InvalidCharacter:
        return "it may contain only ASCII letters and digits after the period";
    }
    return "its format is invalid";
}

/// Maps named application choices into the closed request values accepted by AssetRouting.
routing::RunRequest adaptedRequest(const ApplicationRunChoices &choices)
{
    std::vector<routing::RequestedWork> work;
    work.reserve(6);
    const auto includeWhenSelected = [&work](const bool selected,
                                             const routing::RequestedWork requestedWork) {
        if (selected)
            work.push_back(requestedWork);
    };

    includeWhenSelected(choices.optimizeNativeTextures,
                        routing::RequestedWork::NativeTextureOptimization);
    includeWhenSelected(choices.convertTextures,
                        routing::RequestedWork::ConvertibleTextureConversion);
    includeWhenSelected(choices.optimizeStandardMeshes,
                        routing::RequestedWork::StandardMeshOptimization);
    includeWhenSelected(choices.optimizeTerrainMeshes,
                        routing::RequestedWork::TerrainMeshOptimization);
    includeWhenSelected(choices.optimizeAnimations,
                        routing::RequestedWork::AnimationOptimization);
    includeWhenSelected(choices.extractArchives,
                        routing::RequestedWork::ArchiveExtraction);
    return routing::RunRequest::forWork(choices.executionMode, work);
}

/// Maps selected-profile facts into the closed capability values accepted by AssetRouting.
routing::ProfileCapabilities adaptedCapabilities(const SelectedProfileFacts &profile)
{
    std::vector<routing::ProfileCapability> capabilities;
    capabilities.reserve(7);
    const auto includeWhenSupported = [&capabilities](
                                          const bool supported,
                                          const routing::ProfileCapability capability) {
        if (supported)
            capabilities.push_back(capability);
    };

    includeWhenSupported(profile.supportsNativeTextureOptimization,
                         routing::ProfileCapability::NativeTextureOptimization);
    includeWhenSupported(profile.supportsTextureConversion,
                         routing::ProfileCapability::ConvertibleTextureConversion);
    includeWhenSupported(profile.supportsStandardMeshOptimization,
                         routing::ProfileCapability::StandardMeshOptimization);
    includeWhenSupported(profile.supportsTerrainMeshOptimization,
                         routing::ProfileCapability::TerrainMeshOptimization);
    includeWhenSupported(profile.supportsAnimationOptimization,
                         routing::ProfileCapability::AnimationOptimization);
    includeWhenSupported(profile.supportsArchiveExtraction,
                         routing::ProfileCapability::ArchiveExtraction);
    includeWhenSupported(profile.supportsMeshReferenceMaintenance,
                         routing::ProfileCapability::MeshReferenceMaintenance);

    if (profile.archiveExtension.has_value())
        return routing::ProfileCapabilities::define(*profile.archiveExtension, capabilities);
    return routing::ProfileCapabilities::withoutArchiveExtension(capabilities);
}
}

routing::RoutingPolicyBuildResult RunSetup::prepare(
    const ApplicationRunChoices &choices,
    const SelectedProfileFacts &profile)
{
    return routing::RoutingPolicy::compile(adaptedRequest(choices),
                                           adaptedCapabilities(profile));
}

std::string policyValidationErrorMessage(const routing::PolicyValidationError &error)
{
    return std::visit(
        Overloaded{
            [](const routing::MissingArchiveExtension &) {
                return std::string("The selected profile is missing its Archive extension.");
            },
            [](const routing::MalformedArchiveExtension &malformed) {
                return "The selected profile Archive extension '" + malformed.extension
                       + "' is invalid because "
                       + malformedArchiveExtensionReason(malformed.reason) + ".";
            },
            [](const routing::AmbiguousArchiveExtension &ambiguous) {
                return "The selected profile Archive extension '" + ambiguous.extension
                       + "' conflicts with the built-in "
                       + assetKindName(ambiguous.conflictingKind) + " extension '"
                       + ambiguous.conflictingExtension + "'.";
            },
            [](const routing::UnsupportedRequestedAssetKind &unsupported) {
                return "The selected profile does not support requested "
                       + requestedWorkName(unsupported.request) + " for the "
                       + assetKindName(unsupported.kind) + " Asset Kind.";
            },
            [](const routing::UnsupportedRequestedAssetVariant &unsupported) {
                return "The selected profile does not support requested "
                       + requestedWorkName(unsupported.request) + " for the "
                       + assetVariantName(unsupported.variant) + " Asset Variant.";
            },
            [](const routing::UnsupportedDerivedOperation &unsupported) {
                return "Requested " + requestedWorkName(unsupported.cause)
                       + " requires unsupported Mesh Reference Maintenance.";
            },
        },
        error);
}
}
