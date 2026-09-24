#pragma once

#include "Run/RunExecutor.h"
#include <memory>

class OptionsCAO;

namespace cao::run {
class ApplicationRunConfigurationProvider;

/// Captures caller-owned options before scheduling and shares the provider's Preparing snapshot.
/// Optimizers and Qt objects are constructed and destroyed only on the run execution thread.
[[nodiscard]] std::shared_ptr<RunWorkService> makeApplicationRunWork(
    const OptionsCAO& options,
    std::shared_ptr<const ApplicationRunConfigurationProvider> configuration);
}  // namespace cao::run
