#include "GuiRun.h"
#include "RunTestConfiguration.h"
#include "Run/RunExecutor.h"
#include "Run/AssetRun.h"
#include "Run/ArchiveExtraction.h"
#include "Run/ArchiveFinalizationResult.h"
#include "Run/TemporaryArtifactRegistry.h"
#include "AssetRouting/AssetRouter.h"
#include <QTest>

#include <array>
#include <tuple>

namespace {
enum class ClassificationScenario { AllSuccessful, ContainedFailure, Cancelled, UnsafeFailure };

/// Completes three routed attempts, leaving five pending only when cancellation is requested.
cao::run::OptimizationRunResult classifiedRun(const ClassificationScenario scenario) {
    using namespace cao::run;
    class Work final : public RunWorkService {
       public:
        /// Borrows cancellation intent until synchronous execution has returned.
        Work(const ClassificationScenario scenario, std::stop_source& cancellation)
            : _scenario(scenario), _cancellation(cancellation) {}

        /// Submits completed attempts to the executor without manufacturing a terminal result.
        void execute(const RunPreparation& preparation, RunWorkEvidence& evidence,
                     TemporaryArtifactRegistry&,
                     RunWorkMilestones& observations, std::stop_token) override {
            const auto root = preparation.modRoots().front();
            observations.archiveDiscoveryStarted();
            evidence.recordArchiveDiscovery(ArchiveDiscoveryEvidence({}, {}, 0));
            observations.dryRunArchiveExtraction();
            observations.effectiveAssetTreeStarted();
            const cao::routing::AssetRouter router(preparation.policy());
            std::vector<std::filesystem::path> paths;
            const std::size_t total = _scenario == ClassificationScenario::Cancelled ? 8 : 3;
            for (std::size_t index = 0; index < total; ++index)
                paths.push_back(root / ("asset-" + std::to_string(index) + ".dds"));
            auto ledger = router.route(paths);
            // Attempts need routed values after the evidence owner consumes the definitive ledger.
            const auto assets = std::vector<cao::routing::RoutedAsset>(
                ledger.routedAssets().begin(), ledger.routedAssets().end());
            evidence.recordRoutingLedger(std::move(ledger));
            observations.assetProcessingPlanned(total);
            evidence.recordAssetAttempt(
                {root, assets[0], cao::execution::AssetExecutionResult::success()}, total);
            evidence.recordAssetAttempt(
                {root, assets[1], cao::execution::AssetExecutionResult::success()}, total);
            const bool failed = _scenario == ClassificationScenario::ContainedFailure ||
                                _scenario == ClassificationScenario::UnsafeFailure;
            const auto third = failed ? cao::execution::AssetExecutionResult::failed(
                                            cao::execution::AssetExecutionFailure::SaveFailed,
                                            "third attempt failed",
                                            _scenario == ClassificationScenario::UnsafeFailure
                                                ? cao::execution::MutationState::PartialOrUnknown
                                                : cao::execution::MutationState::None)
                                      : cao::execution::AssetExecutionResult::success();
            evidence.recordAssetAttempt({root, assets[2], third}, total);
            if (_scenario == ClassificationScenario::Cancelled) _cancellation.request_stop();
        }

       private:
        ClassificationScenario _scenario;
        std::stop_source& _cancellation;
    };
    std::stop_source cancellation;
    Work work(scenario, cancellation);
    TemporaryArtifactRegistry cleanup;
    return RunExecutor().execute(
        RunRequest::create("SkyrimSE", cao::routing::ExecutionMode::DryRun,
                           ModSelection::singleModRoot(testModRoot()),
                           {cao::routing::RequestedWork::NativeTextureOptimization}),
        RunServices{cleanup, nullptr, testRunConfiguration().get(), &work},
        cancellation.get_token(), "run");
}
}  // namespace

