#include "Run/OptimizationRunService.h"

#include <QtTest>

#include <atomic>
#include <chrono>
#include <functional>
#include <mutex>
#include <optional>
#include <semaphore>
#include <stdexcept>
#include <thread>
#include <vector>

using namespace cao::run;
using namespace std::chrono_literals;

namespace {
/// Creates a request whose deterministic phase traversal needs no filesystem services.
RunRequest noWorkRequest() {
    return RunRequest::create("profile", cao::routing::ExecutionMode::Apply,
                              ModSelection::singleModRoot("mod"), {});
}
}  // namespace

class RunEventDeliveryTests final : public QObject {
    Q_OBJECT

   private slots:
    /// Verifies late presentation failures remain observable without rewriting a committed result.
    void queuedFailureAfterWaitPreservesTheResult();
    /// Verifies a dispatcher cannot revive its disabled observer through a retained closure.
    void dispatcherThatQueuesThenThrowsDisablesDelivery();
    /// Verifies a dispatcher failure after successful inline delivery is recorded exactly once.
    void dispatcherThatDeliversThenThrowsIsDiagnosedOnce();
    /// Runs queued callbacks against an active producer and detects concurrent observer entry.
    void concurrentDispatcherPreservesSerializedSequences();
    /// Verifies terminal delivery observes a released process-wide run slot.
    void terminalObserverStartsTheNextRun();
    /// Verifies structural and active-run rejections never invoke presentation callbacks.
    void rejectedStartsEmitNoEvents();
    /// Catches wait returning while terminal delivery has not reached the caller's queue yet.
    void waitIncludesTerminalDispatcherAdmission();
};

void RunEventDeliveryTests::queuedFailureAfterWaitPreservesTheResult() {
    InlineRunScheduler scheduler;
    OptimizationRunService service{scheduler};
    std::vector<std::function<void()>> queued;
    std::vector<RunEvent> healthy;
    std::size_t failingCalls{};
    auto started = service.start(noWorkRequest(), std::vector<RunObservation>{
        {[&](const RunEvent&) {
             ++failingCalls;
             throw std::runtime_error("late presentation failure");
         },
         [&](std::function<void()> delivery) { queued.push_back(std::move(delivery)); }},
        {[&](const RunEvent& event) { healthy.push_back(event); }, {}}});
    const auto* terminal = &started.handle()->wait();
    const auto id = terminal->runId();
    const auto phaseCount = terminal->phases().size();
    QVERIFY(started.handle()->diagnostics().empty());
    QVERIFY(!healthy.empty());
    QVERIFY(std::holds_alternative<std::shared_ptr<const OptimizationRunResult>>(
        healthy.back().payload()));
    QVERIFY(!queued.empty());
    for (auto& delivery : queued) delivery();

    QCOMPARE(failingCalls, std::size_t{1});
    QVERIFY(started.handle()->terminalResult() == terminal);
    QCOMPARE(terminal->outcome(), RunOutcome::Succeeded);
    QCOMPARE(terminal->runId(), id);
    QCOMPARE(terminal->phases().size(), phaseCount);
    const auto diagnostics = started.handle()->diagnostics();
    QCOMPARE(diagnostics.size(), std::size_t{1});
    QCOMPARE(diagnostics.front().code(), RunDiagnosticCode::ObserverFailed);
    QCOMPARE(diagnostics.front().detail(), std::string{"late presentation failure"});
    QVERIFY(std::holds_alternative<RunDiagnostic>(healthy.back().payload()));
    for (std::size_t i = 0; i < healthy.size(); ++i) {
        QCOMPARE(healthy[i].runId(), id);
        QCOMPARE(healthy[i].sequence(), i + 1);
    }
}

void RunEventDeliveryTests::dispatcherThatQueuesThenThrowsDisablesDelivery() {
    InlineRunScheduler scheduler;
    OptimizationRunService service{scheduler};
    std::function<void()> retained;
    std::size_t observerCalls{};
    std::size_t dispatchCalls{};
    auto started = service.start(noWorkRequest(),
        [&](const RunEvent&) { ++observerCalls; },
        [&](std::function<void()> delivery) {
            ++dispatchCalls;
            retained = std::move(delivery);
            throw std::runtime_error("queue rejected");
        });
    QCOMPARE(started.handle()->wait().outcome(), RunOutcome::Succeeded);
    QVERIFY(static_cast<bool>(retained));
    retained();
    QCOMPARE(observerCalls, std::size_t{0});
    QCOMPARE(dispatchCalls, std::size_t{1});
    const auto diagnostics = started.handle()->diagnostics();
    QCOMPARE(diagnostics.size(), std::size_t{1});
    QCOMPARE(diagnostics.front().code(), RunDiagnosticCode::DispatcherFailed);
}

void RunEventDeliveryTests::dispatcherThatDeliversThenThrowsIsDiagnosedOnce() {
    InlineRunScheduler scheduler;
    OptimizationRunService service{scheduler};
    std::size_t observerCalls{};
    std::size_t dispatchCalls{};
    auto started = service.start(noWorkRequest(),
        [&](const RunEvent&) { ++observerCalls; },
        [&](std::function<void()> delivery) {
            ++dispatchCalls;
            delivery();
            throw std::runtime_error("dispatcher failed after delivery");
        });
    QCOMPARE(started.handle()->wait().outcome(), RunOutcome::Succeeded);
    QCOMPARE(observerCalls, std::size_t{1});
    QCOMPARE(dispatchCalls, std::size_t{1});
    const auto diagnostics = started.handle()->diagnostics();
    QCOMPARE(diagnostics.size(), std::size_t{1});
    QCOMPARE(diagnostics.front().code(), RunDiagnosticCode::DispatcherFailed);
}

