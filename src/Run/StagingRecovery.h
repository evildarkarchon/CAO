#pragma once

#include "Run/RunLifecycle.h"

#include <memory>
#include <optional>
#include <stop_token>

namespace cao::run {
/// Recovers only manifest-owned temporary entries, retaining OS locks through Safety Cleanup.
/// The executor owns one instance on its execution thread; destruction releases locks without
/// deleting control files. Absent staging is not created by recovery.
class StagingRecovery final {
   public:
    /// Starts an empty recovery scope without filesystem access.
    StagingRecovery();
    /// Releases all retained OS locks without deleting control files or retrying cleanup.
    ~StagingRecovery();
    StagingRecovery(const StagingRecovery&) = delete;
    StagingRecovery& operator=(const StagingRecovery&) = delete;

    /// Checks a canonical Mod Root and recovers its verified stale staging. Returns actionable
    /// Preparing failures for active, unknown, or inaccessible contents. Cancellation returns no
    /// failure and leaves unattempted entries intact; the executor owns its outcome. Apply only.
    [[nodiscard]] std::optional<RunFailure> recover(const std::filesystem::path& modRoot,
                                                    std::stop_token stop = {});

   private:
    struct State;
    std::unique_ptr<State> _state;
};
}  // namespace cao::run
