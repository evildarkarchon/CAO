#pragma once

#include "Run/OptimizationRunService.h"
#include <functional>
#include <ostream>

namespace cao::cli {
/// Maps the shared terminal classification to the stable shell status.
[[nodiscard]] int exitCode(run::RunOutcome outcome) noexcept;

/// Writes one ordered service observation without deriving lifecycle state or progress.
void renderEvent(std::ostream& output, const run::RunEvent& event);

/// Starts a run, forwards interrupt intent, and waits through Safety Cleanup and terminal delivery.
/// The owned output remains alive through inline observer execution; interruption is polled only
/// on the calling thread. The service owns all work scheduling and cancellation boundaries.
[[nodiscard]] int run(run::OptimizationRunService& service, run::RunRequest request,
                      std::shared_ptr<std::ostream> output,
                      const std::function<bool()>& interrupted);

/// Installs process interrupt handling for one CLI invocation and restores it after the run joins.
/// Repeated interrupts only latch cancellation intent; handlers never access a Run Handle.
class ConsoleInterrupt final {
   public:
    ConsoleInterrupt();
    ~ConsoleInterrupt();
    ConsoleInterrupt(const ConsoleInterrupt&) = delete;
    ConsoleInterrupt& operator=(const ConsoleInterrupt&) = delete;
    /// Returns whether a console interrupt has been received since installation.
    [[nodiscard]] bool requested() const noexcept;

   private:
    using SignalHandler = void (*)(int);
    SignalHandler _previous{};
};
}  // namespace cao::cli
