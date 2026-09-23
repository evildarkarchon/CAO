#include "GuiRunDispatch.h"
#include "GuiRun.h"
#include "AssetExecution/AssetExecutor.h"
#include "AssetRouting/AssetRouter.h"
#include "Run/TemporaryArtifactRegistry.h"
#include "Run/AssetRun.h"
#include "RunTestConfiguration.h"

#include <QtTest>

#include <memory>
#include <atomic>
#include <fstream>
#include <semaphore>
#include <thread>

namespace {
/// Holds an atomic attempt open so the GUI can cancel without allowing Safety Cleanup yet.
class GatedAttempt final : public cao::run::RunWorkService {
   public:
    /// Owns the temporary path; the caller retains its directory until the worker is joined.
    explicit GatedAttempt(std::filesystem::path path) : artifact(std::move(path)) {}

    /// Retains two completed attempts, then holds a third open before mandatory cleanup.
    void execute(const cao::run::RunPreparation& preparation, cao::run::RunWorkRecord&,
                 cao::run::MutableRunEvidence& evidence,
                 cao::run::TemporaryArtifactRegistry& artifacts,
                 cao::run::RunObservationSink& observations, std::stop_token) override {
        const auto root = preparation.modRoots().front();
        observations.archiveDiscoveryStarted();
        evidence.recordArchiveDiscovery(cao::run::ArchiveDiscoveryEvidence({}, {}, 0));
        observations.archiveExtractionPlanned(0);
        observations.effectiveAssetTreeStarted();
        const cao::routing::AssetRouter router(preparation.policy());
        const std::vector paths{root / "committed.dds", root / "failed.dds", root / "pending.dds"};
        auto ledger = router.route(paths);
        // Attempts need routed values after the evidence owner consumes the definitive ledger.
        const auto assets = std::vector<cao::routing::RoutedAsset>(ledger.routedAssets().begin(),
                                                                   ledger.routedAssets().end());
        evidence.recordRoutingLedger(std::move(ledger));
        observations.assetProcessingPlanned(3);
        evidence.recordAssetAttempt({root, assets[0],
                                     cao::execution::AssetExecutionResult::success(
                                         cao::execution::MutationState::Committed)},
                                    3);
        evidence.recordAssetAttempt(
            {root, assets[1],
             cao::execution::AssetExecutionResult::failed(
                 cao::execution::AssetExecutionFailure::SaveFailed, "queued asset failure")},
            3);
        static_cast<void>(artifacts.registerArtifact(artifact));
        std::ofstream(artifact) << "temporary attempt output";
        entered.store(true);
        release.acquire();
    }

    std::filesystem::path artifact;
    std::atomic<bool> entered{};
    std::binary_semaphore release{0};
};

/// Releases a gated worker before handle destruction even when a Qt assertion returns early.
struct ReleaseAttempt final {
    GatedAttempt& work;
    bool released{};
    ~ReleaseAttempt() {
        if (!released) work.release.release();
    }
};
}  // namespace

class GuiRunDispatchTests final : public QObject {
    Q_OBJECT

   private slots:
    /// Verifies GUI-thread submissions still wait for the event queue before delivery.
    void deliveryIsAlwaysQueued();
    /// Verifies worker submissions on either side of target destruction discard borrowed callbacks.
    void destroyedTargetDiscardsQueuedDelivery();
    /// Verifies worker-produced observations invoke the supplied observer on the target thread.
    void workerDeliveryRunsOnTargetThread();
    /// Verifies destruction inside one observer also suppresses later events in the same drain.
    void observerDestructionStopsTheCurrentDrain();
    /// Closing during an atomic attempt waits for real cleanup and queued terminal delivery.
    void closeWaitsForCleanupAndTerminalDelivery();
};

void GuiRunDispatchTests::deliveryIsAlwaysQueued() {
    QObject target;
    bool delivered{};
    auto observation = cao::gui::queuedObservation(&target, {});
    observation.dispatcher([&] { delivered = true; });
    QVERIFY(!delivered);
    QTRY_VERIFY(delivered);
}

void GuiRunDispatchTests::destroyedTargetDiscardsQueuedDelivery() {
    auto target = std::make_unique<QObject>();
    unsigned delivered{};
    auto observation = cao::gui::queuedObservation(target.get(), {});
    std::thread beforeDestruction([&] { observation.dispatcher([&] { ++delivered; }); });
    beforeDestruction.join();
    target.reset();
    std::thread afterDestruction([&] { observation.dispatcher([&] { ++delivered; }); });
    afterDestruction.join();
    QCoreApplication::sendPostedEvents(nullptr, QEvent::MetaCall);
    QCOMPARE(delivered, 0U);
}

