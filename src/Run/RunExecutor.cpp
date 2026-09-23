#include "RunExecutor.h"
#include "PathOrdering.h"
#include "RunEvidence.h"
#include "StagingRecovery.h"
#include "RunWorkRecord.h"
#include "TemporaryArtifactRegistry.h"

#include <algorithm>
#include <cstdlib>
#include <stdexcept>
#include <unordered_set>
#include <utility>
#include <exception>
#include <vector>

namespace cao::run {
namespace {
/// Adapts executor and work facts into the Run Evidence retention/publication boundary.
class WorkObservations final : public RunObservationSink {
   public:
    /// Borrows worker-confined evidence and the compatibility work record through cleanup.
    WorkObservations(MutableRunEvidence& evidence, RunWorkRecord& work,
                     routing::ExecutionMode mode)
        : _evidence(evidence), _work(work), _mode(mode) {}

    /// Replaces a phase's latest counts without losing its traversal position.
    void recordPhase(const RunPhaseRecord& phase) override {
        _evidence.recordPhase(phase);
    }

    /// Translates work milestones into the executor's canonical lifecycle account.
    RunPhaseRecord archiveDiscoveryStarted() override {
        _evidence.recordArchiveDiscoveryStarted();
        return *_evidence.currentPhase();
    }
    /// Starts Archive extraction with the discovered immutable attempt total.
    RunPhaseRecord archiveExtractionPlanned(const std::size_t total) override {
        _evidence.recordArchiveExtractionPlan(total);
        return *_evidence.currentPhase();
    }
    /// Excludes Archive extraction when the prepared policy is a Dry Run.
    RunPhaseRecord dryRunArchiveExtraction() override {
        _evidence.recordDryRunArchiveExtraction();
        return *_evidence.currentPhase();
    }
    /// Starts definitive Effective Asset Tree discovery after Archive handling.
    RunPhaseRecord effectiveAssetTreeStarted() override {
        _evidence.recordEffectiveAssetTreeStarted();
        return *_evidence.currentPhase();
    }
    /// Starts Asset processing with the retained Routing Ledger's exact work total.
    RunPhaseRecord assetProcessingPlanned(const std::size_t total) override {
        _evidence.recordAssetProcessingPlan(total);
        return *_evidence.currentPhase();
    }
    /// Selects the final work phase from the request mode and available finalizer.
    /// Throws RunEvidenceInvariantViolation if work changes the prepared execution mode.
    RunPhaseRecord archiveFinalizationAvailable(const routing::ExecutionMode mode,
                                                const bool hasFinalizer) override {
        if (mode != _mode)
            throw RunEvidenceInvariantViolation("Work mode differs from the Run Request");
        const auto phase = _mode == routing::ExecutionMode::DryRun
                               ? RunPhaseRecord::skipped(RunPhase::ArchiveFinalization,
                                                         PhaseSkipReason::DryRun)
                               : hasFinalizer
                                     ? RunPhaseRecord::executed(RunPhase::ArchiveFinalization)
                                     : RunPhaseRecord::skipped(
                                           RunPhase::ArchiveFinalization,
                                           PhaseSkipReason::NoRequestedWork);
        recordPhase(phase);
        return phase;
    }

    /// Retains a new work failure in both migration stores before publishing from Run Evidence.
    void recordFailure(const RunFailure& failure) override {
        _work.failures.push_back(failure);
        _evidence.recordFailure(failure);
    }

    /// Retains a new informational observation before publishing from Run Evidence.
    void recordDiagnostic(const RunDiagnostic& diagnostic) override {
        _work.diagnostics.push_back(diagnostic);
        // AssetRun still carries the transitional cursor; keep it past facts whose real
        // publication position is already owned by Run Evidence.
        _work.publishedDiagnostics = _work.diagnostics.size();
        _evidence.recordDiagnostic(diagnostic);
    }

    /// Publishes a work-owned diagnostic through Run Evidence without duplicating the work copy.
    void publishRetainedDiagnostic(const RunDiagnostic&) override {
        _evidence.publishDiagnostics();
    }

    /// Hands deferred work evidence to Run Evidence without changing its publication boundary.
    void retainDiagnostic(const RunDiagnostic& diagnostic) override {
        _evidence.retainDiagnostic(diagnostic);
    }

    /// Publishes a work-owned failure through Run Evidence without duplicating the work copy.
    void publishRetainedFailure(const RunFailure& failure) override {
        _evidence.recordFailure(failure);
    }

    /// Retains the frozen Archive output count before packing can commit an output.
    void recordArchiveFinalizationPlan(const std::size_t total) override {
        _evidence.recordArchiveFinalizationPlan(total);
    }

