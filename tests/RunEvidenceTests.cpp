#include "Run/RunEvidence.h"
#include "Run/ArchiveExtraction.h"
#include "Run/ArchiveFinalizationResult.h"
#include "Run/RunSetup.h"

#include <QtTest>

#include <filesystem>
#include <optional>
#include <stdexcept>
#include <utility>
#include <vector>

using cao::routing::ExecutionMode;
using cao::run::ArchivePrecedence;
using cao::run::ArchiveFinalizationResult;
using cao::run::MutableRunEvidence;
using cao::run::RunConfiguration;
using cao::run::RunDiagnostic;
using cao::run::RunDiagnosticCode;
using cao::run::RunEvidence;
using cao::run::RunEvidenceInvariantViolation;
using cao::run::RunFailure;
using cao::run::RunFailureCode;
using cao::run::RunObservationSink;
using cao::run::RunPhase;
using cao::run::RunPhaseRecord;
using cao::run::RunPreparation;
using cao::run::RunProgress;
using cao::run::RunSetup;
using cao::run::SelectedProfileFacts;

namespace {
/// Builds one complete preparation value without borrowing any producer-owned facts.
RunPreparation successfulPreparation(
    std::filesystem::path root = "prepared-root",
    ArchivePrecedence precedence = ArchivePrecedence::explicitOrder({"winner.bsa",
                                                                     "shadowed.bsa"})) {
    RunConfiguration configuration{
        SelectedProfileFacts{.archiveExtension = ".BSA"}, {"ignored-mod"}, {"separator"}};
    const auto compiled =
        RunSetup::prepare(cao::routing::RoutingPolicyRequest::forWork(ExecutionMode::DryRun, {}),
                          configuration.profile());
    if (!compiled.hasPolicy()) throw std::logic_error("Test preparation policy did not compile");
    return RunPreparation({std::move(root)}, std::move(configuration), *compiled.policy(),
                          std::move(precedence));
}

/// Records mandatory cleanup and consumes the mutable owner into terminal evidence.
RunEvidence consumeAfterCleanup(MutableRunEvidence& evidence) {
    evidence.recordPhase(RunPhaseRecord::executed(RunPhase::SafetyCleanup));
    return std::move(evidence).consume();
}
}  // namespace

class RunEvidenceTests final : public QObject {
    Q_OBJECT

   private slots:
    /// Verifies all successful Preparing facts become visible together and own producer data.
    void successfulPreparationIsAtomicallyOwned();
    /// Verifies a terminal path without successful preparation exposes no partial facts.
    void unfinishedPreparationRemainsAbsent();
    /// Verifies the latest phase account replaces its predecessor without changing traversal order.
    void laterPhaseRecordsReplaceWithoutReordering();
    /// Verifies lifecycle traversal cannot start late, regress, or continue after cleanup.
    void phaseOrderViolationsAreRejected();
    /// Verifies phase-local totals and completed attempt counts cannot regress or become invalid.
    void progressRegressionsAreRejected();
    /// Verifies preparation is single-assignment and a consumed owner rejects every mutation.
    void preparationAndPostConsumptionMutationAreRejected();
    /// Verifies cancellation is retained as a fact without requiring terminal classification.
    void cancellationObservationIsSealedIndependently();
    /// Keeps Archive and Safety Cleanup failures out of run-level failure storage.
    void finalizationAndCleanupFailuresRemainSeparate();
    /// Publishes each completed Archive output only after its full attempt is retained.
    void finalizationProgressPublishesRetainedAttempt();
    /// Preserves streamed outputs when finalization fails before returning its result.
    void interruptedFinalizationRetainsCompletedAttempts();
    /// Derives committed and uncertain mutation counts from sealed attempts in Mod Root order.
    void sealedMutationSummariesReflectCompletedAttempts();
    /// Verifies every live payload is already queryable when its publication callback begins.
    void liveFactsAreRetainedBeforePublication();
    /// Supplies each live payload category as the one whose adapter callback throws.
    void throwingPublicationIsClaimedOnce_data();
    /// Verifies observer failure is retained once without retry, recursion, or later replay.
    void throwingPublicationIsClaimedOnce();
};

