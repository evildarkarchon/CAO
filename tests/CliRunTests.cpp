#include "CliRun.h"
#include "RunTestConfiguration.h"
#include "Run/RunExecutor.h"
#include "Run/AssetRun.h"
#include "Run/ArchiveExtraction.h"
#include "Run/ArchiveFinalizationResult.h"
#include "AssetRouting/AssetRouter.h"
#include <QTest>
#include <QProcess>
#include <array>
#include <atomic>
#include <future>
#include <sstream>
#include <thread>
#include <tuple>
#ifdef _WIN32
#define NOMINMAX
#include <windows.h>
#endif

/// Holds one atomic attempt until the caller confirms repeated cancellation cannot finish it.
class GatedCliWork final : public cao::run::RunWorkService {
   public:
    std::promise<void> entered;
    std::promise<void> release;
    /// Publishes an in-flight attempt, then records completion before the executor observes
    /// cancellation.
    void execute(const cao::run::RunPreparation&, cao::run::RunWorkRecord&,
                 cao::run::MutableRunEvidence&,
                 cao::run::TemporaryArtifactRegistry&, cao::run::RunObservationSink& observations,
                 std::stop_token) override {
        observations.recordPhase(cao::run::RunPhaseRecord::executed(
            cao::run::RunPhase::ProcessingAssets, cao::run::RunProgress::determinate(2)));
        entered.set_value();
        release.get_future().wait();
        observations.recordPhase(cao::run::RunPhaseRecord::executed(
            cao::run::RunPhase::ProcessingAssets, cao::run::RunProgress::determinate(2, 1)));
    }
};

namespace {
enum class TerminalScenario { ContainedFailure, Cancelled, Unsafe, CleanupFailure };

/// Submits terminal facts through the executor while leaving its transitional work record empty.
class TerminalCliWork final : public cao::run::RunWorkService {
   public:
    /// Borrows cancellation only until the synchronous work call completes.
    TerminalCliWork(TerminalScenario scenario, std::stop_source& cancellation)
        : _scenario(scenario), _cancellation(cancellation) {}

    /// Records completed Archive and Asset attempts before their terminal classification.
    void execute(const cao::run::RunPreparation& preparation, cao::run::RunWorkRecord&,
                 cao::run::MutableRunEvidence& evidence,
                 cao::run::TemporaryArtifactRegistry&,
                 cao::run::RunObservationSink& observations, std::stop_token) override {
        using namespace cao::run;
        using cao::execution::MutationState;
        const auto root = preparation.modRoots().front();
        observations.archiveDiscoveryStarted();
        const std::array collisions{ArchiveCollision(root, "textures/a.dds", root / "winner.bsa",
                                                       {root / "shadowed.bsa"}, true)};
        evidence.recordArchiveCollisions(collisions);
        evidence.recordArchiveDiscovery(ArchiveDiscoveryEvidence({}, {}, 0));
        observations.archiveExtractionPlanned(1);
        if (_scenario == TerminalScenario::CleanupFailure)
            evidence.recordArchiveExtractionAttempt(
                {root / "input.bsa", MutationState::Committed, {}, true, {}, root}, 1);
        else
            evidence.recordArchiveExtractionAttempt(
                {root / "input.bsa", MutationState::Committed,
                 ArchiveExtractionFailure::SourceCleanupFailed, true, "extracted source remains",
                 root},
                1);
        observations.effectiveAssetTreeStarted();
        const cao::routing::AssetRouter router(preparation.policy());
        const std::vector paths{root / "failed.dds"};
        auto ledger = router.route(paths);
        // The attempt needs its routed identity after evidence takes ownership of the ledger.
        const auto asset = ledger.routedAssets().front();
        evidence.recordRoutingLedger(std::move(ledger));
        observations.assetProcessingPlanned(1);
        const auto result =
            _scenario == TerminalScenario::CleanupFailure
                ? cao::execution::AssetExecutionResult::success(MutationState::Committed)
                : cao::execution::AssetExecutionResult::failed(
                      cao::execution::AssetExecutionFailure::SaveFailed, "asset save failed",
                      _scenario == TerminalScenario::Unsafe ? MutationState::PartialOrUnknown
                                                            : MutationState::Committed,
                      _scenario != TerminalScenario::Unsafe, root / "staged-output.dds", "save_texture",
                      "backend detail");
        evidence.recordAssetAttempt({root, asset, result}, 1);
        if (_scenario == TerminalScenario::Unsafe) {
            observations.recordFailure(RunFailure(RunFailureCode::WorkServiceFailed,
                                                  RunPhase::ProcessingAssets, "primary failure",
                                                  {}, root / "fatal.bsa"));
            return;
        }
        observations.archiveFinalizationAvailable(cao::routing::ExecutionMode::Apply, true);
        evidence.recordArchiveFinalizationPlan(1);
        evidence.recordArchiveFinalization(ArchiveFinalizationResult{
            .attempts = {ArchiveFinalizationAttempt{
                .archivePath = root / "output.bsa",
                .mutation = MutationState::Committed,
                .failure = _scenario == TerminalScenario::CleanupFailure
                               ? std::optional<ArchiveFinalizationFailure>{}
                               : ArchiveFinalizationFailure::SourceCleanupFailed,
                .safeToContinue = true,
                .detail = _scenario == TerminalScenario::CleanupFailure ? "" : "source remains usable",
                .modRoot = root}}});
        if (_scenario == TerminalScenario::Cancelled) _cancellation.request_stop();
    }

