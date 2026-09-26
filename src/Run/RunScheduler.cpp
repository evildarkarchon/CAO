#include "RunScheduler.h"

#include <atomic>
#include <cstdio>
#include <exception>
#include <thread>
#include <utility>

namespace cao::run {
namespace {
/// A worker whose work already ran, so joining it has nothing left to wait for.
class CompletedRunWorker final : public ScheduledRunWorker {
   public:
    // The inline scheduler finished the work before returning this, so joining is a no-op rather
    // than a missing implementation.
    void join() override {}

    /// An inline worker has no running thread once its scheduler returns it.
    [[nodiscard]] bool isCurrentThread() const noexcept override { return false; }
};

/// Owns one production thread and defensively joins it before releasing its resources.
class StandardRunWorker final : public ScheduledRunWorker {
   public:
    /// Allocates thread bookkeeping before starting work, preserving the scheduling guarantee.
    explicit StandardRunWorker(std::function<void()> work)
        : _finished(std::make_shared<std::atomic<bool>>(false)),
          _thread([work = std::move(work), finished = _finished] {
              work();
              finished->store(true, std::memory_order_release);
          }),
          _threadId(_thread.get_id()) {}

    /// Joins defensively even when a caller outside the run lifetime releases the worker.
    ~StandardRunWorker() override { join(); }

    /// Joins once, diagnosing self-join before standard thread destruction could deadlock.
    void join() override {
        if (isCurrentThread()) {
            std::fputs("An Optimization Run cannot join or destroy its own worker\n", stderr);
            std::terminate();
        }
        if (_thread.joinable()) _thread.join();
    }

    /// Uses immutable identity because querying the thread object could race a concurrent join.
    [[nodiscard]] bool isCurrentThread() const noexcept override {
        // An exited thread's ID may be reused while its old handle is still retained.
        return !_finished->load(std::memory_order_acquire) &&
               _threadId == std::this_thread::get_id();
    }

   private:
    std::shared_ptr<std::atomic<bool>> _finished;
    std::jthread _thread;
    const std::thread::id _threadId;
};
}  // namespace

std::unique_ptr<ScheduledRunWorker> StandardRunScheduler::schedule(std::function<void()> work) {
    return std::make_unique<StandardRunWorker>(std::move(work));
}

std::unique_ptr<ScheduledRunWorker> InlineRunScheduler::schedule(std::function<void()> work) {
    // Allocation must precede work: a failure after invocation would incorrectly let the service
    // commit a scheduling failure after that work had already committed its real result.
    auto worker = std::make_unique<CompletedRunWorker>();
    // Running before returning is what makes the inline seam deterministic: the Optimization Run
    // service observes a terminal run the moment scheduling succeeds.
    work();
    return worker;
}
}  // namespace cao::run