void RunEvidenceTests::successfulPreparationIsAtomicallyOwned() {
    std::optional<RunEvidence> terminal;
    {
        MutableRunEvidence evidence;
        evidence.recordPhase(RunPhaseRecord::executed(RunPhase::Preparing));
        auto preparation = successfulPreparation();
        static_cast<void>(evidence.recordPreparation(std::move(preparation)));
        terminal.emplace(consumeAfterCleanup(evidence));
    }

    QVERIFY(terminal->preparation() != nullptr);
    QCOMPARE(terminal->preparation()->modRoots().size(), std::size_t{1});
    QCOMPARE(terminal->preparation()->modRoots().front(), std::filesystem::path("prepared-root"));
    QCOMPARE(terminal->preparation()->policy().archiveExtension(), std::string(".bsa"));
    QCOMPARE(terminal->preparation()->policy().executionMode(), ExecutionMode::DryRun);
    QCOMPARE(terminal->preparation()->configuration().ignoredMods().front(),
             std::string("ignored-mod"));
    QCOMPARE(terminal->preparation()->configuration().separatorMarkers().front(),
             std::string("separator"));
    QCOMPARE(terminal->preparation()->archivePrecedence().highToLow().size(), std::size_t{2});
    QCOMPARE(terminal->preparation()->archivePrecedence().highToLow().front(),
             std::filesystem::path("winner.bsa"));
}

void RunEvidenceTests::unfinishedPreparationRemainsAbsent() {
    MutableRunEvidence evidence;
    evidence.recordPhase(RunPhaseRecord::executed(RunPhase::Preparing));

    const auto terminal = consumeAfterCleanup(evidence);

    QVERIFY(terminal.preparation() == nullptr);
}

void RunEvidenceTests::laterPhaseRecordsReplaceWithoutReordering() {
    MutableRunEvidence evidence;
    evidence.recordPhase(RunPhaseRecord::executed(RunPhase::Preparing));
    evidence.recordPhase(RunPhaseRecord::executed(RunPhase::DiscoveringArchives));
    evidence.recordPhase(
        RunPhaseRecord::executed(RunPhase::ExtractingArchives, RunProgress::determinate(3)));
    evidence.recordPhase(
        RunPhaseRecord::executed(RunPhase::ExtractingArchives, RunProgress::determinate(3, 1, 1)));

    const auto terminal = consumeAfterCleanup(evidence);

    QCOMPARE(terminal.phases().size(), std::size_t{4});
    QCOMPARE(terminal.phases()[0].phase(), RunPhase::Preparing);
    QCOMPARE(terminal.phases()[1].phase(), RunPhase::DiscoveringArchives);
    QCOMPARE(terminal.phases()[2].phase(), RunPhase::ExtractingArchives);
    QCOMPARE(terminal.phases()[3].phase(), RunPhase::SafetyCleanup);
    const auto* extraction = terminal.phase(RunPhase::ExtractingArchives);
    QVERIFY(extraction != nullptr);
    QCOMPARE(extraction->progress()->total(), std::size_t{3});
    QCOMPARE(extraction->progress()->succeeded(), std::size_t{1});
    QCOMPARE(extraction->progress()->failed(), std::size_t{1});
}

void RunEvidenceTests::phaseOrderViolationsAreRejected() {
    MutableRunEvidence evidence;
    QVERIFY_EXCEPTION_THROWN(
        evidence.recordPhase(RunPhaseRecord::executed(RunPhase::DiscoveringArchives)),
        std::logic_error);
    evidence.recordPhase(RunPhaseRecord::executed(RunPhase::Preparing));
    evidence.recordPhase(RunPhaseRecord::executed(RunPhase::ProcessingAssets));
    QVERIFY_EXCEPTION_THROWN(
        evidence.recordPhase(RunPhaseRecord::executed(RunPhase::ExtractingArchives)),
        std::logic_error);
    QVERIFY_EXCEPTION_THROWN(evidence.recordPhase(RunPhaseRecord::executed(RunPhase::Preparing)),
                             std::logic_error);
    static_cast<void>(consumeAfterCleanup(evidence));
    QVERIFY_EXCEPTION_THROWN(
        evidence.recordPhase(RunPhaseRecord::executed(RunPhase::SafetyCleanup)), std::logic_error);
}

