#include "ApplicationRunWork.h"

#include "BsaOptimizer.h"
#include "MainOptimizer.h"
#include "OptimizerProfileSnapshot.h"
#include "Run/AssetRun.h"
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
    void execute(const RunPreparation& preparation, RunWorkEvidence& evidence,
                 TemporaryArtifactRegistry& artifacts,
                 RunWorkMilestones& milestones, std::stop_token stop) override {
        OptionsCAO options;
        _options.apply(options);
        options.bDryRun = preparation.policy().executionMode() == routing::ExecutionMode::DryRun;
        std::unique_ptr<MainOptimizer> optimizer;
        std::unique_ptr<BSAOptimizer> archives;
        const auto archiveBackend = [&]() -> BSAOptimizer& {
            if (!archives) archives = std::make_unique<BSAOptimizer>(_profile);
            return *archives;
        };
        AssetRunAdapters adapters;
        adapters.extractArchiveWithResult = [&](const ArchiveExtractionPlan& plan) {
            return archiveBackend().extract(plan, options.bBsaDeleteBackup, artifacts);
        };
        adapters.executeAssetWithResult = [&](const routing::RoutedAsset& asset,
                                              const std::filesystem::path& modRoot) {
            if (!optimizer) optimizer = std::make_unique<MainOptimizer>(options, _profile);
            return optimizer->process(asset, artifacts, modRoot);
        };
        adapters.finalizeArchiveLifecycleWithResult = [&] {
            if (options.bBsaCreate) {
                try {
                    auto plan = archiveBackend().planFinalization(preparation.modRoots(), options,
                                                                   stop);
                    const auto total = plan.outputs().size();
                    evidence.recordArchiveFinalizationPlan(total);
                    return archiveBackend().finalize(
                        plan, artifacts, stop, {}, availableArchiveCapacity,
                        [&](const ArchiveFinalizationAttempt& attempt) {
                            // Evidence owns each atomic result before its progress event is published.
                            evidence.recordArchiveFinalizationAttempt(attempt, total);
                        });
                } catch (const ArchiveFinalizationPlanningCancelled&) {
                    // Planning made no mutations or trustworthy output total before cancellation.
                    return ArchiveFinalizationResult{.cancelled = true};
                }
            }
            ArchiveFinalizationResult result;
            evidence.recordArchiveFinalizationPlan(0);
            // Empty-directory pruning is the legacy Apply finalization even without packing.
            // It preserves roots and reserved staging, which belongs to executor cleanup.
            for (const auto& root : preparation.modRoots()) {
                if (stop.stop_requested()) {
                    result.cancelled = true;
                    break;
                }
                const auto removed = FilesystemOperations::deleteEmptyDirectories(
                    QString::fromStdWString(root.wstring()));
                if (removed != 0)
                    result.mutations.push_back(ArchiveFinalizationMutation{
                        .modRoot = root,
                        .path = root,
                        .kind = ArchiveFinalizationMutationKind::EmptyDirectoryPruning,
                        .mutation = execution::MutationState::Committed,
                        .count = removed});
            }
            return result;
        };
        executeAssetRun(preparation, evidence, milestones, stop, adapters);
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
