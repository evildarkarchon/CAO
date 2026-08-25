#include "ApplicationRunSetup.h"

#include "Profiles.h"

namespace cao::run {
namespace {
/// Snapshots application option facts without allowing OptionsCAO to cross the AssetRouting
/// interface.
ApplicationRunChoices choicesFrom(const OptionsCAO& options) {
    const bool optimizeNativeTextures = options.bTexturesNecessary || options.bTexturesCompress ||
                                        options.bTexturesMipmaps || options.bTexturesResizeSize ||
                                        options.bTexturesResizeRatio;
    // The application has one Mesh level, while routing keeps Standard and Terrain choices
    // explicit.
    const bool optimizeMeshes = options.iMeshesOptimizationLevel > 0;
    // A profile's TGA preference only participates when the user selected Texture work for this
    // run.
    return ApplicationRunChoices{
        .executionMode =
            options.bDryRun ? routing::ExecutionMode::DryRun : routing::ExecutionMode::Apply,
        .optimizeNativeTextures = optimizeNativeTextures,
        .convertTextures = optimizeNativeTextures && Profiles::texturesConvertTga(),
        .optimizeStandardMeshes = optimizeMeshes,
        .optimizeTerrainMeshes = optimizeMeshes,
        .optimizeAnimations = options.bAnimationsOptimization,
        .extractArchives = options.bBsaExtract,
    };
}

/// Snapshots the selected profile and converts its Archive type to a dedicated extension value.
SelectedProfileFacts factsFromSelectedProfile() {
    const auto archiveExtension =
        btu::common::as_ascii(btu::bsa::Settings::get(Profiles::bsaGame()).extension);
    const bool texturesEnabled = Profiles::texturesEnabled();
    const bool meshesEnabled = Profiles::meshesEnabled();
    // Reference maintenance belongs to Texture conversion; Mesh enablement controls optimization
    // only.
    return SelectedProfileFacts{
        .archiveExtension = std::string(archiveExtension.data(), archiveExtension.size()),
        .supportsNativeTextureOptimization = texturesEnabled,
        .supportsTextureConversion = texturesEnabled,
        .supportsStandardMeshOptimization = meshesEnabled,
        .supportsTerrainMeshOptimization = meshesEnabled,
        .supportsAnimationOptimization = Profiles::animationsEnabled(),
        .supportsArchiveExtraction = Profiles::bsaEnabled(),
        .supportsMeshReferenceMaintenance = texturesEnabled,
    };
}
}  // namespace

routing::RoutingPolicyBuildResult prepareApplicationRun(const OptionsCAO& options) {
    return RunSetup::prepare(choicesFrom(options), factsFromSelectedProfile());
}

QStringList policyValidationErrorMessages(
    const std::span<const routing::PolicyValidationError> errors) {
    QStringList messages;
    messages.reserve(static_cast<int>(errors.size()));
    for (const auto& error : errors)
        messages.push_back(QString::fromStdString(policyValidationErrorMessage(error)));
    return messages;
}
}  // namespace cao::run