class GuiRunTests final : public QObject {
    Q_OBJECT
   private slots:
    /// GUI strings are UTF-8 even when a Windows path cannot be encoded in the ANSI code page.
    void preservesUnicodePaths() {
        using namespace cao::run;
        cao::gui::RunViewModel view;
        view.begin("run");
        const std::filesystem::path path(u8"mods/\u65e5\u672c/file.bsa");
        QVERIFY(view.consume(
            RunEvent("run", 1,
                     RunFailure(RunFailureCode::WorkServiceFailed, RunPhase::ExtractingArchives,
                                "archive failure", {}, path))));
        QVERIFY(view.state().details.front().find("mods/\xe6\x97\xa5\xe6\x9c\xac/file.bsa") !=
                std::string::npos);
    }
    /// Failed attempts count as completed while skipped and cleanup phases remain indeterminate.
    void presentsAuthoritativeProgress() {
        using namespace cao::run;
        cao::gui::RunViewModel view;
        view.begin("run");
        QVERIFY(
            view.consume(RunEvent("run", 1,
                                  RunPhaseRecord::executed(RunPhase::ProcessingAssets,
                                                           RunProgress::determinate(9, 2, 1)))));
        QCOMPARE(view.state().label, std::string("Processing Assets"));
        QVERIFY(view.state().progress.has_value());
        QCOMPARE(view.state().progress->completed(), std::size_t(3));
        QCOMPARE(view.state().progress->total(), std::size_t(9));
        QVERIFY(view.consume(RunEvent(
            "run", 2,
            RunPhaseRecord::skipped(RunPhase::ArchiveFinalization, PhaseSkipReason::DryRun))));
        QVERIFY(!view.state().progress);
        QCOMPARE(view.state().label, std::string("Archive Finalization - Skipped (Dry Run)"));
        QVERIFY(
            view.consume(RunEvent("run", 3, RunPhaseRecord::executed(RunPhase::SafetyCleanup))));
        QCOMPARE(view.state().label, std::string("Safety Cleanup"));
        QVERIFY(view.state().active);
        QVERIFY(!view.state().outcome);
    }
    /// Cancellation is intent until terminal delivery and only success is presented as Done.
    void presentsTerminalClassification() {
        using namespace cao::run;
        for (const auto& [scenario, outcome, label] :
             {std::tuple{ClassificationScenario::AllSuccessful, RunOutcome::Succeeded, "Done"},
              {ClassificationScenario::ContainedFailure, RunOutcome::CompletedWithFailures,
               "Completed With Failures"},
              {ClassificationScenario::Cancelled, RunOutcome::Cancelled, "Cancelled"},
              {ClassificationScenario::UnsafeFailure, RunOutcome::Failed, "Failed"}}) {
            const std::size_t total = scenario == ClassificationScenario::Cancelled ? 8 : 3;
            cao::gui::RunViewModel view;
            view.begin("run");
            QVERIFY(
                view.consume(RunEvent("run", 1,
                                      RunPhaseRecord::executed(RunPhase::ProcessingAssets,
                                                               RunProgress::determinate(total)))));
            view.requestCancellation();
            view.requestCancellation();
            QVERIFY(view.state().active);
            QVERIFY(view.state().cancellationRequested);
            QCOMPARE(view.state().label, std::string("Cancelling - Processing Assets"));
            QVERIFY(!view.state().outcome);
            QCOMPARE(view.state().progress->completed(), std::size_t(0));
            auto result = std::make_shared<const OptimizationRunResult>(classifiedRun(scenario));
            QCOMPARE(result->outcome(), outcome);
            QVERIFY(view.consume(RunEvent("run", 2, result)));
            QCOMPARE(view.state().label, std::string(label));
            QCOMPARE(view.state().outcome, std::optional(outcome));
            QVERIFY(!view.state().active);
            QCOMPARE(view.state().progress->completed(), std::size_t(3));
            QCOMPARE(view.state().progress->total(), total);
            view.requestCancellation();
            QCOMPARE(view.state().label, std::string(label));
        }
    }
    /// Closing keeps the window alive until the service's cleanup and terminal observation finish.
    void defersCloseUntilTerminal() {
        using namespace cao::run;
        cao::gui::RunViewModel view;
        QVERIFY(view.requestClose());
        QVERIFY(view.canStart());
        view.begin("run");
        QVERIFY(!view.canStart());
        QVERIFY(!view.requestClose());
        QVERIFY(view.state().closeRequested);
        QVERIFY(view.state().cancellationRequested);
        QVERIFY(view.state().active);
        QVERIFY(
            view.consume(RunEvent("run", 1, RunPhaseRecord::executed(RunPhase::SafetyCleanup))));
        QVERIFY(!view.requestClose());
        auto result = std::make_shared<const OptimizationRunResult>(
            classifiedRun(ClassificationScenario::Cancelled));
        QCOMPARE(result->outcome(), RunOutcome::Cancelled);
        QVERIFY(view.consume(RunEvent("run", 2, result)));
        QVERIFY(view.requestClose());
        QVERIFY(view.state().closeRequested);
        QVERIFY(!view.canStart());
    }
    /// Old runs and duplicate events cannot overwrite a new run, including delayed terminal events.
    void rejectsStaleObservations() {
        using namespace cao::run;
        cao::gui::RunViewModel view;
        view.begin("new");
        QCOMPARE(view.state().label, std::string("Preparing"));
        QVERIFY(
            !view.consume(RunEvent("old", 9, RunPhaseRecord::executed(RunPhase::SafetyCleanup))));
        QVERIFY(
            view.consume(RunEvent("new", 2, RunPhaseRecord::executed(RunPhase::ProcessingAssets))));
        QVERIFY(!view.consume(RunEvent("new", 1, RunPhaseRecord::executed(RunPhase::Preparing))));
        QVERIFY(!view.consume(RunEvent("new", 2, RunPhaseRecord::executed(RunPhase::Preparing))));
        QCOMPARE(view.state().label, std::string("Processing Assets"));
    }
    /// Failed work presents sealed attempts, collision winners, cleanup, and durable mutations.
    void presentsTerminalEvidence() {
        using namespace cao::run;
        using cao::execution::MutationState;
        class EvidenceOnlyWork final : public RunWorkService {
           public:
            /// Submits completed work directly to the executor's evidence owner.
            void execute(const RunPreparation& preparation, RunWorkEvidence& evidence,
                         TemporaryArtifactRegistry&,
                         RunWorkMilestones& observations, std::stop_token) override {
                const auto root = preparation.modRoots().front();
                observations.archiveDiscoveryStarted();
                const std::array collisions{ArchiveCollision(
                    root, "textures/a.dds", root / "winner.bsa", {root / "shadowed.bsa"}, true)};
                evidence.recordArchiveCollisions(collisions);
                evidence.recordArchiveDiscovery(ArchiveDiscoveryEvidence({}, {}, 0));
                observations.archiveExtractionPlanned(2);
                evidence.recordArchiveExtractionAttempt(
                    {root / "committed.bsa", MutationState::Committed, {}, true, {}, root}, 2);
                evidence.recordArchiveExtractionAttempt(
                    {root / "input.bsa", MutationState::PartialOrUnknown,
                     ArchiveExtractionFailure::MergeFailed, false, "merge interrupted", root},
                    2);
                observations.effectiveAssetTreeStarted();
                const cao::routing::AssetRouter router(preparation.policy());
                const std::vector paths{root / "failed.dds"};
                auto ledger = router.route(paths);
                // The attempt still needs its route after evidence takes ownership of the ledger.
                const auto asset = ledger.routedAssets().front();
                evidence.recordRoutingLedger(std::move(ledger));
                observations.assetProcessingPlanned(1);
                evidence.recordAssetAttempt(
                    {root, asset,
                     cao::execution::AssetExecutionResult::failed(
                         cao::execution::AssetExecutionFailure::SaveFailed, "asset save failed",
                         MutationState::Committed, true, root / "failed.dds", "save_texture",
                         "backend detail")},
                    1);
                observations.archiveFinalizationAvailable(cao::routing::ExecutionMode::Apply, true);
                evidence.recordArchiveFinalizationPlan(5);
                evidence.recordArchiveFinalization(ArchiveFinalizationResult{
                    .attempts = {ArchiveFinalizationAttempt{
                                     .archivePath = root / "output.bsa",
                                     .mutation = MutationState::Committed,
                                     .failure = ArchiveFinalizationFailure::SourceCleanupFailed,
                                     .safeToContinue = true,
                                     .detail = "source remains usable",
                                     .modRoot = root},
                                 ArchiveFinalizationAttempt{.archivePath = root / "output-two.bsa",
                                                            .modRoot = root}}});
                evidence.recordFailure(RunFailure(RunFailureCode::WorkServiceFailed,
                                                  RunPhase::ArchiveFinalization,
                                                  "primary failure"));
            }
        } work;
        class FailingCleanup final : public SafetyCleanupService {
           public:
            /// Reports a remaining temporary artifact from the mandatory cleanup pass.
            std::vector<RunFailure> performSafetyCleanup() override {
                return {RunFailure(RunFailureCode::TemporaryArtifactCleanupFailed,
                                   RunPhase::SafetyCleanup, "cleanup failure", {}, "staging.tmp")};
            }
        } cleanup;
        const auto root = testModRoot();
        const auto result = std::make_shared<const OptimizationRunResult>(RunExecutor().execute(
            RunRequest::create("SkyrimSE", cao::routing::ExecutionMode::Apply,
                               ModSelection::singleModRoot(root),
                               {cao::routing::RequestedWork::ArchiveExtraction,
                                cao::routing::RequestedWork::NativeTextureOptimization,
                                cao::routing::RequestedWork::ArchiveCreation}),
            RunServices{cleanup, nullptr, testRunConfiguration().get(), &work}, {}, "run"));
        QCOMPARE(result->outcome(), RunOutcome::Failed);
        cao::gui::RunViewModel view;
        view.begin("run");
        QVERIFY(
            view.consume(RunEvent("run", 1, RunPhaseRecord::executed(RunPhase::SafetyCleanup))));
        QVERIFY(view.consume(RunEvent("run", 2, result->failures().front())));
        QVERIFY(view.consume(RunEvent("run", 3, result)));
        std::string details;
        for (const auto& line : view.state().details) details += line + '\n';
        std::size_t primaryFailures = 0;
        for (const auto& line : view.state().details)
            if (line.find("Failure: primary failure") != std::string::npos) ++primaryFailures;
        QCOMPARE(primaryFailures, std::size_t{1});
        for (const auto* expected :
             {"primary failure", "cleanup failure", "staging.tmp", "source remains usable",
              "merge interrupted", "asset save failed", "save_texture", "backend detail",
              "winner.bsa", "shadowed.bsa", "loose-asset-wins=yes", "Committed Mutations Retained",
              "committed=1", "partial-or-unknown=1", "Cancellation Observed: no"})
            QVERIFY2(details.find(expected) != std::string::npos, expected);
        QVERIFY(view.state().progress);
        QCOMPARE(view.state().progress->completed(), std::size_t(2));
        QCOMPARE(view.state().progress->total(), std::size_t(5));
        const auto terminalLabel = view.state().label;
        QVERIFY(view.consume(RunEvent("run", 4,
                                      RunDiagnostic(RunDiagnosticCode::ObserverFailed,
                                                    RunPhase::SafetyCleanup, "late diagnostic"))));
        QCOMPARE(view.state().label, terminalLabel);
        QVERIFY(view.state().details.back().find("late diagnostic") != std::string::npos);
    }
};
QTEST_APPLESS_MAIN(GuiRunTests)
#include "GuiRunTests.moc"