void RunEvidenceTests::progressRegressionsAreRejected() {
    MutableRunEvidence evidence;
    evidence.recordPhase(RunPhaseRecord::executed(RunPhase::Preparing));
    QVERIFY_EXCEPTION_THROWN(evidence.recordPhase(RunPhaseRecord::executed(
                                 RunPhase::ExtractingArchives, RunProgress::determinate(4, 1))),
                             std::logic_error);
    evidence.recordPhase(
        RunPhaseRecord::executed(RunPhase::ExtractingArchives, RunProgress::determinate(4)));
    evidence.recordPhase(
        RunPhaseRecord::executed(RunPhase::ExtractingArchives, RunProgress::determinate(4, 2, 1)));

    QVERIFY_EXCEPTION_THROWN(evidence.recordPhase(RunPhaseRecord::executed(
                                 RunPhase::ExtractingArchives, RunProgress::determinate(5, 2, 1))),
                             std::logic_error);
    QVERIFY_EXCEPTION_THROWN(evidence.recordPhase(RunPhaseRecord::executed(
                                 RunPhase::ExtractingArchives, RunProgress::determinate(4, 1, 1))),
                             std::logic_error);
    QVERIFY_EXCEPTION_THROWN(evidence.recordPhase(RunPhaseRecord::executed(
                                 RunPhase::ExtractingArchives, RunProgress::determinate(4, 3, 2))),
                             std::logic_error);
    QVERIFY_EXCEPTION_THROWN(
        evidence.recordPhase(RunPhaseRecord::executed(RunPhase::ExtractingArchives)),
        std::logic_error);
    QVERIFY_EXCEPTION_THROWN(evidence.recordPhase(RunPhaseRecord::skipped(
                                 RunPhase::ExtractingArchives, cao::run::PhaseSkipReason::DryRun)),
                             std::logic_error);
}

void RunEvidenceTests::preparationAndPostConsumptionMutationAreRejected() {
    MutableRunEvidence evidence;
    evidence.recordPhase(RunPhaseRecord::executed(RunPhase::Preparing));
    QVERIFY_EXCEPTION_THROWN(static_cast<void>(std::move(evidence).consume()), std::logic_error);
    static_cast<void>(evidence.recordPreparation(successfulPreparation()));
    QVERIFY_EXCEPTION_THROWN(static_cast<void>(evidence.recordPreparation(successfulPreparation())),
                             std::logic_error);
    static_cast<void>(consumeAfterCleanup(evidence));

    QVERIFY_EXCEPTION_THROWN(static_cast<void>(evidence.recordPreparation(successfulPreparation())),
                             std::logic_error);
    QVERIFY_EXCEPTION_THROWN(evidence.recordCancellationObservation(), std::logic_error);
    QVERIFY_EXCEPTION_THROWN(static_cast<void>(std::move(evidence).consume()), std::logic_error);
}

void RunEvidenceTests::cancellationObservationIsSealedIndependently() {
    MutableRunEvidence evidence;
    evidence.recordPhase(RunPhaseRecord::executed(RunPhase::Preparing));
    evidence.recordCancellationObservation();

    const auto terminal = consumeAfterCleanup(evidence);

    QVERIFY(terminal.cancellationObserved());
}

void RunEvidenceTests::finalizationAndCleanupFailuresRemainSeparate() {
    using cao::execution::MutationState;
    using cao::run::ArchiveFinalizationFailure;

    MutableRunEvidence evidence;
    evidence.recordPhase(RunPhaseRecord::executed(RunPhase::Preparing));
    QVERIFY_EXCEPTION_THROWN(evidence.recordArchiveFinalizationPlan(2),
                             RunEvidenceInvariantViolation);
    evidence.recordPhase(RunPhaseRecord::executed(RunPhase::ArchiveFinalization));
    evidence.recordArchiveFinalizationPlan(2);
    evidence.recordArchiveFinalization(ArchiveFinalizationResult{{
        {"first.bsa", MutationState::Committed, {}, true, {}, "first-mod"},
        {"second.bsa", MutationState::Committed,
         ArchiveFinalizationFailure::SourceCleanupFailed, true, "source retained", "second-mod"}}});
    evidence.recordPhase(RunPhaseRecord::executed(RunPhase::SafetyCleanup));
    evidence.recordSafetyCleanupFailure(
        RunFailure{RunFailureCode::TemporaryArtifactCleanupFailed, RunPhase::SafetyCleanup,
                   "first temporary artifact"});
    evidence.recordSafetyCleanupFailure(
        RunFailure{RunFailureCode::SafetyCleanupServiceFailed, RunPhase::SafetyCleanup,
                   "cleanup service exception"});

    const auto terminal = std::move(evidence).consume();

    const auto* finalization = terminal.archiveFinalization();
    QVERIFY(finalization != nullptr);
    QCOMPARE(finalization->attempts.size(), std::size_t{2});
    QCOMPARE(finalization->attempts[0].modRoot, std::filesystem::path("first-mod"));
    QCOMPARE(finalization->attempts[0].mutation, MutationState::Committed);
    QVERIFY(!finalization->attempts[0].failure);
    QCOMPARE(finalization->attempts[1].modRoot, std::filesystem::path("second-mod"));
    QCOMPARE(finalization->attempts[1].failure,
             std::optional{ArchiveFinalizationFailure::SourceCleanupFailed});
    QVERIFY(finalization->attempts[1].safeToContinue);
    QCOMPARE(terminal.phase(RunPhase::ArchiveFinalization)->progress()->completed(),
             std::size_t{2});
    QVERIFY(terminal.failures().empty());
    QCOMPARE(terminal.safetyCleanupFailures().size(), std::size_t{2});
    QCOMPARE(terminal.safetyCleanupFailures()[0].detail(),
             std::string("first temporary artifact"));
    QCOMPARE(terminal.safetyCleanupFailures()[1].detail(),
             std::string("cleanup service exception"));
    QCOMPARE(terminal.cleanupFailures().size(), std::size_t{2});
    QCOMPARE(terminal.cleanupFailures()[0].detail(), std::string("first temporary artifact"));
    QCOMPARE(terminal.cleanupFailures()[1].detail(), std::string("cleanup service exception"));
}

