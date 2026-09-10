#include "GuiRunDispatch.h"

#include <QtTest>

#include <memory>
#include <thread>

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

QTEST_GUILESS_MAIN(GuiRunDispatchTests)
#include "GuiRunDispatchTests.moc"