void GuiRunDispatchTests::workerDeliveryRunsOnTargetThread() {
    QObject target;
    QThread* deliveryThread{};
    std::uint64_t sequence{};
    auto observation = cao::gui::queuedObservation(&target, [&](const cao::run::RunEvent& event) {
        deliveryThread = QThread::currentThread();
        sequence = event.sequence();
    });
    const cao::run::RunEvent event{"queued-run", 7,
                                   std::shared_ptr<const cao::run::OptimizationRunResult>{}};
    std::thread worker([&] { observation.dispatcher([&] { observation.observer(event); }); });
    worker.join();
    QVERIFY(!deliveryThread);
    QTRY_VERIFY(deliveryThread);
    QCOMPARE(deliveryThread, target.thread());
    QCOMPARE(sequence, std::uint64_t{7});
}

void GuiRunDispatchTests::observerDestructionStopsTheCurrentDrain() {
    auto target = std::make_unique<QObject>();
    unsigned delivered{};
    auto observation = cao::gui::queuedObservation(target.get(), [&](const cao::run::RunEvent&) {
        ++delivered;
        target.reset();
    });
    const cao::run::RunEvent event{"destroying-run", 1,
                                   std::shared_ptr<const cao::run::OptimizationRunResult>{}};
    observation.dispatcher([&] {
        observation.observer(event);
        observation.observer(event);
    });
    QCoreApplication::sendPostedEvents(nullptr, QEvent::MetaCall);
    QCOMPARE(delivered, 1U);
}

void GuiRunDispatchTests::closeWaitsForCleanupAndTerminalDelivery() {
    using namespace cao::run;
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    const auto artifact = root / "attempt.tmp";
    auto work = std::make_shared<GatedAttempt>(artifact);
    const std::weak_ptr<GatedAttempt> retainedWork = work;
    auto service = std::make_unique<OptimizationRunService>(testRunConfiguration(), work);
    auto target = std::make_unique<QObject>();
    cao::gui::RunViewModel view;
    bool terminalDelivered{};
    bool cleanupFinishedAtDelivery{};
    std::shared_ptr<const OptimizationRunResult> eventResult;
    auto observation = cao::gui::queuedObservation(target.get(), [&](const RunEvent& event) {
        if (const auto* terminal =
                std::get_if<std::shared_ptr<const OptimizationRunResult>>(&event.payload()))
            eventResult = *terminal;
        if (!view.consume(event) || !view.state().outcome) return;
        terminalDelivered = true;
        cleanupFinishedAtDelivery = !std::filesystem::exists(artifact);
    });
    auto started =
        service->start(RunRequest::create("SkyrimSE", cao::routing::ExecutionMode::Apply,
                                          ModSelection::singleModRoot(root),
                                          {cao::routing::RequestedWork::NativeTextureOptimization}),
                       std::vector<RunObservation>{std::move(observation)});
    // This guard must unwind before the owning handle, which joins the blocked worker.
    ReleaseAttempt release{*work};
    QVERIFY(started.started());
    view.begin(started.handle()->snapshot().runId());
    QTRY_VERIFY(work->entered.load());
    QTRY_COMPARE(view.state().label, std::string("Processing Assets"));
    QVERIFY(!view.requestClose());
    started.handle()->requestCancellation();
    QVERIFY(!view.requestClose());
    QCOMPARE(view.state().label, std::string("Cancelling - Processing Assets"));
    QCOMPARE(view.state().progress->completed(), std::size_t{2});
    QVERIFY(std::filesystem::exists(artifact));
    QVERIFY(!terminalDelivered);
    QVERIFY(target);
    work.reset();
    QVERIFY(!retainedWork.expired());

    release.released = true;
    release.work.release.release();
    const auto& result = started.handle()->wait();
    QCOMPARE(result.outcome(), RunOutcome::Cancelled);
    QVERIFY(!std::filesystem::exists(artifact));
    QVERIFY(!terminalDelivered);
    QVERIFY(!view.requestClose());
    QVERIFY(view.state().active);

    QTRY_VERIFY(terminalDelivered);
    QVERIFY(cleanupFinishedAtDelivery);
    QCOMPARE(eventResult.get(), &result);
    QCOMPARE(started.handle()->terminalResult(), &result);
    std::string details;
    for (const auto& line : view.state().details) details += line + '\n';
    QVERIFY(details.find("queued asset failure") != std::string::npos);
    QVERIFY(details.find("Committed Mutations Retained") != std::string::npos);
    QVERIFY(details.find("committed=1") != std::string::npos);
    QVERIFY(view.requestClose());
    QVERIFY(!view.canStart());
    target.reset();
    // Delivery has unwound before the run owners and their dependencies are released.
    service.reset();
}

QTEST_GUILESS_MAIN(GuiRunDispatchTests)
#include "GuiRunDispatchTests.moc"
