#pragma once

#include "AssetRouting/AssetRouter.h"

#include <optional>
#include <string>

namespace cao::run
{
/// Application-level work selections captured before filesystem discovery begins.
struct ApplicationRunChoices final
{
    routing::ExecutionMode executionMode{routing::ExecutionMode::Apply};
    bool optimizeNativeTextures{};
    bool convertTextures{};
    bool optimizeStandardMeshes{};
    bool optimizeTerrainMeshes{};
    bool optimizeAnimations{};
    bool extractArchives{};
};

/// Selected-profile facts adapted without exposing the profile singleton to AssetRouting.
struct SelectedProfileFacts final
{
    std::optional<std::string> archiveExtension;
    bool supportsNativeTextureOptimization{};
    bool supportsTextureConversion{};
    bool supportsStandardMeshOptimization{};
    bool supportsTerrainMeshOptimization{};
    bool supportsAnimationOptimization{};
    bool supportsArchiveExtraction{};
    bool supportsMeshReferenceMaintenance{};
};

/// Compiles application and selected-profile facts into one immutable Routing Policy outcome.
class RunSetup final
{
public:
    /// Adapts the supplied facts to dedicated routing values and returns either one policy or every conflict.
    [[nodiscard]] static routing::RoutingPolicyBuildResult prepare(
        const ApplicationRunChoices &choices,
        const SelectedProfileFacts &profile);
};

/// Produces a caller-facing message by visiting one structured policy validation conflict.
[[nodiscard]] std::string policyValidationErrorMessage(
    const routing::PolicyValidationError &error);
}
