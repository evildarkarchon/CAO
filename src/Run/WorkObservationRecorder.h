#pragma once

#include "RunExecutor.h"
#include "RunWorkRecord.h"

#include <algorithm>
#include <exception>

namespace cao::run {
/// Coordinates transitional work-record retention with synchronous Run Evidence publication.
/// Borrows the record, optional observation sink, and optional concrete evidence owner; none may be
/// retained beyond their execution lifetime.
class WorkObservationRecorder final {
   public:
    /// Shares publication position and optionally mirrors Archive and Asset facts into Run Evidence.
    WorkObservationRecorder(RunWorkRecord& record, RunObservationSink* sink,
                            MutableRunEvidence* evidence = nullptr)
        : _record(record), _sink(sink), _evidence(evidence) {}

    /// Isolates presentation exceptions after evidence is retained, without changing Run Outcome.
    template <typename Callback>
    void reportSafely(RunPhase phase, Callback&& callback) {
        try {
            callback();
        } catch (const std::exception& error) {
            // Presentation cannot suppress completed work or abandon mandatory cleanup.
            retainDiagnostic(RunDiagnostic{RunDiagnosticCode::ObserverFailed, phase, error.what()});
        } catch (...) {
            // Unknown observer exceptions have the same informational status as standard ones.
            retainDiagnostic(RunDiagnostic{RunDiagnosticCode::ObserverFailed, phase,
                                           "The observer threw a non-standard exception"});
        }
    }

    /// Retains one failure before attempting publication; there is no normal-return replay.
    void recordFailure(const RunFailure& failure) {
        _record.failures.push_back(failure);
        const auto retained = _record.failures.back();
        reportSafely(retained.phase(), [&] {
            if (_sink) _sink->publishRetainedFailure(retained);
        });
    }

    /// Reports typed Archive discovery while Run Evidence owns lifecycle translation.
    void recordArchiveDiscoveryStarted() {
        if (_evidence) {
            _evidence->recordArchiveDiscoveryStarted();
            return;
        }
        reportPhaseWithoutEvidence(RunPhaseRecord::executed(RunPhase::DiscoveringArchives));
    }

    /// Reports the immutable Archive work total while Run Evidence owns phase progress.
    void recordArchiveExtractionPlan(const std::size_t total) {
        if (_evidence) {
            _evidence->recordArchiveExtractionPlan(total);
            return;
        }
        reportPhaseWithoutEvidence(RunPhaseRecord::executed(RunPhase::ExtractingArchives,
                                                            RunProgress::determinate(total)));
    }

    /// Reports Dry Run exclusion without letting AssetRun construct executor evidence.
    void recordDryRunArchiveExtraction() {
        if (_evidence) {
            _evidence->recordDryRunArchiveExtraction();
            return;
        }
        reportPhaseWithoutEvidence(
            RunPhaseRecord::skipped(RunPhase::ExtractingArchives, PhaseSkipReason::DryRun));
    }

    /// Reports entry into definitive Effective Asset Tree discovery.
    void recordEffectiveAssetTreeStarted() {
        if (_evidence) {
            _evidence->recordEffectiveAssetTreeStarted();
            return;
        }
        reportPhaseWithoutEvidence(RunPhaseRecord::executed(RunPhase::BuildingEffectiveAssetTree));
    }

    /// Retains the complete collision plan before any presentation callback can interrupt work.
    void recordArchiveCollisions(const std::span<const ArchiveCollision> collisions) {
        if (_evidence) _evidence->recordArchiveCollisions(collisions);
        _record.collisions.assign(collisions.begin(), collisions.end());
    }