   private:
    TerminalScenario _scenario;
    std::stop_source& _cancellation;
};

/// Adds a final Safety Cleanup failure only to the cleanup scenario.
class TerminalCliCleanup final : public cao::run::SafetyCleanupService {
   public:
    /// Chooses whether this run leaves one artifact after its final cleanup pass.
    explicit TerminalCliCleanup(bool fail) : _fail(fail) {}

    /// Reports the artifact that the mandatory cleanup pass could not remove.
    std::vector<cao::run::RunFailure> performSafetyCleanup() override {
        if (!_fail) return {};
        return {cao::run::RunFailure(cao::run::RunFailureCode::TemporaryArtifactCleanupFailed,
                                     cao::run::RunPhase::SafetyCleanup, "cleanup failure", {},
                                     "staging.tmp")};
    }

   private:
    bool _fail;
};

/// Requests all work needed to exercise the CLI's focused terminal evidence categories.
cao::run::RunRequest terminalCliRequest() {
    return cao::run::RunRequest::create(
        "SkyrimSE", cao::routing::ExecutionMode::Apply,
        cao::run::ModSelection::singleModRoot(testModRoot()),
        {cao::routing::RequestedWork::ArchiveExtraction,
         cao::routing::RequestedWork::NativeTextureOptimization,
         cao::routing::RequestedWork::ArchiveCreation});
}
}  // namespace

