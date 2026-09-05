#include "RunExecutor.h"

#include <utf8proc.h>

#include <algorithm>
#include <cstdlib>
#include <stdexcept>
#include <unordered_set>
#include <utility>
#include <exception>
#include <vector>

namespace cao::run {
namespace {
/// Folds UTF-8 names independently of the process locale; invalid encoding throws to Preparing.
std::string foldedName(std::string_view name) {
    utf8proc_uint8_t* mapped = nullptr;
    const auto size =
        utf8proc_map(reinterpret_cast<const utf8proc_uint8_t*>(name.data()),
                     static_cast<utf8proc_ssize_t>(name.size()), &mapped, UTF8PROC_CASEFOLD);
    const std::unique_ptr<utf8proc_uint8_t, decltype(&std::free)> owned(mapped, &std::free);
    if (size < 0) throw std::runtime_error(utf8proc_errmsg(size));
    return std::string(reinterpret_cast<const char*>(owned.get()), static_cast<std::size_t>(size));
}

/// Returns normalized generic UTF-8 without depending on the Windows ANSI code page.
std::string relativeName(const std::filesystem::path& path) {
    const auto utf8 = path.lexically_normal().generic_u8string();
    return std::string(utf8.begin(), utf8.end());
}

/// Tests existing directory identities, including platform-specific case and path aliases.
/// Filesystem lookup failures propagate to Preparing instead of accepting uncertain containment.
bool containsDirectory(const std::filesystem::path& boundary, std::filesystem::path directory) {
    // String prefixes confuse siblings such as Mod and Mod2, and cannot recognize filesystem
    // aliases.
    for (;;) {
        if (std::filesystem::equivalent(boundary, directory)) return true;
        auto parent = directory.parent_path();
        if (parent == directory || parent.empty()) return false;
        directory = std::move(parent);
    }
}

/// Resolves independent roots without recursion or mutation; lookup errors fail all preparation.
/// Each linked selection is resolved once, and overlapping directory identities are rejected.
std::variant<std::vector<std::filesystem::path>, RunFailure> resolveModRoots(
    const ModSelection& selection, const RunConfiguration& configuration,
    RunObservationSink* observations, std::stop_token stop) {
    try {
        std::error_code error;
        auto root = std::filesystem::canonical(selection.directory(), error);
        if (error || !std::filesystem::is_directory(root, error))
            return RunFailure{
                RunFailureCode::ModSelectionResolutionFailed, RunPhase::Preparing,
                "The selected Mod Root could not be resolved to an existing directory"};
        if (selection.kind() == ModSelectionKind::SingleModRoot)
            return std::vector{std::move(root)};

        std::unordered_set<std::string> ignoredNames;
        for (const auto& ignored : configuration.ignoredMods())
            ignoredNames.insert(foldedName(ignored));

        struct Child {
            std::filesystem::path path;
            std::string name;
            std::string folded;
        };
        std::vector<Child> children;
        for (const auto& entry : std::filesystem::directory_iterator(root)) {
            if (stop.stop_requested()) return std::vector<std::filesystem::path>{};
            if (!entry.is_directory()) continue;
            auto name = relativeName(entry.path().lexically_relative(root));
            auto folded = foldedName(name);
            children.push_back({entry.path(), std::move(name), std::move(folded)});
        }
        std::sort(children.begin(), children.end(), [](const Child& left, const Child& right) {
            if (left.folded != right.folded) return left.folded < right.folded;
            return left.name < right.name;
        });
        std::vector<std::filesystem::path> roots;
        for (const auto& child : children) {
            if (stop.stop_requested()) return std::vector<std::filesystem::path>{};
            const auto markers = configuration.separatorMarkers();
            const bool separator =
                std::any_of(markers.begin(), markers.end(), [&](const auto& marker) {
                    return !marker.empty() && child.name.find(marker) != std::string::npos;
                });
            // A child matching both policies owes one exclusion. Preserve separator precedence.
            if (separator || ignoredNames.contains(child.folded)) {
                if (observations != nullptr)
                    observations->recordDiagnostic(RunDiagnostic{
                        separator ? RunDiagnosticCode::SeparatorModExcluded
                                  : RunDiagnosticCode::IgnoredModExcluded,
                        RunPhase::Preparing,
                        separator ? "The child Mod Root matches a configured separator marker"
                                  : "The child Mod Root matches an ignored-mod name",
                        child.path});
                continue;
            }
            // Sort the selected entry names before resolving links: target names do not define run
            // order.
            auto resolved = std::filesystem::canonical(child.path);
            for (const auto& existing : roots) {
                if (containsDirectory(existing, resolved) || containsDirectory(resolved, existing))
                    return RunFailure{RunFailureCode::ConflictingModRoots, RunPhase::Preparing,
                                      "The selected Mod Roots overlap: " + relativeName(existing) +
                                          " and " + relativeName(child.path)};
            }
            roots.push_back(std::move(resolved));
        }
        return roots;
    } catch (const std::exception& error) {
        return RunFailure{RunFailureCode::ModSelectionResolutionFailed, RunPhase::Preparing,
                          error.what()};
    }
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
std::variant<std::shared_ptr<const RunPreparation>, RunFailure> prepareRun(
    const RunRequest& request, const RunConfigurationProvider* provider,
    RunObservationSink* observations, std::stop_token stop) {
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

    auto resolved = resolveModRoots(request.modSelection(), configuration, observations, stop);
    if (auto* failure = std::get_if<RunFailure>(&resolved)) return std::move(*failure);
    if (stop.stop_requested()) return std::shared_ptr<const RunPreparation>{};
    return std::make_shared<const RunPreparation>(
        std::move(std::get<std::vector<std::filesystem::path>>(resolved)), std::move(configuration),
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

std::vector<RunFailure> collectSafetyCleanupFailures(SafetyCleanupService& service) {
    try {
        return service.performSafetyCleanup();
    } catch (const std::exception& error) {
        return {RunFailure{RunFailureCode::SafetyCleanupServiceFailed, RunPhase::SafetyCleanup,
                           error.what()}};
    } catch (...) {
        return {RunFailure{RunFailureCode::SafetyCleanupServiceFailed, RunPhase::SafetyCleanup,
                           "The cleanup service threw a non-standard exception"}};
    }
}

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
        auto prepared = prepareRun(request, services.configuration, services.observations, stop);
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
    phases.push_back(RunPhaseRecord::executed(RunPhase::SafetyCleanup));
    if (services.observations != nullptr) services.observations->recordPhase(phases.back());
    auto cleanupFailures = collectSafetyCleanupFailures(services.safetyCleanup);
    for (const auto& failure : cleanupFailures)
        if (services.observations != nullptr) services.observations->recordFailure(failure);
    // A service-contract exception cannot establish that all owned artifacts were attempted.
    if (std::any_of(cleanupFailures.begin(), cleanupFailures.end(), [](const RunFailure& failure) {
            return failure.code() == RunFailureCode::SafetyCleanupServiceFailed;
        }))
        outcome = RunOutcome::Failed;

    // A fatal failure keeps precedence; cancellation observed during cleanup still records a
    // cancelled run, without ever interrupting the cleanup pass.
    if (outcome != RunOutcome::Failed && stop.stop_requested()) outcome = RunOutcome::Cancelled;
    if (outcome == RunOutcome::Succeeded && !cleanupFailures.empty())
        outcome = RunOutcome::CompletedWithFailures;

    return OptimizationRunResult::terminal(outcome, finalPhase, std::move(phases), std::move(runId),
                                           std::move(failures), std::move(preparation),
                                           std::move(cleanupFailures));
}
}  // namespace cao::run