void RunEvidenceTests::finalizationProgressPublishesRetainedAttempt() {
    class InspectingObservation final : public RunObservationSink {
       public:
        const MutableRunEvidence* evidence{};
        std::size_t completed{};

        /// Reads the exact output attempt during its completed progress publication.
        void recordPhase(const RunPhaseRecord& phase) override {
            if (phase.phase() != RunPhase::ArchiveFinalization || !phase.progress() ||
                phase.progress()->completed() == 0)
                return;
            const auto* finalization = evidence->archiveFinalization();
            QVERIFY(finalization != nullptr);
            QCOMPARE(finalization->attempts.size(), phase.progress()->completed());
            QCOMPARE(finalization->attempts.back().modRoot,
                     std::filesystem::path(phase.progress()->completed() == 1 ? "first-mod"
                                                                           : "second-mod"));
            completed = phase.progress()->completed();
        }
        /// This fixture produces no run-level failures.
        void recordFailure(const RunFailure&) override {}
        /// This fixture produces no diagnostics.
        void recordDiagnostic(const RunDiagnostic&) override {}
    } observation;
    MutableRunEvidence evidence{&observation};
    observation.evidence = &evidence;
    evidence.recordPhase(RunPhaseRecord::executed(RunPhase::Preparing));
    evidence.recordPhase(RunPhaseRecord::executed(RunPhase::ArchiveFinalization));
    evidence.recordArchiveFinalizationPlan(2);
    ArchiveFinalizationResult finalization;
    finalization.attempts.push_back(
        {"first.bsa", cao::execution::MutationState::Committed, {}, true, {}, "first-mod"});
    finalization.attempts.push_back(
        {"second.bsa", cao::execution::MutationState::None,
         cao::run::ArchiveFinalizationFailure::WriteFailed, true, "failed write", "second-mod"});
    evidence.recordArchiveFinalizationAttempt(finalization.attempts.front(), 2);
    evidence.recordArchiveFinalizationAttempt(finalization.attempts.back(), 2);
    evidence.recordArchiveFinalization(std::move(finalization));

    const auto terminal = consumeAfterCleanup(evidence);
    QCOMPARE(observation.completed, std::size_t{2});
    QCOMPARE(terminal.archiveFinalization()->attempts.size(), std::size_t{2});
    QCOMPARE(terminal.phase(RunPhase::ArchiveFinalization)->progress()->succeeded(),
             std::size_t{1});
    QCOMPARE(terminal.phase(RunPhase::ArchiveFinalization)->progress()->failed(),
             std::size_t{1});
}

