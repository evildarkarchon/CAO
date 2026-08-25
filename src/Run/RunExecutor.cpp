#include "RunExecutor.h"

#include <utility>
#include <vector>

namespace cao::run {
namespace {
/// Records the work phases a request with no requested work skips, in canonical order.
///
/// Every phase reports the one reason the run actually knows: nothing was requested. A skipped
/// phase must not report the outcome of a phase that never ran, so a run that skipped discovery
/// cannot claim that no Archives were discovered, and one that skipped routing cannot claim there
/// were no Routed Assets. Execution mode is deliberately not consulted either: a Dry Run that was
/// asked for nothing is excluded by the empty request, not by its mode.
void recordSkippedWorkPhases(std::vector<RunPhaseRecord>& phases) {
    for (const auto phase : {RunPhase::DiscoveringArchives, RunPhase::ExtractingArchives,
                             RunPhase::BuildingEffectiveAssetTree, RunPhase::ProcessingAssets,
                             RunPhase::ArchiveFinalization})
        phases.push_back(RunPhaseRecord::skipped(phase, PhaseSkipReason::NoRequestedWork));
}
}  // namespace

OptimizationRunResult RunExecutor::execute(const RunRequest& request,
                                           const RunServices& services) const {
    std::vector<RunPhaseRecord> phases;
    phases.reserve(runPhaseSequence().size());

    // Preparing always executes: it is where the request becomes run-scoped state. It is
    // indeterminate work, so it reports no progress rather than a total of one.
    phases.push_back(RunPhaseRecord::executed(RunPhase::Preparing));
    auto finalPhase = RunPhase::Preparing;
    auto outcome = RunOutcome::Succeeded;

    if (request.hasRequestedWork()) {
        // Requested work needs service seams this slice does not yet own. Traversing the work
        // phases here would report a Succeeded run that touched nothing, so Preparing fails and
        // the run still reaches Safety Cleanup. Later lifecycle slices replace this branch with
        // real discovery, Asset processing, and Archive Finalization.
        outcome = RunOutcome::Failed;
    } else {
        recordSkippedWorkPhases(phases);
        finalPhase = RunPhase::ArchiveFinalization;
    }

    // Safety Cleanup runs exactly once on every terminal path, before the terminal result is
    // committed, so cancellation and failure cannot litter Mod Roots with run-owned artifacts.
    services.safetyCleanup.performSafetyCleanup();
    phases.push_back(RunPhaseRecord::executed(RunPhase::SafetyCleanup));

    return OptimizationRunResult::terminal(outcome, finalPhase, std::move(phases));
}
}  // namespace cao::run
