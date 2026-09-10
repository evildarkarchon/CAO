#include "GuiRun.h"
#include "Run/RunWorkRecord.h"
#include <QTest>

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
        for (const auto& [outcome, label] :
             {std::pair{RunOutcome::Succeeded, "Done"},
              {RunOutcome::CompletedWithFailures, "Completed With Failures"},
              {RunOutcome::Cancelled, "Cancelled"},
              {RunOutcome::Failed, "Failed"}}) {
            cao::gui::RunViewModel view;
            view.begin("run");
            QVERIFY(view.consume(
                RunEvent("run", 1,
                         RunPhaseRecord::executed(RunPhase::ProcessingAssets,
                                                  RunProgress::determinate(8, 2, 1)))));
            view.requestCancellation();
            view.requestCancellation();
            QVERIFY(view.state().active);
            QVERIFY(view.state().cancellationRequested);
            QCOMPARE(view.state().label, std::string("Cancelling - Processing Assets"));
            QVERIFY(!view.state().outcome);
            QCOMPARE(view.state().progress->completed(), std::size_t(3));
            auto result = std::make_shared<const OptimizationRunResult>(
                OptimizationRunResult::terminal(outcome, RunPhase::ProcessingAssets, {}, "run"));
            QVERIFY(view.consume(RunEvent("run", 2, result)));
            QCOMPARE(view.state().label, std::string(label));
            QCOMPARE(view.state().outcome, std::optional(outcome));
            QVERIFY(!view.state().active);
            QCOMPARE(view.state().progress->completed(), std::size_t(3));
            view.requestCancellation();
            QCOMPARE(view.state().label, std::string(label));
        }
    }
    /// Closing keeps the window alive until the service's cleanup and terminal observation finish.
    void defersCloseUntilTerminal() {
        using namespace cao::run;
        cao::gui::RunViewModel view;
        QVERIFY(view.requestClose());
        view.begin("run");
        QVERIFY(!view.requestClose());
        QVERIFY(view.state().closeRequested);
        QVERIFY(view.state().cancellationRequested);
        QVERIFY(view.state().active);
        QVERIFY(
            view.consume(RunEvent("run", 1, RunPhaseRecord::executed(RunPhase::SafetyCleanup))));
        QVERIFY(!view.requestClose());
        auto result = std::make_shared<const OptimizationRunResult>(OptimizationRunResult::terminal(
            RunOutcome::Cancelled, RunPhase::ProcessingAssets, {}, "run"));
        QVERIFY(view.consume(RunEvent("run", 2, result)));
        QVERIFY(view.requestClose());
        QVERIFY(view.state().closeRequested);
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
    /// Cancellation retains independently inspectable failures, collision winners, and mutations.
    void presentsTerminalEvidence() {
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
        work.archiveAttempts.push_back(
            ArchiveExtractionResult{.archivePath = "mod/input.bsa",
                                    .mutation = cao::execution::MutationState::PartialOrUnknown,
                                    .failure = ArchiveExtractionFailure::MergeFailed,
                                    .safeToContinue = false,
                                    .detail = "merge interrupted",
                                    .modRoot = "mod"});
        work.collisions.emplace_back("mod", "textures/a.dds", "winner.bsa",
                                     std::vector<std::filesystem::path>{"shadowed.bsa"}, true);
        auto result = std::make_shared<const OptimizationRunResult>(OptimizationRunResult::terminal(
            RunOutcome::Cancelled, RunPhase::ArchiveFinalization,
            {RunPhaseRecord::executed(RunPhase::ArchiveFinalization,
                                      RunProgress::determinate(5, 1, 1))},
            "run",
            {RunFailure(RunFailureCode::WorkServiceFailed, RunPhase::ArchiveFinalization,
                        "primary failure")},
            {},
            {RunFailure(RunFailureCode::TemporaryArtifactCleanupFailed, RunPhase::SafetyCleanup,
                        "cleanup failure", {}, "staging.tmp")},
            true, &work));
        cao::gui::RunViewModel view;
        view.begin("run");
        QVERIFY(
            view.consume(RunEvent("run", 1, RunPhaseRecord::executed(RunPhase::SafetyCleanup))));
        QVERIFY(view.consume(RunEvent("run", 2, result)));
        std::string details;
        for (const auto& line : view.state().details) details += line + '\n';
        for (const auto* expected :
             {"primary failure", "cleanup failure", "staging.tmp", "source remains usable",
              "merge interrupted", "winner.bsa", "shadowed.bsa", "loose-asset-wins=yes",
              "Committed Mutations Retained", "partial-or-unknown=1", "Cancellation Observed: yes"})
            QVERIFY2(details.find(expected) != std::string::npos, expected);
        QVERIFY(view.state().progress);
        QCOMPARE(view.state().progress->completed(), std::size_t(2));
        QCOMPARE(view.state().progress->total(), std::size_t(5));
        const auto terminalLabel = view.state().label;
        QVERIFY(view.consume(RunEvent("run", 3,
                                      RunDiagnostic(RunDiagnosticCode::ObserverFailed,
                                                    RunPhase::SafetyCleanup, "late diagnostic"))));
        QCOMPARE(view.state().label, terminalLabel);
        QVERIFY(view.state().details.back().find("late diagnostic") != std::string::npos);
    }
};
QTEST_APPLESS_MAIN(GuiRunTests)
#include "GuiRunTests.moc"
