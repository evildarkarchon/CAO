#include "CliRun.h"
#include "RunTestConfiguration.h"
#include "Run/RunExecutor.h"
#include "Run/RunWorkRecord.h"
#include <QTest>
#include <QProcess>
#include <atomic>
#include <future>
#include <sstream>
#include <thread>
#ifdef _WIN32
#define NOMINMAX
#include <windows.h>
#endif

/// Holds one atomic attempt until the caller confirms repeated cancellation cannot finish it.
class GatedCliWork final : public cao::run::RunWorkService {
   public:
    std::promise<void> entered;
    std::promise<void> release;
    /// Publishes an in-flight attempt, then records completion before observing cancellation.
    void execute(const cao::run::RunPreparation&, cao::run::RunWorkRecord& record,
                 cao::run::TemporaryArtifactRegistry&, cao::run::RunObservationSink& observations,
                 std::stop_token stop) override {
        observations.recordPhase(cao::run::RunPhaseRecord::executed(
            cao::run::RunPhase::ProcessingAssets, cao::run::RunProgress::determinate(2)));
        entered.set_value();
        release.get_future().wait();
        observations.recordPhase(cao::run::RunPhaseRecord::executed(
            cao::run::RunPhase::ProcessingAssets, cao::run::RunProgress::determinate(2, 1)));
        record.cancellationObserved = stop.stop_requested();
    }
};

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
    /// Terminal output retains committed mutations and operation failure details after cancellation.
    void rendersRetainedMutations() {
        using namespace cao::run;
        RunWorkRecord work;
        work.finalizations.push_back(ArchiveFinalizationResult{
            .attempts = {ArchiveFinalizationAttempt{
                .archivePath = "mod/output.bsa",
                .mutation = cao::execution::MutationState::Committed,
                .failure = ArchiveFinalizationFailure::SourceCleanupFailed,
                .safeToContinue = true,
                .detail = "source remains usable",
                .modRoot = "mod"}}});
        auto result = std::make_shared<const OptimizationRunResult>(
            OptimizationRunResult::terminal(RunOutcome::Cancelled, RunPhase::ArchiveFinalization,
                                            {}, "retained", {}, {}, {}, true, &work));
        std::ostringstream output;
        cao::cli::renderEvent(output, RunEvent("retained", 12, result));
        const auto text = output.str();
        QVERIFY(text.find("Cancelled") != std::string::npos);
        QVERIFY(text.find("source remains usable") != std::string::npos);
        QVERIFY(text.find("Committed Mutations Retained|mod|Archive Finalization|1") !=
                std::string::npos);
        QVERIFY(text.find("Cancellation Observed|yes") != std::string::npos);
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