void RunEventDeliveryTests::concurrentDispatcherPreservesSerializedSequences() {
    InlineRunScheduler scheduler;
    OptimizationRunService service{scheduler};
    std::binary_semaphore firstEntered{0};
    std::binary_semaphore releaseFirst{0};
    std::atomic<unsigned> active{};
    std::atomic<bool> concurrent{}, timedOut{};
    std::mutex observationsMutex;
    std::vector<RunEvent> events;
    std::vector<std::jthread> deliveries;
    auto started = service.start(noWorkRequest(),
        [&](const RunEvent& event) {
            if (active.fetch_add(1) != 0) concurrent.store(true);
            if (event.sequence() == 1) {
                firstEntered.release();
                // Hold the first callback while the worker produces every remaining event.
                if (!releaseFirst.try_acquire_for(5s)) timedOut.store(true);
            }
            {
                const std::lock_guard lock(observationsMutex);
                events.push_back(event);
            }
            active.fetch_sub(1);
        },
        [&](std::function<void()> delivery) {
            deliveries.emplace_back(std::move(delivery));
            if (deliveries.size() == 1 && !firstEntered.try_acquire_for(5s))
                timedOut.store(true);
        });
    const auto* terminal = started.handle()->terminalResult();
    releaseFirst.release();
    for (auto& delivery : deliveries) delivery.join();

    QVERIFY(!timedOut.load());
    QVERIFY(!concurrent.load());
    QVERIFY(terminal != nullptr);
    QCOMPARE(events.size(), runPhaseSequence().size() + 1);
    for (std::size_t i = 0; i < events.size(); ++i) {
        QCOMPARE(events[i].sequence(), i + 1);
        QCOMPARE(events[i].runId(), terminal->runId());
    }
    QVERIFY(std::holds_alternative<std::shared_ptr<const OptimizationRunResult>>(
        events.back().payload()));
}

void RunEventDeliveryTests::terminalObserverStartsTheNextRun() {
    InlineRunScheduler scheduler;
    OptimizationRunService service{scheduler};
    OptimizationRunService nextService{scheduler};
    std::optional<RunStartResult> next;
    auto started = service.start(noWorkRequest(), [&](const RunEvent& event) {
        if (std::holds_alternative<std::shared_ptr<const OptimizationRunResult>>(event.payload()))
            next.emplace(nextService.start(noWorkRequest()));
    });
    QVERIFY(next.has_value());
    QVERIFY(next->started());
    QCOMPARE(next->handle()->wait().outcome(), RunOutcome::Succeeded);
    QCOMPARE(started.handle()->wait().outcome(), RunOutcome::Succeeded);
    QVERIFY(next->handle()->terminalResult()->runId() != started.handle()->terminalResult()->runId());
}

void RunEventDeliveryTests::rejectedStartsEmitNoEvents() {
    InlineRunScheduler scheduler;
    OptimizationRunService service{scheduler};
    std::size_t rejectedEvents{}, rejectedDispatches{};
    auto observer = [&](const RunEvent&) { ++rejectedEvents; };
    auto dispatcher = [&](std::function<void()> delivery) {
        ++rejectedDispatches;
        delivery();
    };
    auto malformed = service.start(RunRequest::create("", cao::routing::ExecutionMode::Apply,
        ModSelection::singleModRoot("mod"), {}), observer, dispatcher);
    QCOMPARE(malformed.startError(), std::optional{StartError::MissingProfileIdentity});
    std::optional<StartError> conflict;
    auto started = service.start(noWorkRequest(), [&](const RunEvent& event) {
        if (event.sequence() == 1)
            conflict = service.start(noWorkRequest(), observer, dispatcher).startError();
    });
    QCOMPARE(started.handle()->wait().outcome(), RunOutcome::Succeeded);
    QCOMPARE(conflict, std::optional{StartError::ActiveRun});
    QCOMPARE(rejectedEvents, std::size_t{0});
    QCOMPARE(rejectedDispatches, std::size_t{0});
}

void RunEventDeliveryTests::waitIncludesTerminalDispatcherAdmission() {
    std::binary_semaphore atTerminalDispatcher{0}, allowEnqueue{0}, waiterEntered{0}, returned{0};
    std::function<void()> terminalDelivery;
    std::size_t dispatched{};
    std::atomic<std::size_t> delivered{};
    OptimizationRunService service;
    auto started = service.start(noWorkRequest(),
        [&](const RunEvent&) { ++delivered; },
        [&](std::function<void()> delivery) {
            if (++dispatched == 8) {
                atTerminalDispatcher.release();
                allowEnqueue.acquire();
                terminalDelivery = std::move(delivery);
            } else delivery();
        });
    atTerminalDispatcher.acquire();
    std::jthread waiter([&] {
        waiterEntered.release();
        static_cast<void>(started.handle()->wait());
        returned.release();
    });
    waiterEntered.acquire();
    const bool returnedBeforeEnqueue = returned.try_acquire_for(100ms);
    allowEnqueue.release();
    waiter.join();
    QVERIFY(!returnedBeforeEnqueue);
    QCOMPARE(delivered.load(), std::size_t{7});
    QVERIFY(static_cast<bool>(terminalDelivery));
    terminalDelivery();
    QCOMPARE(delivered.load(), std::size_t{8});
}

QTEST_GUILESS_MAIN(RunEventDeliveryTests)
#include "RunEventDeliveryTests.moc"