void RunEvidenceTests::interruptedFinalizationRetainsCompletedAttempts() {
    MutableRunEvidence evidence;
    evidence.recordPhase(RunPhaseRecord::executed(RunPhase::Preparing));
    evidence.recordPhase(RunPhaseRecord::executed(RunPhase::ArchiveFinalization));
    evidence.recordArchiveFinalizationPlan(2);
    evidence.recordArchiveFinalizationAttempt(
        {"committed.bsa", cao::execution::MutationState::Committed, {}, true, {}, "first-mod"},
        2);
    ArchiveFinalizationResult interrupted;
    interrupted.failure = cao::run::ArchiveFinalizationFailure::UnexpectedException;
    interrupted.safeToContinue = false;
    interrupted.detail = "packing interrupted";
    evidence.recordArchiveFinalization(std::move(interrupted));

    const auto terminal = consumeAfterCleanup(evidence);
    const auto* finalization = terminal.archiveFinalization();
    QVERIFY(finalization != nullptr);
    QCOMPARE(finalization->attempts.size(), std::size_t{1});
    QCOMPARE(finalization->attempts.front().modRoot, std::filesystem::path("first-mod"));
    QCOMPARE(finalization->attempts.front().mutation,
             cao::execution::MutationState::Committed);
    QCOMPARE(finalization->failure,
             std::optional{cao::run::ArchiveFinalizationFailure::UnexpectedException});
    QVERIFY(!finalization->safeToContinue);
    QCOMPARE(terminal.phase(RunPhase::ArchiveFinalization)->progress()->total(),
             std::size_t{2});
    QCOMPARE(terminal.phase(RunPhase::ArchiveFinalization)->progress()->completed(),
             std::size_t{1});
}

void RunEvidenceTests::sealedMutationSummariesReflectCompletedAttempts() {
    using cao::execution::MutationState;
    using cao::run::ArchiveExtractionFailure;
    using cao::run::ArchiveFinalizationFailure;
    using cao::run::MutationKind;

    std::optional<RunEvidence> terminal;
    {
        MutableRunEvidence evidence;
        evidence.recordPhase(RunPhaseRecord::executed(RunPhase::Preparing));
        evidence.recordPhase(RunPhaseRecord::executed(RunPhase::DiscoveringArchives));
        evidence.recordArchiveExtractionPlan(3);
        evidence.recordArchiveExtractionAttempt(
            {"first.bsa", MutationState::Committed, {}, true, {}, "alpha"}, 3);
        evidence.recordArchiveExtractionAttempt(
            {"second.bsa", MutationState::PartialOrUnknown, ArchiveExtractionFailure::MergeFailed,
             false, "uncertain merge", "alpha"},
            3);
        evidence.recordArchiveExtractionAttempt(
            {"third.bsa", MutationState::None, ArchiveExtractionFailure::ExtractionFailed, true,
             "no write", "beta"},
            3);
        evidence.recordPhase(RunPhaseRecord::executed(RunPhase::ArchiveFinalization));
        evidence.recordArchiveFinalizationPlan(2);
        evidence.recordArchiveFinalization(ArchiveFinalizationResult{
            {{"packed-a.bsa", MutationState::Committed, {}, true, {}, "alpha"},
             {"packed-b.bsa", MutationState::Committed,
              ArchiveFinalizationFailure::SourceCleanupFailed, true, "source retained", "beta"}}});
        terminal.emplace(consumeAfterCleanup(evidence));
    }

    const auto summaries = terminal->mutationSummaries();
    QCOMPARE(summaries.size(), std::size_t{3});
    QCOMPARE(summaries[0].modRoot, std::filesystem::path("alpha"));
    QCOMPARE(summaries[0].kind, MutationKind::ArchiveExtraction);
    QCOMPARE(summaries[0].committed, std::size_t{1});
    QCOMPARE(summaries[0].partialOrUnknown, std::size_t{1});
    QCOMPARE(summaries[1].modRoot, std::filesystem::path("alpha"));
    QCOMPARE(summaries[1].kind, MutationKind::ArchiveFinalization);
    QCOMPARE(summaries[1].committed, std::size_t{1});
    QCOMPARE(summaries[2].modRoot, std::filesystem::path("beta"));
    QCOMPARE(summaries[2].kind, MutationKind::ArchiveFinalization);
    QCOMPARE(summaries[2].committed, std::size_t{1});
}

