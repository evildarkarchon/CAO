#pragma once

#include "Run/RunPreparation.h"

#include <QTemporaryDir>

#include <filesystem>
#include <memory>
#include <stdexcept>

/// Returns an existing empty Mod Root retained until the test process exits.
inline std::filesystem::path testModRoot() {
    static const QTemporaryDir directory;
    if (!directory.isValid()) throw std::runtime_error("Could not create the test Mod Root");
    return std::filesystem::path(directory.path().toStdWString());
}

/// Supplies independent profile facts for lifecycle tests without loading application settings.
class TestRunConfigurationProvider final : public cao::run::RunConfigurationProvider {
   public:
    /// Returns a valid profile supporting every work choice so tests reach their intended phase.
    [[nodiscard]] cao::run::RunConfiguration load(std::string_view) const override {
        return cao::run::RunConfiguration(cao::run::SelectedProfileFacts{
            .archiveExtension = ".bsa",
            .supportsNativeTextureOptimization = true,
            .supportsTextureConversion = true,
            .supportsStandardMeshOptimization = true,
            .supportsTerrainMeshOptimization = true,
            .supportsAnimationOptimization = true,
            .supportsArchiveExtraction = true,
            .supportsMeshReferenceMaintenance = true,
            .supportsArchiveCreation = true,
        });
    }
};

/// Returns a shared immutable provider after initializing filesystem fixtures on the caller thread.
inline std::shared_ptr<const cao::run::RunConfigurationProvider> testRunConfiguration() {
    // Service construction precedes scheduling, so concurrent request helpers only read the root.
    static_cast<void>(testModRoot());
    static const auto provider = std::make_shared<const TestRunConfigurationProvider>();
    return provider;
}
