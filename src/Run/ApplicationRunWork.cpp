#include "ApplicationRunWork.h"

#include "BsaOptimizer.h"
#include "MainOptimizer.h"
#include "OptimizerProfileSnapshot.h"
#include "Run/AssetRun.h"
#include "Run/RunWorkRecord.h"
#include "Run/TemporaryArtifactRegistry.h"

#include <stdexcept>

namespace cao::run {
namespace {
/// Plain values may cross threads; OptionsCAO itself is a thread-affine QObject.
struct OptionsSnapshot final {
    bool bBsaExtract;
    bool bBsaCreate;
    bool bBsaDeleteBackup;
    bool bBsaMergeIncomp;
    bool bBsaMergeTexture;
    bool bBsaProcessContent;
    bool bBsaCreateDummies;
    bool bBsaCompress;
    bool bBsaDeleteSource;
    bool bAnimationsOptimization;
    bool bDryRun;
    int iMeshesOptimizationLevel;
    bool bMeshesHeadparts;
    bool bMeshesResave;
    bool bTexturesNecessary;
    bool bTexturesCompress;
    bool bTexturesMipmaps;
    bool bTexturesResizeSize;
    size_t iTexturesTargetHeight;
    size_t iTexturesTargetWidth;
    bool bTexturesResizeRatio;
    uint iTexturesTargetWidthRatio;
    uint iTexturesTargetHeightRatio;
    bool bDebugLog;
    OptionsCAO::OptimizationMode mode;
    QString userPath;

    /// Copies the complete CLI option intent without retaining its QObject.
    explicit OptionsSnapshot(const OptionsCAO& options)
        : bBsaExtract(options.bBsaExtract),
          bBsaCreate(options.bBsaCreate),
          bBsaDeleteBackup(options.bBsaDeleteBackup),
          bBsaMergeIncomp(options.bBsaMergeIncomp),
          bBsaMergeTexture(options.bBsaMergeTexture),
          bBsaProcessContent(options.bBsaProcessContent),
          bBsaCreateDummies(options.bBsaCreateDummies),
          bBsaCompress(options.bBsaCompress),
          bBsaDeleteSource(options.bBsaDeleteSource),
          bAnimationsOptimization(options.bAnimationsOptimization),
          bDryRun(options.bDryRun),
          iMeshesOptimizationLevel(options.iMeshesOptimizationLevel),
          bMeshesHeadparts(options.bMeshesHeadparts),
          bMeshesResave(options.bMeshesResave),
          bTexturesNecessary(options.bTexturesNecessary),
          bTexturesCompress(options.bTexturesCompress),
          bTexturesMipmaps(options.bTexturesMipmaps),
          bTexturesResizeSize(options.bTexturesResizeSize),
          iTexturesTargetHeight(options.iTexturesTargetHeight),
          iTexturesTargetWidth(options.iTexturesTargetWidth),
          bTexturesResizeRatio(options.bTexturesResizeRatio),
          iTexturesTargetWidthRatio(options.iTexturesTargetWidthRatio),
          iTexturesTargetHeightRatio(options.iTexturesTargetHeightRatio),
          bDebugLog(options.bDebugLog),
          mode(options.mode),
          userPath(options.userPath) {}

    /// Materializes the owned values into an execution-thread options object.
    void apply(OptionsCAO& options) const {
        options.bBsaExtract = bBsaExtract;
        options.bBsaCreate = bBsaCreate;
        options.bBsaDeleteBackup = bBsaDeleteBackup;
        options.bBsaMergeIncomp = bBsaMergeIncomp;
        options.bBsaMergeTexture = bBsaMergeTexture;
        options.bBsaProcessContent = bBsaProcessContent;
        options.bBsaCreateDummies = bBsaCreateDummies;
        options.bBsaCompress = bBsaCompress;
        options.bBsaDeleteSource = bBsaDeleteSource;
        options.bAnimationsOptimization = bAnimationsOptimization;
        options.bDryRun = bDryRun;
        options.iMeshesOptimizationLevel = iMeshesOptimizationLevel;
        options.bMeshesHeadparts = bMeshesHeadparts;
        options.bMeshesResave = bMeshesResave;
        options.bTexturesNecessary = bTexturesNecessary;
        options.bTexturesCompress = bTexturesCompress;
        options.bTexturesMipmaps = bTexturesMipmaps;
        options.bTexturesResizeSize = bTexturesResizeSize;
        options.iTexturesTargetHeight = iTexturesTargetHeight;
        options.iTexturesTargetWidth = iTexturesTargetWidth;
        options.bTexturesResizeRatio = bTexturesResizeRatio;
        options.iTexturesTargetWidthRatio = iTexturesTargetWidthRatio;
        options.iTexturesTargetHeightRatio = iTexturesTargetHeightRatio;
        options.bDebugLog = bDebugLog;
        options.mode = mode;
        options.userPath = userPath;
    }
};

/// Bridges the application backends into one executor-owned evidence and cleanup lifetime.
class ApplicationRunWork final : public RunWorkService {
   public:
    /// Captures all application state while still on the caller thread.
    explicit ApplicationRunWork(const OptionsCAO& options)
        : _options(options), _profile(OptimizerProfileSnapshot::captureIntent()) {}

    /// Reads auxiliary configuration only after start, on the execution thread during Preparing.
    void prepare() override { _profile.loadAuxiliaryFiles(); }

