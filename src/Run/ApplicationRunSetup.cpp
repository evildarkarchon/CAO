#include "ApplicationRunSetup.h"

#include "Profiles.h"

#include <stdexcept>

namespace cao::run {
namespace {
/// Loads one run's settings with thread-local Qt objects and no profile-singleton access.
class ApplicationRunConfigurationProvider final : public RunConfigurationProvider {
   public:
    /// Owns the absolute configuration root before asynchronous execution can change context.
    explicit ApplicationRunConfigurationProvider(QString profilesDirectory)
        : _profilesDirectory(std::move(profilesDirectory)) {}

    /// Reads owned profile capabilities and legacy child exclusions; missing profiles fail
    /// Preparing.
    RunConfiguration load(std::string_view identity) const override {
        const auto name = QString::fromUtf8(identity.data(), static_cast<int>(identity.size()));
        const QDir profiles(_profilesDirectory);
        const QDir selected(profiles.filePath(name));
        const auto profilePath = selected.filePath(QStringLiteral("profile.ini"));
        if (!QFile::exists(profilePath))
            throw std::runtime_error("Selected profile is unavailable");
        QSettings settings(profilePath, QSettings::IniFormat);
        const auto game = static_cast<btu::Game>(settings.value("BSA/bsaGame").toInt());
        const auto extension = btu::common::as_ascii(btu::bsa::Settings::get(game).extension);
        const bool textures = settings.value("Textures/texturesEnabled").toBool();
        const bool meshes = settings.value("Meshes/meshesEnabled").toBool();
        const bool archives = settings.value("BSA/bsaEnabled").toBool();
        SelectedProfileFacts facts{
            .archiveExtension = std::string(extension.data(), extension.size()),
            .supportsNativeTextureOptimization = textures,
            .supportsTextureConversion = textures,
            .supportsStandardMeshOptimization = meshes,
            .supportsTerrainMeshOptimization = meshes,
            .supportsAnimationOptimization =
                settings.value("Animations/animationsEnabled").toBool(),
            .supportsArchiveExtraction = archives,
            .supportsMeshReferenceMaintenance = textures,
            .supportsArchiveCreation = archives,
        };
        if (settings.status() != QSettings::NoError)
            throw std::runtime_error("Selected profile could not be read");

        // Preserve Profiles::getFile's SSE fallback without sharing its QObject-owned settings.
        auto ignoredPath = selected.filePath(QStringLiteral("ignoredMods.txt"));
        if (!QFile::exists(ignoredPath))
            ignoredPath = profiles.filePath(QStringLiteral("SSE/ignoredMods.txt"));
        QFile ignoredFile(ignoredPath);
        std::vector<std::string> ignored;
        if (ignoredFile.open(QIODevice::ReadOnly)) {
            while (!ignoredFile.atEnd()) {
                const auto line = QString::fromUtf8(ignoredFile.readLine()).simplified();
                if (!line.isEmpty() && !line.startsWith('#')) ignored.push_back(line.toStdString());
            }
        }
        return RunConfiguration(std::move(facts), std::move(ignored), {"separator"});
    }

   private:
    QString _profilesDirectory;
};

/// Snapshots application option facts without allowing OptionsCAO to cross the AssetRouting
/// interface.
ApplicationRunChoices choicesFrom(const OptionsCAO& options) {
    const bool optimizeNativeTextures = options.bTexturesNecessary || options.bTexturesCompress ||
                                        options.bTexturesMipmaps || options.bTexturesResizeSize ||
                                        options.bTexturesResizeRatio;
    // The application has one Mesh level, while routing keeps Standard and Terrain choices
    // explicit.
    // Resaving is independent of optimization level, so resave-only runs still need Mesh routing.
    const bool optimizeMeshes = options.iMeshesOptimizationLevel > 0 || options.bMeshesResave;
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
        // Archive creation is validated here as well, otherwise a CLI run could pack and then
        // delete Loose Assets under a profile that declares no Archive support at all.
        .createArchives = options.bBsaCreate,
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
        .supportsArchiveCreation = Profiles::bsaEnabled(),
    };
}
}  // namespace

RunRequest makeApplicationRunRequest(const OptionsCAO& options) {
    // Validate numeric intent here; directory resolution belongs to the service's Preparing phase.
    if (options.mode != OptionsCAO::SingleMod && options.mode != OptionsCAO::SeveralMods)
        throw std::invalid_argument("This mode does not exist.");
    if (options.iMeshesOptimizationLevel < 0 || options.iMeshesOptimizationLevel > 3)
        throw std::invalid_argument("Mesh optimization level must be between 0 and 3.");
    if (options.iTexturesTargetWidth % 2 != 0 || options.iTexturesTargetHeight % 2 != 0)
        throw std::invalid_argument("Texture target width and height must be even.");
    if (options.bTexturesResizeRatio &&
        (options.iTexturesTargetWidthRatio == 0 || options.iTexturesTargetHeightRatio == 0))
        throw std::invalid_argument("Texture resizing requires non-zero width and height ratios.");
    if (options.bTexturesResizeSize &&
        (options.iTexturesTargetWidth == 0 || options.iTexturesTargetHeight == 0))
        throw std::invalid_argument("Texture resizing requires non-zero width and height.");

    const auto choices = choicesFrom(options);
    std::vector<routing::RequestedWork> work;
    const auto include = [&work](bool selected, routing::RequestedWork item) {
        if (selected) work.push_back(item);
    };
    include(choices.optimizeNativeTextures, routing::RequestedWork::NativeTextureOptimization);
    include(choices.convertTextures, routing::RequestedWork::ConvertibleTextureConversion);
    include(choices.optimizeStandardMeshes, routing::RequestedWork::StandardMeshOptimization);
    include(choices.optimizeTerrainMeshes, routing::RequestedWork::TerrainMeshOptimization);
    include(choices.optimizeAnimations, routing::RequestedWork::AnimationOptimization);
    include(choices.extractArchives, routing::RequestedWork::ArchiveExtraction);
    include(choices.createArchives, routing::RequestedWork::ArchiveCreation);
    const auto path = std::filesystem::path(options.userPath.toStdWString());
    auto selection = options.mode == OptionsCAO::SeveralMods ? ModSelection::childModRoots(path)
                                                             : ModSelection::singleModRoot(path);
    return RunRequest::create(Profiles::currentProfile().toStdString(), choices.executionMode,
                              std::move(selection), std::move(work));
}

std::shared_ptr<const RunConfigurationProvider> makeApplicationRunConfigurationProvider() {
    return std::make_shared<ApplicationRunConfigurationProvider>(QDir("profiles").absolutePath());
}

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
