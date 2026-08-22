#pragma once

#include "OptionsCAO.h"
#include "Run/RunSetup.h"

#include <QStringList>

namespace cao::run
{
/// Snapshots the current application choices and selected profile, then compiles one Routing Policy outcome.
[[nodiscard]] routing::RoutingPolicyBuildResult prepareApplicationRun(
    const OptionsCAO &options);

/// Presents structured policy conflicts without inspecting or parsing exception text.
[[nodiscard]] QStringList policyValidationErrorMessages(
    std::span<const routing::PolicyValidationError> errors);
}
