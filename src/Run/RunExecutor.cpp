#include "RunExecutor.h"

#include <utility>
#include <exception>
#include <vector>

namespace cao::run {
namespace {
/// Resolves one existing directory without enumerating or changing any Asset or Archive.
std::variant<std::filesystem::path, RunFailure> resolveSingleModRoot(
    const ModSelection& selection) {
    if (selection.kind() != ModSelectionKind::SingleModRoot)
        return RunFailure{RunFailureCode::ModSelectionResolutionFailed, RunPhase::Preparing,
                          "Child Mod Root preparation is not yet available"};
    std::error_code error;
    auto root = std::filesystem::canonical(selection.directory(), error);
    if (error || !std::filesystem::is_directory(root, error))
        return RunFailure{RunFailureCode::ModSelectionResolutionFailed, RunPhase::Preparing,
                          "The selected Mod Root could not be resolved to an existing directory"};
    return root;
}

/// Loads independent configuration values and converts provider exceptions into run failures.
std::variant<RunConfiguration, RunFailure> loadConfiguration(
    const RunRequest& request, const RunConfigurationProvider* provider) {
    if (provider == nullptr)
        return RunFailure{RunFailureCode::ConfigurationLoadingFailed, RunPhase::Preparing,
                          "No run configuration provider is available"};
    try {
        return provider->load(request.profileIdentity());
    } catch (const std::exception& error) {
        return RunFailure{RunFailureCode::ConfigurationLoadingFailed, RunPhase::Preparing,
                          error.what()};
    } catch (...) {
        return RunFailure{RunFailureCode::ConfigurationLoadingFailed, RunPhase::Preparing,
                          "The configuration provider threw a non-standard exception"};
    }
}

/// Prepares immutable facts without mutation; a null success value means loading was cancelled.
std::variant<std::shared_ptr<const RunPreparation>, RunFailure> prepareSingleModRun(
    const RunRequest& request, const RunConfigurationProvider* provider, std::stop_token stop) {
    auto loaded = loadConfiguration(request, provider);
    if (auto* failure = std::get_if<RunFailure>(&loaded)) return std::move(*failure);
    // A provider may finish an atomic read after cancellation. Do not resolve roots or compile
    // additional facts once that read returns and the cancellation can be observed safely.
    if (stop.stop_requested()) return std::shared_ptr<const RunPreparation>{};

    auto configuration = std::move(std::get<RunConfiguration>(loaded));
    const auto policy =
        RunSetup::prepare(routing::RoutingPolicyRequest::forWork(
                              request.executionMode(),
                              std::vector<routing::RequestedWork>(request.requestedWork().begin(),
                                                                  request.requestedWork().end())),
                          configuration.profile());
    if (!policy.hasPolicy())
        return RunFailure{
            RunFailureCode::PolicyConflict, RunPhase::Preparing,
            "The loaded profile conflicts with the requested Routing Policy",
            routing::PolicyValidationErrors(policy.errors().begin(), policy.errors().end())};

    auto resolved = resolveSingleModRoot(request.modSelection());
    if (auto* failure = std::get_if<RunFailure>(&resolved)) return std::move(*failure);
    return std::make_shared<const RunPreparation>(
        std::vector{std::move(std::get<std::filesystem::path>(resolved))}, std::move(configuration),
        *policy.policy(), request.archivePrecedence());
}

/// Records the work phases a request with no requested work skips, in canonical order.
///
/// Every phase reports the one reason the run actually knows: nothing was requested. A skipped
/// phase must not report the outcome of a phase that never ran, so a run that skipped discovery
/// cannot claim that no Archives were discovered, and one that skipped routing cannot claim there
/// were no Routed Assets. Execution mode is deliberately not consulted either: a Dry Run that was
/// asked for nothing is excluded by the empty request, not by its mode.
/// Returns the last traversed work phase, observing cancellation before each transition so an
/// inline observation can stop traversal without inventing skipped phases after cancellation.
RunPhase recordSkippedWorkPhases(std::vector<RunPhaseRecord>& phases,
                                 RunObservationSink* observations, std::stop_token stop) {
    auto finalPhase = RunPhase::Preparing;
    for (const auto phase : {RunPhase::DiscoveringArchives, RunPhase::ExtractingArchives,
                             RunPhase::BuildingEffectiveAssetTree, RunPhase::ProcessingAssets,
                             RunPhase::ArchiveFinalization}) {
        if (stop.stop_requested()) break;
        phases.push_back(RunPhaseRecord::skipped(phase, PhaseSkipReason::NoRequestedWork));
        finalPhase = phase;
        if (observations != nullptr) observations->recordPhase(phases.back());
    }
    return finalPhase;
}
}  // namespace

OptimizationRunResult RunExecutor::execute(const RunRequest& request, const RunServices& services,
                                           std::stop_token stop, RunId runId) const {
    std::vector<RunPhaseRecord> phases;
    std::vector<RunFailure> failures;
    phases.reserve(runPhaseSequence().size());

    // Preparing always executes: it is where the request becomes run-scoped state. It is
    // indeterminate work, so it reports no progress rather than a total of one.
    phases.push_back(RunPhaseRecord::executed(RunPhase::Preparing));
    if (services.observations != nullptr) services.observations->recordPhase(phases.back());
    auto finalPhase = RunPhase::Preparing;
    auto outcome = RunOutcome::Succeeded;
    std::shared_ptr<const RunPreparation> preparation;
    if (!stop.stop_requested()) {
        auto prepared = prepareSingleModRun(request, services.configuration, stop);
        if (auto* failure = std::get_if<RunFailure>(&prepared)) {
            outcome = RunOutcome::Failed;
            failures.push_back(std::move(*failure));
            if (services.observations != nullptr)
                services.observations->recordFailure(failures.back());
        } else {
            preparation = std::move(std::get<std::shared_ptr<const RunPreparation>>(prepared));
        }
    }

    if (outcome == RunOutcome::Failed) {
        // Preparation failure stops traversal, but never bypasses the mandatory cleanup pass.
    } else if (stop.stop_requested()) {
        outcome = RunOutcome::Cancelled;
    } else if (request.hasRequestedWork()) {
        // Requested work needs service seams this slice does not yet own. Traversing the work
        // phases here would report a Succeeded run that touched nothing, so Preparing fails and
        // the run still reaches Safety Cleanup. Later lifecycle slices replace this branch with
        // real discovery, Asset processing, and Archive Finalization.
        outcome = RunOutcome::Failed;
        failures.emplace_back(RunFailureCode::RequestedWorkUnavailable, RunPhase::Preparing,
                              "Requested work requires run services that are not yet available");
        if (services.observations != nullptr) services.observations->recordFailure(failures.back());
    } else {
        finalPhase = recordSkippedWorkPhases(phases, services.observations, stop);
    }

    // Safety Cleanup runs exactly once on every terminal path, before the terminal result is
    // committed, so cancellation and failure cannot litter Mod Roots with run-owned artifacts.
    services.safetyCleanup.performSafetyCleanup();
    phases.push_back(RunPhaseRecord::executed(RunPhase::SafetyCleanup));
    if (services.observations != nullptr) services.observations->recordPhase(phases.back());

    // A fatal failure keeps precedence; cancellation observed during cleanup still records a
    // cancelled run, without ever interrupting the cleanup pass.
    if (outcome != RunOutcome::Failed && stop.stop_requested()) outcome = RunOutcome::Cancelled;

    return OptimizationRunResult::terminal(outcome, finalPhase, std::move(phases), std::move(runId),
                                           std::move(failures), std::move(preparation));
}
}  // namespace cao::run
