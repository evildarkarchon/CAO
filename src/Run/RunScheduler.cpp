#include "RunScheduler.h"

#include <utility>

namespace cao::run {
namespace {
/// A worker whose work already ran, so joining it has nothing left to wait for.
class CompletedRunWorker final : public ScheduledRunWorker {
   public:
    // The inline scheduler ran the work before constructing this, so joining is a no-op rather
    // than a missing implementation.
    void join() override {}
};
}  // namespace

std::unique_ptr<ScheduledRunWorker> InlineRunScheduler::schedule(std::function<void()> work) {
    // Running before returning is what makes the inline seam deterministic: the Optimization Run
    // service observes a terminal run the moment scheduling succeeds.
    work();
    return std::make_unique<CompletedRunWorker>();
}
}  // namespace cao::run