    /// Runs all roots together so routing, precedence, progress and evidence share one lifecycle.
    void execute(const RunPreparation& preparation, RunWorkRecord& record,
                 TemporaryArtifactRegistry& artifacts, RunObservationSink& observations,
                 std::stop_token stop) override {
        OptionsCAO options;
        _options.apply(options);
        options.bDryRun = preparation.policy().executionMode() == routing::ExecutionMode::DryRun;
        std::unique_ptr<MainOptimizer> optimizer;
        std::unique_ptr<BSAOptimizer> archives;
        const auto archiveBackend = [&]() -> BSAOptimizer& {
            if (!archives) archives = std::make_unique<BSAOptimizer>(_profile);
            return *archives;
        };
        std::size_t assetSucceeded = 0;
        std::size_t archiveSucceeded = 0;
        std::size_t publishedDiagnostics = 0;
        std::size_t publishedFailures = 0;
        AssetRunAdapters adapters;
        adapters.isCancelled = [&] { return stop.stop_requested(); };
        adapters.reportPhase = [&](const RunPhaseRecord& phase) {
            observations.recordPhase(phase);
        };
        adapters.reportDiagnostics = [&](const AssetRunDiagnostics& diagnostics) {
            for (const auto& diagnostic : diagnostics.diagnostics()) {
                observations.recordDiagnostic(diagnostic);
                ++publishedDiagnostics;
            }
        };
        adapters.reportDiscoveryFailure = [&](const RunFailure& failure) {
            observations.recordFailure(failure);
            ++publishedFailures;
        };
        adapters.extractArchiveWithResult = [&](const ArchiveExtractionPlan& plan) {
            auto attempt = archiveBackend().extract(plan, options.bBsaDeleteBackup, artifacts);
            attempt.modRoot = plan.modRoot;
            if (attempt.succeeded()) ++archiveSucceeded;
            // Keep completed mutations even if discovery later throws before producing its result.
            record.archiveAttempts.push_back(attempt);
            return attempt;
        };
        adapters.executeAssetWithResult = [&](const routing::RoutedAsset& asset) {
            std::filesystem::path modRoot;
            for (const auto& root : preparation.modRoots()) {
                const auto relative = asset.executionPath().lexically_relative(root);
                if (!relative.empty() && *relative.begin() != "..") {
                    modRoot = root;
                    break;
                }
            }
            if (modRoot.empty())
                throw std::logic_error("Routed Asset is outside prepared Mod Roots");
            if (!optimizer) optimizer = std::make_unique<MainOptimizer>(options, _profile);
            auto attempt = optimizer->process(asset, artifacts, modRoot);
            if (attempt.succeeded()) ++assetSucceeded;
            record.assetAttempts.push_back({modRoot, asset, attempt});
            return attempt;
        };
        adapters.reportProgress = [&](const AssetRunProgress& progress) {
            const bool extraction = progress.phase == routing::RoutedAssetPhase::ArchiveExtraction;
            observations.recordPhase(RunPhaseRecord::executed(
                extraction ? RunPhase::ExtractingArchives : RunPhase::ProcessingAssets,
                RunProgress::determinate(
                    progress.total, extraction ? archiveSucceeded : assetSucceeded,
                    progress.completed - (extraction ? archiveSucceeded : assetSucceeded))));
        };
        adapters.finalizeArchiveLifecycleWithResult = [&] {
            if (options.bBsaCreate) {
                auto plan = archiveBackend().planFinalization(preparation.modRoots(), options);
                auto result = archiveBackend().finalize(
                    plan, artifacts, stop, [&](const ArchiveFinalizationProgress& progress) {
                        observations.recordPhase(RunPhaseRecord::executed(
                            RunPhase::ArchiveFinalization,
                            RunProgress::determinate(progress.total, progress.succeeded,
                                                     progress.failed)));
                    });
                record.finalizations.push_back(result);
                return result;
            }
            ArchiveFinalizationResult result;
            observations.recordPhase(RunPhaseRecord::executed(RunPhase::ArchiveFinalization,
                                                              RunProgress::determinate(0)));
            // Empty-directory pruning is the legacy Apply finalization even without packing.
            // It preserves roots and reserved staging, which belongs to executor cleanup.
            for (const auto& root : preparation.modRoots()) {
                if (stop.stop_requested()) {
                    result.cancelled = true;
                    break;
                }
                FilesystemOperations::deleteEmptyDirectories(
                    QString::fromStdWString(root.wstring()));
            }
            record.finalizations.push_back(result);
            return result;
        };
        auto completed =
            AssetRun(preparation.policy())
                .execute(preparation.modRoots(), adapters, preparation.archivePrecedence())
                .workRecord();
        // Preparing observations already belong to this record; publish newly returned observations
        // through the executor sink exactly once, then transfer the complete work evidence.
        auto diagnostics = std::move(completed.diagnostics);
        auto failures = std::move(completed.failures);
        completed.diagnostics = std::move(record.diagnostics);
        completed.failures = std::move(record.failures);
        record = std::move(completed);
        for (auto index = publishedDiagnostics; index < diagnostics.size(); ++index)
            observations.recordDiagnostic(diagnostics[index]);
        for (auto index = publishedFailures; index < failures.size(); ++index)
            observations.recordFailure(failures[index]);
    }

   private:
    OptionsSnapshot _options;
    OptimizerProfileSnapshot _profile;
};
}  // namespace

std::shared_ptr<RunWorkService> makeApplicationRunWork(const OptionsCAO& options) {
    return std::make_shared<ApplicationRunWork>(options);
}
}  // namespace cao::run