void RunEvidenceTests::liveFactsAreRetainedBeforePublication() {
    class InspectingObservation final : public RunObservationSink {
       public:
        const MutableRunEvidence* evidence{};
        std::vector<int> payloadKinds;

        /// Confirms the accepted phase is visible before the publication adapter runs.
        void recordPhase(const RunPhaseRecord& phase) override {
            QVERIFY(evidence->phase(phase.phase()) != nullptr);
            payloadKinds.push_back(0);
        }

        /// Confirms the accepted failure is retained before the publication adapter runs.
        void recordFailure(const RunFailure& failure) override {
            QVERIFY(!evidence->failures().empty());
            QCOMPARE(evidence->failures().back().code(), failure.code());
            payloadKinds.push_back(2);
        }

        /// Confirms the accepted diagnostic is retained before the publication adapter runs.
        void recordDiagnostic(const RunDiagnostic& diagnostic) override {
            QVERIFY(!evidence->diagnostics().empty());
            QCOMPARE(evidence->diagnostics().back().code(), diagnostic.code());
            payloadKinds.push_back(1);
        }
    } observation;

    MutableRunEvidence evidence{&observation};
    observation.evidence = &evidence;
    evidence.recordPhase(RunPhaseRecord::executed(RunPhase::Preparing));
    evidence.recordDiagnostic(RunDiagnostic{RunDiagnosticCode::IgnoredModExcluded,
                                            RunPhase::Preparing, "retained diagnostic"});
    evidence.recordFailure(RunFailure{RunFailureCode::ConfigurationLoadingFailed,
                                      RunPhase::Preparing, "retained failure"});

    const auto terminal = consumeAfterCleanup(evidence);

    QCOMPARE(observation.payloadKinds, std::vector<int>({0, 1, 2, 0}));
    QCOMPARE(terminal.diagnostics().size(), std::size_t{1});
    QCOMPARE(terminal.failures().size(), std::size_t{1});
}

void RunEvidenceTests::throwingPublicationIsClaimedOnce_data() {
    QTest::addColumn<int>("throwingKind");
    QTest::newRow("phase") << 0;
    QTest::newRow("diagnostic") << 1;
    QTest::newRow("failure") << 2;
}

void RunEvidenceTests::throwingPublicationIsClaimedOnce() {
    QFETCH(int, throwingKind);
    class ThrowingObservation final : public RunObservationSink {
       public:
        explicit ThrowingObservation(const int throwingKind) : _throwingKind(throwingKind) {}

        /// Throws from the selected first phase callback after recording its one invocation.
        void recordPhase(const RunPhaseRecord&) override {
            ++phaseCalls;
            throwSelected(0);
        }

        /// Throws from the selected diagnostic callback without receiving generated failures.
        void recordDiagnostic(const RunDiagnostic& diagnostic) override {
            ++diagnosticCalls;
            deliveredDiagnosticCodes.push_back(diagnostic.code());
            throwSelected(1);
        }

        /// Throws from the selected failure callback after recording its one invocation.
        void recordFailure(const RunFailure&) override {
            ++failureCalls;
            throwSelected(2);
        }

        std::size_t phaseCalls{};
        std::size_t diagnosticCalls{};
        std::size_t failureCalls{};
        std::vector<RunDiagnosticCode> deliveredDiagnosticCodes;

       private:
        /// Raises one controlled adapter exception for the selected payload category.
        void throwSelected(const int kind) {
            if (!_thrown && kind == _throwingKind) {
                _thrown = true;
                throw std::runtime_error("controlled publication failure");
            }
        }

        int _throwingKind;
        bool _thrown{};
    } observation{throwingKind};

    MutableRunEvidence evidence{&observation};
    evidence.recordPhase(RunPhaseRecord::executed(RunPhase::Preparing));
    evidence.recordDiagnostic(RunDiagnostic{RunDiagnosticCode::IgnoredModExcluded,
                                            RunPhase::Preparing, "first diagnostic"});
    evidence.recordFailure(RunFailure{RunFailureCode::ConfigurationLoadingFailed,
                                      RunPhase::Preparing, "retained failure"});
    evidence.recordDiagnostic(RunDiagnostic{RunDiagnosticCode::SeparatorModExcluded,
                                            RunPhase::Preparing, "later diagnostic"});

    const auto terminal = consumeAfterCleanup(evidence);
    const auto observerFailures = std::count_if(
        terminal.diagnostics().begin(), terminal.diagnostics().end(), [](const auto& diagnostic) {
            return diagnostic.code() == RunDiagnosticCode::ObserverFailed;
        });

    QCOMPARE(observation.phaseCalls, std::size_t{2});
    QCOMPARE(observation.diagnosticCalls, std::size_t{2});
    QCOMPARE(observation.failureCalls, std::size_t{1});
    QCOMPARE(observation.deliveredDiagnosticCodes,
             std::vector<RunDiagnosticCode>(
                 {RunDiagnosticCode::IgnoredModExcluded, RunDiagnosticCode::SeparatorModExcluded}));
    QCOMPARE(observerFailures, 1);
    QCOMPARE(terminal.diagnostics().size(), std::size_t{3});
    QCOMPARE(terminal.failures().size(), std::size_t{1});
}

QTEST_MAIN(RunEvidenceTests)
#include "RunEvidenceTests.moc"
