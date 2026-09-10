#pragma once

#include "Run/RunLifecycle.h"

namespace cao::gui {
/// Presentation facts copied from ordered service observations; no widget or worker ownership.
struct RunViewState final {
    std::string runId;
    std::string label;
    std::optional<run::RunProgress> progress;
    std::vector<std::string> details;
    bool active{};
    bool cancellationRequested{};
    bool closeRequested{};
    std::optional<run::RunOutcome> outcome;
};

/// Maps immutable run observations on the GUI thread; callers serialize all access.
class RunViewModel final {
   public:
    /// Clears the previous presentation and binds subsequent observations to this run identity.
    void begin(std::string runId);
    /// Accepts a newer observation for the bound run; returns false for stale/foreign events.
    bool consume(const run::RunEvent& event);
    /// Presents cooperative cancellation intent without inventing a terminal outcome.
    void requestCancellation();
    /// Defers an active window close through cooperative cancellation and terminal delivery.
    /// Returns true when no active run remains and the caller may close immediately.
    [[nodiscard]] bool requestClose();
    /// Keeps restart disabled between terminal delivery and the deferred window close.
    [[nodiscard]] bool canStart() const noexcept { return !_state.active && !_state.closeRequested; }
    /// Borrows presentation state until the next mutation; GUI-thread access only.
    [[nodiscard]] const RunViewState& state() const noexcept { return _state; }

   private:
    RunViewState _state;
    std::uint64_t _sequence{};
};
}  // namespace cao::gui