class CliRunTests final : public QObject {
    Q_OBJECT
   private slots:
    /// Exercises actual repeated Windows Ctrl+C delivery in an isolated hidden console.
    void consoleInterruptsRemainCooperative() {
#ifdef _WIN32
        QProcess process;
        process.setCreateProcessArgumentsModifier([](QProcess::CreateProcessArguments* arguments) {
            arguments->flags |= CREATE_NEW_CONSOLE;
            arguments->startupInfo->dwFlags |= STARTF_USESHOWWINDOW;
            arguments->startupInfo->wShowWindow = SW_HIDE;
        });
        process.start(QCoreApplication::applicationFilePath(), {"--interrupt-probe"});
        QVERIFY(process.waitForStarted());
        QVERIFY(process.waitForFinished(10000));
        QCOMPARE(process.exitStatus(), QProcess::NormalExit);
        QCOMPARE(process.exitCode(), 0);
#endif
    }
    /// Checks shell status against the four public outcomes and synchronous rejection.
    void exitCodes() {
        using cao::run::RunOutcome;
        QCOMPARE(cao::cli::exitCode(RunOutcome::Succeeded), 0);
        QCOMPARE(cao::cli::exitCode(RunOutcome::CompletedWithFailures), 1);
        QCOMPARE(cao::cli::exitCode(RunOutcome::Failed), 2);
        QCOMPARE(cao::cli::exitCode(RunOutcome::Cancelled), 130);
        cao::run::OptimizationRunService service(testRunConfiguration());
        auto output = std::make_shared<std::ostringstream>();
        const auto request =
            cao::run::RunRequest::create("", cao::routing::ExecutionMode::DryRun,
                                         cao::run::ModSelection::singleModRoot(testModRoot()), {});
        QCOMPARE(cao::cli::run(service, request, output, [] { return false; }), 2);
        QVERIFY(output->str().find("Start Error") != std::string::npos);
    }
    /// Renders the event's exact counters, including failures, without inventing completion.
    void rendersOrderedEvents() {
        using namespace cao::run;
        std::ostringstream output;
        cao::cli::renderEvent(
            output, RunEvent("run-42", 7,
                             RunPhaseRecord::executed(RunPhase::ProcessingAssets,
                                                      RunProgress::determinate(9, 2, 1))));
        cao::cli::renderEvent(output,
                              RunEvent("run-42", 8,
                                       RunPhaseRecord::skipped(RunPhase::ArchiveFinalization,
                                                               PhaseSkipReason::NoRequestedWork)));
        cao::cli::renderEvent(
            output, RunEvent("run-42", 9, RunPhaseRecord::executed(RunPhase::SafetyCleanup)));
        const auto text = output.str();
        QVERIFY(text.find("run-42|7") < text.find("run-42|8"));
        QVERIFY(text.find("run-42|8") < text.find("run-42|9"));
        QVERIFY(text.find("PROGRESS:|Processing Assets|3|9|succeeded=2|failed=1") !=
                std::string::npos);
        QVERIFY(text.find("Archive Finalization|Skipped|No Requested Work") != std::string::npos);
        QVERIFY(text.find("Safety Cleanup|Indeterminate") != std::string::npos);
        QVERIFY(text.find("|9|9") == std::string::npos);
    }
    /// Repeated interrupt intent cannot end an atomic attempt; cleanup precedes Cancelled output.
    void waitsThroughCancellation() {
        auto work = std::make_shared<GatedCliWork>();
        cao::run::OptimizationRunService service(testRunConfiguration(), work);
        auto output = std::make_shared<std::ostringstream>();
        std::atomic<int> interrupts{};
        std::atomic<bool> requested{};
        auto request =
            cao::run::RunRequest::create("test", cao::routing::ExecutionMode::DryRun,
                                         cao::run::ModSelection::singleModRoot(testModRoot()),
                                         {cao::routing::RequestedWork::NativeTextureOptimization});
        auto finished = std::async(std::launch::async, [&] {
            return cao::cli::run(service, request, output, [&] {
                if (!requested.load()) return false;
                ++interrupts;
                return true;
            });
        });
        const auto entered = work->entered.get_future().wait_for(std::chrono::seconds(5));
        requested = true;
        const auto waiting = finished.wait_for(std::chrono::milliseconds(100));
        work->release.set_value();
        QCOMPARE(entered, std::future_status::ready);
        QCOMPARE(waiting, std::future_status::timeout);
        QCOMPARE(finished.get(), 130);
        QVERIFY(interrupts.load() > 1);
        const auto text = output->str();
        QVERIFY(text.find("|1|2|succeeded=1") != std::string::npos);
        QVERIFY(text.find("Safety Cleanup") < text.find("Cancelled"));
        QVERIFY(text.find("|2|2") == std::string::npos);
    }
    /// Every terminal classification renders its sealed facts without a populated work record.
    void rendersFocusedTerminalEvidence() {
        using namespace cao::run;
        for (const auto& [scenario, expectedOutcome, label] :
             {std::tuple{TerminalScenario::ContainedFailure, RunOutcome::CompletedWithFailures,
                         "Completed With Failures"},
              {TerminalScenario::Cancelled, RunOutcome::Cancelled, "Cancelled"},
              {TerminalScenario::Unsafe, RunOutcome::Failed, "Failed"},
              {TerminalScenario::CleanupFailure, RunOutcome::CompletedWithFailures,
               "Completed With Failures"}}) {
            std::stop_source cancellation;
            TerminalCliWork work(scenario, cancellation);
            TerminalCliCleanup cleanup(scenario == TerminalScenario::CleanupFailure);
            const auto result = std::make_shared<const OptimizationRunResult>(
                RunExecutor().execute(
                    terminalCliRequest(),
                    RunServices{cleanup, nullptr, testRunConfiguration().get(), &work},
                    cancellation.get_token(), "terminal"));
            QCOMPARE(result->outcome(), expectedOutcome);
            std::ostringstream output;
            cao::cli::renderEvent(output, RunEvent("terminal", 12, result));
            const auto text = output.str();
            QVERIFY2(text.find(label) != std::string::npos, label);
            const auto root = testModRoot().generic_string();
            QVERIFY(text.find("Archive Collision|" + root + "|textures/a.dds") !=
                    std::string::npos);
            QVERIFY(text.find("winner.bsa") != std::string::npos);
            QVERIFY(text.find("shadowed.bsa") != std::string::npos);
            QVERIFY(text.find("loose-asset-wins=yes") != std::string::npos);
            QVERIFY(text.find("Committed Mutations Retained|" + root +
                              "|Archive Extraction|1|partial-or-unknown=0") != std::string::npos);
            if (scenario == TerminalScenario::CleanupFailure) {
                QVERIFY(text.find("Cleanup Failure|cleanup failure|staging.tmp") !=
                        std::string::npos);
                QVERIFY(text.find("Committed Mutations Retained|" + root +
                                  "|Asset Processing|1|partial-or-unknown=0") !=
                        std::string::npos);
                continue;
            }
            QVERIFY(text.find("Archive Failure|") != std::string::npos);
            QVERIFY(text.find("extracted source remains") != std::string::npos);
            QVERIFY(text.find("Asset Failure|") != std::string::npos);
            QVERIFY(text.find("save_texture") != std::string::npos);
            QVERIFY(text.find("staged-output.dds") != std::string::npos);
            QVERIFY(text.find("backend detail") != std::string::npos);
            if (scenario == TerminalScenario::Unsafe) {
                QVERIFY(text.find("Run Failure|primary failure") != std::string::npos);
                QVERIFY(text.find("fatal.bsa") != std::string::npos);
                QVERIFY(text.find("Committed Mutations Retained|" + root +
                                  "|Asset Processing|0|partial-or-unknown=1") !=
                        std::string::npos);
            } else {
                QVERIFY(text.find("source remains usable") != std::string::npos);
                QVERIFY(text.find("Committed Mutations Retained|" + root +
                                  "|Archive Finalization|1|partial-or-unknown=0") !=
                        std::string::npos);
            }
            if (scenario == TerminalScenario::Cancelled)
                QVERIFY(text.find("Cancellation Observed|yes") != std::string::npos);
        }
    }
    /// The CLI observer renders the exact immutable result committed to the Run Handle.
    void rendersCommittedTerminalResult() {
        using namespace cao::run;
        std::stop_source cancellation;
        auto work = std::make_shared<TerminalCliWork>(TerminalScenario::ContainedFailure,
                                                      cancellation);
        OptimizationRunService service(testRunConfiguration(), work);
        std::ostringstream output;
        std::promise<const OptimizationRunResult*> observed;
        auto observedResult = observed.get_future();
        auto started = service.start(
            terminalCliRequest(),
            [&](const RunEvent& event) {
                cao::cli::renderEvent(output, event);
                if (const auto* result =
                        std::get_if<std::shared_ptr<const OptimizationRunResult>>(&event.payload()))
                    observed.set_value(result->get());
            });
        QVERIFY(started.started());
        auto handle = std::move(*started.handle());
        const auto* eventResult = observedResult.get();
        QCOMPARE(eventResult, handle.terminalResult());
        QCOMPARE(eventResult, &handle.wait());
        QVERIFY(output.str().find("Asset Failure|") != std::string::npos);
        QVERIFY(output.str().find("Committed Mutations Retained|") != std::string::npos);
    }
};
/// Runs native interrupt checks outside the test runner's console so Ctrl+C cannot escape the test.
int main(int argc, char** argv) {
    QCoreApplication application(argc, argv);
#ifdef _WIN32
    if (application.arguments().contains("--interrupt-probe")) {
        cao::cli::ConsoleInterrupt interruption;
        if (!GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0)) return 10;
        const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
        while (!interruption.requested() && std::chrono::steady_clock::now() < deadline)
            std::this_thread::sleep_for(std::chrono::milliseconds(10));
        if (!interruption.requested()) return 11;
        if (!GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0)) return 12;
        std::this_thread::sleep_for(std::chrono::milliseconds(100));
        return interruption.requested() ? 0 : 13;
    }
#endif
    CliRunTests tests;
    return QTest::qExec(&tests, argc, argv);
}
#include "CliRunTests.moc"
