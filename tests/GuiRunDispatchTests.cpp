#include "GuiRunDispatch.h"
#include "GuiRun.h"
#include "Run/TemporaryArtifactRegistry.h"
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

    /// Registers real cleanup work and delays return without interrupting the atomic attempt.
    void execute(const cao::run::RunPreparation&, cao::run::RunWorkRecord&,
                 cao::run::TemporaryArtifactRegistry& artifacts,
                 cao::run::RunObservationSink& observations, std::stop_token) override {
        static_cast<void>(artifacts.registerArtifact(artifact));
        std::ofstream(artifact) << "temporary attempt output";
        observations.recordPhase(cao::run::RunPhaseRecord::executed(
            cao::run::RunPhase::ProcessingAssets, cao::run::RunProgress::determinate(2, 0, 0)));
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
    auto observation = cao::gui::queuedObservation(target.get(), [&](const RunEvent& event) {
        if (!view.consume(event) || !view.state().outcome) return;
        terminalDelivered = true;
        cleanupFinishedAtDelivery = !std::filesystem::exists(artifact);
    });
    auto started = service->start(
        RunRequest::create("SkyrimSE", cao::routing::ExecutionMode::Apply,
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
    QCOMPARE(view.state().progress->completed(), std::size_t{0});
    QVERIFY(std::filesystem::exists(artifact));
    QVERIFY(!terminalDelivered);
    QVERIFY(target);
    work.reset();
    QVERIFY(!retainedWork.expired());

    release.released = true;
    release.work.release.release();
    const auto result = started.handle()->wait();
    QCOMPARE(result.outcome(), RunOutcome::Cancelled);
    QVERIFY(!std::filesystem::exists(artifact));
    QVERIFY(!terminalDelivered);
    QVERIFY(!view.requestClose());
    QVERIFY(view.state().active);

    QTRY_VERIFY(terminalDelivered);
    QVERIFY(cleanupFinishedAtDelivery);
    QVERIFY(view.requestClose());
    QVERIFY(!view.canStart());
    target.reset();
    // Delivery has unwound before the run owners and their dependencies are released.
    service.reset();
}

QTEST_GUILESS_MAIN(GuiRunDispatchTests)
#include "GuiRunDispatchTests.moc"