    /// Retains each committed or failed output before its progress event is published.
    void recordArchiveFinalizationAttempt(const ArchiveFinalizationAttempt& attempt,
                                          const std::size_t total) override {
        _evidence.recordArchiveFinalizationAttempt(attempt, total);
    }

   private:
    MutableRunEvidence& _evidence;
    RunWorkRecord& _work;
    routing::ExecutionMode _mode;
};

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
/// A filesystem root cannot bound one mod or a mods directory safely.
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
        if (root == root.root_path())
            return RunFailure{RunFailureCode::ModSelectionResolutionFailed, RunPhase::Preparing,
                              "A filesystem root cannot be selected as a Mod Root or mods directory"};
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

/// Prepares immutable facts without mutation; an absent success value means loading was cancelled.
std::variant<std::optional<RunPreparation>, RunFailure> prepareRun(
    const RunRequest& request, const RunConfigurationProvider* provider,
    RunObservationSink* observations, std::stop_token stop) {
    auto loaded = loadConfiguration(request, provider);
    if (auto* failure = std::get_if<RunFailure>(&loaded)) return std::move(*failure);
    // A provider may finish an atomic read after cancellation. Do not resolve roots or compile
    // additional facts once that read returns and the cancellation can be observed safely.
    if (stop.stop_requested()) return std::optional<RunPreparation>{};

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
    if (stop.stop_requested()) return std::optional<RunPreparation>{};
    return std::optional<RunPreparation>{
        std::in_place, std::move(std::get<std::vector<std::filesystem::path>>(resolved)),
        std::move(configuration), *policy.policy(), request.archivePrecedence()};
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
RunPhase recordSkippedWorkPhases(RunObservationSink& observations, std::stop_token stop) {
    auto finalPhase = RunPhase::Preparing;
    for (const auto phase : {RunPhase::DiscoveringArchives, RunPhase::ExtractingArchives,
                             RunPhase::BuildingEffectiveAssetTree, RunPhase::ProcessingAssets,
                             RunPhase::ArchiveFinalization}) {
        if (stop.stop_requested()) break;
        const auto record = RunPhaseRecord::skipped(phase, PhaseSkipReason::NoRequestedWork);
        finalPhase = phase;
        observations.recordPhase(record);
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

OptimizationRunResult RunExecutor::schedulingFailure(
    std::string detail, SafetyCleanupService& cleanup, RunObservationSink* observations,
    const std::stop_token stop, RunId runId) const {
    MutableRunEvidence evidence{observations};
    std::vector<RunFailure> failures{
        RunFailure{RunFailureCode::SchedulingFailed, RunPhase::Preparing, std::move(detail)}};
    evidence.recordFailure(failures.front());
    evidence.recordPhase(RunPhaseRecord::executed(RunPhase::SafetyCleanup));
    auto cleanupFailures = collectSafetyCleanupFailures(cleanup);
    for (const auto& failure : cleanupFailures) evidence.recordSafetyCleanupFailure(failure);
    if (stop.stop_requested()) evidence.recordCancellationObservation();
    return OptimizationRunResult::terminal(RunOutcome::Failed, RunPhase::Preparing,
                                           std::move(evidence).consume(), std::move(runId),
                                           std::move(failures), std::move(cleanupFailures));
}

OptimizationRunResult RunExecutor::execute(const RunRequest& request, const RunServices& services,
                                           std::stop_token stop, RunId runId) const {
    MutableRunEvidence evidence{services.observations};
    std::vector<RunFailure> failures;
    RunWorkRecord work;

    // Preparing always executes: it is where the request becomes run-scoped state. It is
    // indeterminate work, so it reports no progress rather than a total of one.
    auto finalPhase = RunPhase::Preparing;
    WorkObservations observations(evidence, work, request.executionMode());
    observations.recordPhase(RunPhaseRecord::executed(RunPhase::Preparing));
    auto outcome = RunOutcome::Succeeded;
    std::exception_ptr evidenceInvariantViolation;
    std::optional<RunPreparation> pendingPreparation;
    const RunPreparation* preparation{};
    StagingRecovery staging;
    TemporaryArtifactRegistry artifacts(&staging);
    if (!stop.stop_requested()) {
        auto prepared = prepareRun(request, services.configuration, &observations, stop);
        if (auto* failure = std::get_if<RunFailure>(&prepared)) {
            outcome = RunOutcome::Failed;
            failures.push_back(std::move(*failure));
            observations.publishRetainedFailure(failures.back());
        } else {
            auto completed = std::move(std::get<std::optional<RunPreparation>>(prepared));
            if (completed) pendingPreparation.emplace(std::move(*completed));
        }
    }

    if (pendingPreparation && request.hasRequestedWork() && services.work &&
        !stop.stop_requested()) {
        try {
            services.work->prepare();
        } catch (const std::exception& error) {
            outcome = RunOutcome::Failed;
            failures.emplace_back(RunFailureCode::ConfigurationLoadingFailed, RunPhase::Preparing,
                                  error.what());
        } catch (...) {
            outcome = RunOutcome::Failed;
            failures.emplace_back(RunFailureCode::ConfigurationLoadingFailed, RunPhase::Preparing,
                                  "Work configuration threw a non-standard exception");
        }
        if (outcome == RunOutcome::Failed) observations.publishRetainedFailure(failures.back());
    }

    if (pendingPreparation && outcome != RunOutcome::Failed &&
        request.executionMode() == routing::ExecutionMode::Apply) {
        for (const auto& root : pendingPreparation->modRoots()) {
            if (stop.stop_requested()) break;
            if (auto failure = staging.recover(root, stop)) {
                outcome = RunOutcome::Failed;
                failures.push_back(std::move(*failure));
                observations.publishRetainedFailure(failures.back());
                break;
            }
        }
    }

    if (pendingPreparation && outcome != RunOutcome::Failed && request.hasRequestedWork() &&
        services.work == nullptr && !stop.stop_requested()) {
        // A request cannot establish successful Preparing facts when its required work service is
        // absent. Keep the complete candidate private so terminal evidence exposes no partial
        // success.
        outcome = RunOutcome::Failed;
        failures.emplace_back(RunFailureCode::RequestedWorkUnavailable, RunPhase::Preparing,
                              "Requested work requires run services that are not yet available");
        observations.publishRetainedFailure(failures.back());
    }

    if (pendingPreparation && outcome != RunOutcome::Failed && !stop.stop_requested())
        preparation = &evidence.recordPreparation(std::move(*pendingPreparation));

    if (outcome == RunOutcome::Failed) {
        // Preparation failure stops traversal, but never bypasses the mandatory cleanup pass.
    } else if (stop.stop_requested()) {
        outcome = RunOutcome::Cancelled;
    } else if (request.hasRequestedWork() && services.work) {
        const auto synchronizeFinalPhase = [&] {
            if (const auto* current = evidence.currentPhase()) finalPhase = current->phase();
        };
        try {
            services.work->execute(*preparation, work, evidence, artifacts, observations, stop);
        } catch (const RunEvidenceInvariantViolation&) {
            // Preserve mandatory cleanup before propagating this programming defect to its owner.
            synchronizeFinalPhase();
            evidenceInvariantViolation = std::current_exception();
        } catch (const std::exception& error) {
            synchronizeFinalPhase();
            observations.recordFailure(
                RunFailure{RunFailureCode::WorkServiceFailed, finalPhase, error.what()});
        } catch (...) {
            synchronizeFinalPhase();
            observations.recordFailure(
                RunFailure{RunFailureCode::WorkServiceFailed, finalPhase,
                           "The work service threw a non-standard exception"});
        }
        synchronizeFinalPhase();
    } else {
        finalPhase = recordSkippedWorkPhases(observations, stop);
    }

    // Safety Cleanup runs exactly once on every terminal path, before the terminal result is
    // committed, so cancellation and failure cannot litter Mod Roots with run-owned artifacts.
    // Cleanup failure retention does not replace the furthest work phase or publish a Run Failure.
    const auto workFinalPhase = evidence.currentPhase() ? evidence.currentPhase()->phase()
                                                        : RunPhase::Preparing;
    observations.recordPhase(RunPhaseRecord::executed(RunPhase::SafetyCleanup));
    finalPhase = workFinalPhase;
    auto cleanupFailures = collectSafetyCleanupFailures(artifacts);
    auto injectedCleanupFailures = collectSafetyCleanupFailures(services.safetyCleanup);
    cleanupFailures.insert(cleanupFailures.end(), injectedCleanupFailures.begin(),
                           injectedCleanupFailures.end());
    for (const auto& failure : cleanupFailures) evidence.recordSafetyCleanupFailure(failure);
    if (evidenceInvariantViolation) std::rethrow_exception(evidenceInvariantViolation);
    if (stop.stop_requested() || work.cancellationObserved)
        evidence.recordCancellationObservation();
    auto terminalEvidence = std::move(evidence).consume();
    // Preserve the transitional terminal work view from the authoritative sealed sequence. Doing
    // this earlier would let its single cursor skip deferred work diagnostics while withholding an
    // ObserverFailed generated by a different publication boundary.
    work.diagnostics.assign(terminalEvidence.diagnostics().begin(),
                            terminalEvidence.diagnostics().end());
    work.publishedDiagnostics = work.diagnostics.size();
    return OptimizationRunResult::terminal(outcome, finalPhase, std::move(terminalEvidence),
                                           std::move(runId), std::move(failures),
                                           std::move(cleanupFailures), &work);
}
}  // namespace cao::run
