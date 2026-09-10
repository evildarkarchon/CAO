#pragma once

#include "RunExecutor.h"
#include "RunWorkRecord.h"

#include <exception>

namespace cao::run {
/// Coordinates retention and synchronous publication in the executor-owned record.
/// Borrows the record and optional sink; neither may be retained beyond their execution lifetime.
class WorkObservationRecorder final {
   public:
    /// Shares publication position across Preparing, AssetRun, and interrupted work.
    WorkObservationRecorder(RunWorkRecord& record, RunObservationSink* sink)
        : _record(record), _sink(sink) {}

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

    /// Retains and immediately publishes a Preparing or caller-supplied informational observation.
    void recordDiagnostic(const RunDiagnostic& diagnostic) {
        retainDiagnostic(diagnostic);
        publishDiagnostics();
    }

    /// Retains evidence immediately while deferring publication until its established work
    /// boundary.
    void retainDiagnostic(const RunDiagnostic& diagnostic) {
        _record.diagnostics.push_back(diagnostic);
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
    RunWorkRecord& _record;
    RunObservationSink* _sink;
};
}  // namespace cao::run
