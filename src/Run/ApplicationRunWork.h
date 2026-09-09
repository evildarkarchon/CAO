#pragma once

#include "Run/RunExecutor.h"
#include <memory>

class OptionsCAO;

namespace cao::run {
/// Captures caller-owned options and profile data before scheduling; returned work owns every value.
/// Optimizers and Qt objects are constructed and destroyed only on the run execution thread.
[[nodiscard]] std::shared_ptr<RunWorkService> makeApplicationRunWork(const OptionsCAO& options);
}  // namespace cao::run
