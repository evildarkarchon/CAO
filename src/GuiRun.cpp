#include "GuiRun.h"
#include "Run/RunWorkRecord.h"
#include <sstream>

namespace cao::gui {
namespace {
/// Uses canonical lifecycle vocabulary rather than guessing work from optimizer log messages.
const char* phaseName(run::RunPhase phase) noexcept {
    switch (phase) {
        case run::RunPhase::Preparing:
            return "Preparing";
        case run::RunPhase::DiscoveringArchives:
            return "Discovering Archives";
        case run::RunPhase::ExtractingArchives:
            return "Extracting Archives";
        case run::RunPhase::BuildingEffectiveAssetTree:
            return "Building the Effective Asset Tree";
        case run::RunPhase::ProcessingAssets:
            return "Processing Assets";
        case run::RunPhase::ArchiveFinalization:
            return "Archive Finalization";
        case run::RunPhase::SafetyCleanup:
            return "Safety Cleanup";
    }
    return "Unknown Phase";
}
/// Names durable effects independently of which phase happened to terminate the run.
const char* mutationName(run::MutationKind kind) noexcept {
    switch (kind) {
        case run::MutationKind::ArchiveExtraction:
            return "Archive Extraction";
        case run::MutationKind::AssetProcessing:
            return "Asset Processing";
        case run::MutationKind::ArchiveFinalization:
            return "Archive Finalization";
    }
    return "Unknown Mutation";
}
/// Retains owned result evidence, including changes cancellation cannot roll back.
void appendResultDetails(std::vector<std::string>& details,
                         const run::OptimizationRunResult& result) {
    details.push_back(std::string("Cancellation Observed: ") +
                      (result.cancellationObserved() ? "yes" : "no"));
    details.push_back(std::string("Final Phase: ") + phaseName(result.finalPhase()));
    for (const auto& root : result.modRoots())
        details.push_back("Mod Root: " + root.generic_string());
    for (const auto& failure : result.failures())
        details.push_back("Failure: " + failure.detail() + " | " + failure.path().generic_string());
    for (const auto& failure : result.cleanupFailures())
        details.push_back("Cleanup Failure: " + failure.detail() + " | " +
                          failure.path().generic_string());
    for (const auto& attempt : result.work().assetAttempts) {
        if (!attempt.result.succeeded())
            details.push_back("Asset Failure: " + attempt.asset.executionPath().generic_string() +
                              " | " + attempt.result.operation() + " | " +
                              attempt.result.message() + " | " +
                              attempt.result.affectedPath().generic_string() + " | " +
                              attempt.result.serviceDetail());
    }
    for (const auto& attempt : result.work().archiveAttempts)
        if (!attempt.succeeded())
            details.push_back("Archive Failure: " + attempt.archivePath.generic_string() + " | " +
                              attempt.detail);
    for (const auto& finalization : result.work().finalizations) {
        if (finalization.failure) details.push_back("Finalization Failure: " + finalization.detail);
        for (const auto& attempt : finalization.attempts)
            if (!attempt.succeeded())
                details.push_back("Archive Failure: " + attempt.archivePath.generic_string() +
                                  " | " + attempt.detail);
    }
    for (const auto& mutation : result.mutationSummaries()) {
        std::ostringstream text;
        text << "Committed Mutations Retained: " << mutation.modRoot.generic_string() << " | "
             << mutationName(mutation.kind) << " | committed=" << mutation.committed
             << " | partial-or-unknown=" << mutation.partialOrUnknown;
        details.push_back(text.str());
    }
    for (const auto& collision : result.work().collisions) {
        std::string text = "Archive Collision: " + collision.modRoot().generic_string() + " | " +
                           collision.gamePath().generic_string() +
                           " | winner=" + collision.winningArchive().generic_string() +
                           " | loose-asset-wins=" + (collision.looseAssetWins() ? "yes" : "no");
        for (const auto& shadowed : collision.shadowedArchives())
            text += " | shadowed=" + shadowed.generic_string();
        details.push_back(std::move(text));
    }
    for (const auto& [reason, name] :
         {std::pair{routing::SkipReason::DisabledPhase, "Disabled Phase"},
          {routing::SkipReason::DisabledAssetKind, "Disabled Asset Kind"},
          {routing::SkipReason::ExcludedAssetVariant, "Excluded Asset Variant"}})
        if (const auto count = result.skippedAssetCount(reason); count != 0)
            details.push_back(std::string("Skipped Assets: ") + name + " | " +
                              std::to_string(count));
}
}  // namespace
void RunViewModel::begin(std::string runId) {
    _state = {};
    _state.runId = std::move(runId);
    _state.label = "Preparing";
    _state.active = true;
    _sequence = 0;
}
bool RunViewModel::consume(const run::RunEvent& event) {
    // Queued observations can outlive a handle and must never rewrite a subsequent run's view.
    if (event.runId() != _state.runId || event.sequence() <= _sequence) return false;
    _sequence = event.sequence();
    if (const auto* phase = std::get_if<run::RunPhaseRecord>(&event.payload())) {
        _state.label = phaseName(phase->phase());
        _state.progress = phase->progress();
        if (phase->status() == run::RunPhaseStatus::Skipped)
            _state.label += phase->skipReason() == run::PhaseSkipReason::DryRun
                                ? " - Skipped (Dry Run)"
                                : " - Skipped (No Requested Work)";
        if (_state.cancellationRequested) _state.label = "Cancelling - " + _state.label;
    } else if (const auto* diagnostic = std::get_if<run::RunDiagnostic>(&event.payload())) {
        _state.details.push_back("Diagnostic: " + diagnostic->detail() + " | " +
                                 diagnostic->path().generic_string());
    } else if (const auto* failure = std::get_if<run::RunFailure>(&event.payload())) {
        _state.details.push_back("Failure: " + failure->detail() + " | " +
                                 failure->path().generic_string());
    } else if (const auto* result = std::get_if<std::shared_ptr<const run::OptimizationRunResult>>(
                   &event.payload())) {
        _state.active = false;
        _state.outcome = (*result)->outcome();
        // Cleanup has no attempt counters; recover only the authoritative final work phase record.
        if (const auto* finalPhase = (*result)->phase((*result)->finalPhase()))
            _state.progress = finalPhase->progress();
        appendResultDetails(_state.details, **result);
        switch (*_state.outcome) {
            case run::RunOutcome::Succeeded:
                _state.label = "Done";
                break;
            case run::RunOutcome::CompletedWithFailures:
                _state.label = "Completed With Failures";
                break;
            case run::RunOutcome::Cancelled:
                _state.label = "Cancelled";
                break;
            case run::RunOutcome::Failed:
                _state.label = "Failed";
                break;
        }
    }
    return true;
}
void RunViewModel::requestCancellation() {
    if (_state.active && !_state.cancellationRequested) {
        _state.cancellationRequested = true;
        _state.label = "Cancelling - " + _state.label;
    }
}
bool RunViewModel::requestClose() {
    if (!_state.active) return true;
    // The owning window must remain alive through Safety Cleanup, not merely cancellation intent.
    _state.closeRequested = true;
    requestCancellation();
    return false;
}
}  // namespace cao::gui
