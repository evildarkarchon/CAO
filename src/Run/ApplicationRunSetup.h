#pragma once

#include "OptionsCAO.h"
#include "Run/RunSetup.h"
#include "Run/RunLifecycle.h"

#include <QStringList>
#include <memory>

namespace cao::run {
/// Captures owned user intent on the caller thread; filesystem preparation remains with the service.
/// Throws std::invalid_argument for invalid mode, mesh level, or texture dimensions and ratios.
[[nodiscard]] RunRequest makeApplicationRunRequest(const OptionsCAO& options);

/// Captures the absolute profiles directory; each load owns its QSettings on the execution thread.
[[nodiscard]] std::shared_ptr<const RunConfigurationProvider>
makeApplicationRunConfigurationProvider();

/// Snapshots the current application choices and selected profile, then compiles one Routing Policy
/// outcome.
[[nodiscard]] routing::RoutingPolicyBuildResult prepareApplicationRun(const OptionsCAO& options);

/// Presents structured policy conflicts without inspecting or parsing exception text.
[[nodiscard]] QStringList policyValidationErrorMessages(
    std::span<const routing::PolicyValidationError> errors);
}  // namespace cao::run
