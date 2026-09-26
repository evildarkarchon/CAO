#pragma once

#include "Run/OptimizationRunService.h"

class QObject;

namespace cao::gui {
/// Creates an observation delivered through the target thread's Qt event queue, never inline.
/// Call on the live target's thread; destroy the target on that thread too. The observer may
/// borrow the target: queued delivery is discarded after its destruction. The dispatcher owns
/// a separate context, so worker dispatch remains safe even after the target has been destroyed.
[[nodiscard]] run::RunObservation queuedObservation(QObject* target, run::RunObserver observer);
}  // namespace cao::gui