    /// Retains one complete Archive attempt before its progress or cancellation boundary.
    void recordArchiveExtractionAttempt(ArchiveExtractionResult attempt, const std::size_t total) {
        if (_evidence) _evidence->recordArchiveExtractionAttempt(attempt, total);
        _record.archiveAttempts.push_back(std::move(attempt));
        if (!_evidence && _sink) {
            const auto succeeded = static_cast<std::size_t>(
                std::count_if(_record.archiveAttempts.begin(), _record.archiveAttempts.end(),
                              [](const auto& completed) { return completed.succeeded(); }));
            reportPhaseWithoutEvidence(RunPhaseRecord::executed(
                RunPhase::ExtractingArchives,
                RunProgress::determinate(total, succeeded,
                                         _record.archiveAttempts.size() - succeeded)));
        }
    }

    /// Retains one complete returned discovery fact set in both staged migration views.
    void recordArchiveDiscovery(const ArchiveDiscoveryEvidence& discovery) {
        if (_evidence) _evidence->recordArchiveDiscovery(discovery);
        _record.skippedArchiveCounts.clear();
        for (const auto reason :
             {routing::SkipReason::DisabledPhase, routing::SkipReason::DisabledAssetKind,
              routing::SkipReason::ExcludedAssetVariant}) {
            const auto count = discovery.skippedArchiveCount(reason);
            if (count != 0) _record.skippedArchiveCounts.emplace(reason, count);
        }
        _record.unsupportedExplicitPaths.assign(discovery.unsupportedExplicitPaths().begin(),
                                                discovery.unsupportedExplicitPaths().end());
        _record.nestedArchiveCount = discovery.nestedArchiveCount();
    }

    /// Retains a complete definitive ledger before the compatibility record can expose it.
    void recordRoutingLedger(routing::RoutingLedger ledger) {
        if (_evidence) _evidence->recordRoutingLedger(ledger);
        _record.ledger.emplace(std::move(ledger));
    }

    /// Reports the routed-only Asset total through concrete executor-owned evidence, when present.
    void recordAssetProcessingPlan(const std::size_t total) {
        if (_evidence) _evidence->recordAssetProcessingPlan(total);
    }

    /// Retains one completed Asset attempt before concrete evidence publishes phase progress.
    void recordAssetAttempt(RoutedAssetAttempt attempt, const std::size_t total) {
        if (_evidence) _evidence->recordAssetAttempt(attempt, total);
        _record.assetAttempts.push_back(std::move(attempt));
    }

    /// Retains and immediately publishes a Preparing or caller-supplied informational observation.
    void recordDiagnostic(const RunDiagnostic& diagnostic) {
        retainDiagnostic(diagnostic);
        publishDiagnostics();
    }

    /// Retains evidence immediately while deferring publication until its established work
    /// boundary.
    void retainDiagnostic(const RunDiagnostic& diagnostic) {
        _record.diagnostics.push_back(diagnostic);
        if (_sink) _sink->retainDiagnostic(diagnostic);
    }

    /// Publishes retained diagnostics at the caller's established reporting boundary, once each.
    void publishDiagnostics() {
        if (!_sink) return;
        const auto end = _record.diagnostics.size();
        while (_record.publishedDiagnostics < end) {
            // Advance before calling out, and copy before ObserverFailed can reallocate storage.
            const auto diagnostic = _record.diagnostics[_record.publishedDiagnostics++];
            reportSafely(diagnostic.phase(), [&] { _sink->publishRetainedDiagnostic(diagnostic); });
        }
        // Errors raised by this publication remain terminal evidence; feeding them back to the
        // same failing observer would recurse or replay them at the next reporting boundary.
        _record.publishedDiagnostics = _record.diagnostics.size();
    }

   private:
    /// Preserves legacy direct-AssetRun phase observation while migration has no evidence owner.
    void reportPhaseWithoutEvidence(const RunPhaseRecord& phase) {
        reportSafely(phase.phase(), [&] {
            if (_sink) _sink->recordPhase(phase);
        });
    }

    RunWorkRecord& _record;
    RunObservationSink* _sink;
    MutableRunEvidence* _evidence;
};
}  // namespace cao::run
