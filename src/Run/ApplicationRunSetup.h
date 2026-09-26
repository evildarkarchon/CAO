#pragma once

#include "OptionsCAO.h"
#include "Run/RunSetup.h"
#include "Run/RunLifecycle.h"
#include "Run/RunPreparation.h"

#include <QString>
#include <QStringList>
#include <memory>

struct OptimizerProfileSnapshot;

namespace cao::run {
/// Loads routing and backend facts from one selected profile.ini during Preparing.
/// The executor calls load and then preparedOptimizerProfile on the same execution thread.
class ApplicationRunConfigurationProvider final : public RunConfigurationProvider {
   public:
    /// Owns the absolute profiles directory before asynchronous execution can change context.
    explicit ApplicationRunConfigurationProvider(QString profilesDirectory);

    /// Loads the named profile and publishes its immutable backend snapshot on success.
    [[nodiscard]] RunConfiguration load(std::string_view identity) const override;

    /// Returns the backend facts published by the last successful load, or null before loading.
    [[nodiscard]] std::shared_ptr<const OptimizerProfileSnapshot> preparedOptimizerProfile() const;

   private:
    QString _profilesDirectory;
    mutable std::shared_ptr<const OptimizerProfileSnapshot> _optimizerProfile;
};

/// Captures owned user intent on the caller thread; filesystem preparation remains with the
/// service. Throws std::invalid_argument for invalid mode, mesh level, or texture dimensions and
/// ratios.
[[nodiscard]] RunRequest makeApplicationRunRequest(const OptionsCAO& options);

/// Captures the absolute profiles directory; each load owns its QSettings on the execution thread.
[[nodiscard]] std::shared_ptr<const ApplicationRunConfigurationProvider>
makeApplicationRunConfigurationProvider();

/// Snapshots the current application choices and selected profile, then compiles one Routing Policy
/// outcome.
[[nodiscard]] routing::RoutingPolicyBuildResult prepareApplicationRun(const OptionsCAO& options);

/// Presents structured policy conflicts without inspecting or parsing exception text.
[[nodiscard]] QStringList policyValidationErrorMessages(
    std::span<const routing::PolicyValidationError> errors);
}  // namespace cao::run
