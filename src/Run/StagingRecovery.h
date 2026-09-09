#pragma once

#include "Run/RunLifecycle.h"

#include <memory>
#include <optional>
#include <stop_token>

namespace cao::run {
/// Produces and recovers manifest-owned temporary entries, retaining OS locks through Safety
/// Cleanup. The executor owns one instance on its execution thread; destruction releases locks
/// without deleting control files. Absent staging is not created by recovery.
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

    /// Registers a unique sibling temporary name durably, then exclusively creates its empty file.
    /// Reuses recovered ownership locks; throws if ownership or same-root containment fails.
    [[nodiscard]] std::filesystem::path stageFile(const std::filesystem::path& modRoot,
                                                  const std::filesystem::path& destination);
    /// Durably registers and exclusively creates a unique empty file beneath the owned run child.
    /// Accepts arbitrary Archive entry bytes; throws on invalid roots, ownership, or I/O failures.
    [[nodiscard]] std::filesystem::path stageArchiveFile(const std::filesystem::path& modRoot);
    /// Flushes removal of a temporary registration after its file was committed elsewhere.
    /// The destination is deliberately never part of this protocol or its deletion records. Throws
    /// `logic_error` if the file still exists or the supplied path is not registered.
    void releaseFile(const std::filesystem::path& temporary);
    /// Removes verified durable artifacts and empty run children, retaining locks until
    /// destruction. Returns every cleanup failure after attempting all independently owned
    /// artifacts.
    [[nodiscard]] std::vector<RunFailure> cleanupArtifacts();

   private:
    /// Acquires or reuses root ownership and publishes the run child before any staged bytes.
    void prepareArea(const std::filesystem::path& root);
    /// Publishes one file registration before creation, rolling back names rejected by CREATE_NEW.
    [[nodiscard]] std::filesystem::path createRegisteredFile(const std::filesystem::path& root,
                                                              const std::filesystem::path& relative,
                                                              bool rootRelative);
    struct State;
    std::unique_ptr<State> _state;
};
}  // namespace cao::run
