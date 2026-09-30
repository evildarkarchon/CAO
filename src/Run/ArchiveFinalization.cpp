#include "Run/ArchiveFinalization.h"

#include "FilesystemOperations.h"
#include "Run/ArchiveFinalizationInternals.h"
#include "Run/ArchiveFinalizationResult.h"
#include "Run/RunEvidence.h"
#include "Run/RunPreparation.h"
#include "Run/TemporaryArtifactRegistry.h"

#include <algorithm>
#include <exception>
#include <map>
#include <optional>
#include <span>

namespace cao::run {
namespace archive_finalization {
std::optional<std::string> capacityShortfall(const CapacityProbe& capacity,
                                             const std::filesystem::path& root,
                                             const std::uintmax_t required) {
    std::optional<std::uintmax_t> available;
    try {
        if (capacity) available = capacity(root);
    } catch (...) {
        // A failed capacity query is unknown; the atomic writer still handles real I/O errors.
    }
    if (!available || required <= *available) return std::nullopt;
    return archiveCapacityDetail(required, *available);
}
}  // namespace archive_finalization

// This TU owns the phase's interface; its private parts are visible here and nowhere public.
using namespace archive_finalization;

namespace {

/// Carries an exception raised by Run Evidence itself past the phase's work-failure handlers.
/// Deliberately not a std::exception, so no work handler can mistake it for a work failure.
struct EvidenceRecordingFailed final {
    std::exception_ptr error;
};

/// Submits the phase's evidence protocol (output total, attempts, one result) and keeps every
/// exception from Run Evidence distinct from the phase's own work failures.
class EvidenceRecorder final {
   public:
    /// Borrows the work evidence view for one synchronous phase run.
    explicit EvidenceRecorder(RunWorkEvidence& evidence) noexcept : _evidence(evidence) {}

    /// Starts determinate progress with the frozen output total.
    void plan(const std::size_t total) {
        guarded([&] { _evidence.recordArchiveFinalizationPlan(total); });
        _total = total;
    }

    /// Retains one completed output attempt before the next output starts.
    void attempt(const ArchiveFinalizationAttempt& completed) {
        guarded([&] { _evidence.recordArchiveFinalizationAttempt(completed, _total); });
    }

    /// Retains the phase's single final result, whose attempts match those already recorded.
    void result(ArchiveFinalizationResult finalization) {
        guarded([&] { _evidence.recordArchiveFinalization(std::move(finalization)); });
    }

   private:
    /// Runs one evidence call, wrapping anything it throws for unchanged rethrow by run().
    template <typename Record>
    static void guarded(Record&& record) {
        try {
            record();
        } catch (...) {
            throw EvidenceRecordingFailed{std::current_exception()};
        }
    }

    RunWorkEvidence& _evidence;
    std::size_t _total{};
};

/// Builds archive settings from the run-owned profile snapshot.
btu::bsa::Settings archiveSettings(const OptimizerProfileSnapshot& profile) {
    auto sets = btu::bsa::Settings::get(profile.bsaGame);
    if (profile.maxBsaUncompressedSize > sets.max_size)
        sets.max_size = profile.maxBsaUncompressedSize;
    return sets;
}

/// Prunes empty children of each Mod Root, recording one mutation per root that lost any.
/// Preserves the roots themselves and reserved staging, which belongs to Safety Cleanup.
void pruneEmptyDirectories(const std::span<const std::filesystem::path> roots,
                           const std::stop_token stop, ArchiveFinalizationResult& result) {
    for (const auto& root : roots) {
        if (stop.stop_requested()) {
            result.cancelled = true;
            break;
        }
        const auto pruned =
            FilesystemOperations::deleteEmptyDirectories(QString::fromStdWString(root.wstring()));
        if (pruned != 0)
            result.mutations.push_back(
                {.modRoot = root,
                 .path = root,
                 .kind = ArchiveFinalizationMutationKind::EmptyDirectoryPruning,
                 .mutation = execution::MutationState::Committed,
                 .count = pruned});
    }
}

/// Publishes each planned Archive and missing loading plugin through one-use no-replace
/// staging, then cleans its sources without mid-attempt cancellation. A release or later
/// cleanup failure retains the committed Archive mutation in the completed attempt.
/// After all output attempts, maintains Loading Plugins for existing Archives even when the
/// output total is zero. Each new plugin is a separate mutation fact, never an output attempt.
/// Prunes empty children per Mod Root only after all outputs finish without cancellation or
/// unsafe failure. Recoverable source-cleanup failures retain readable evidence and continue.
/// Known capacity shortages stop before mutation; unknown capacity proceeds with atomic
/// attempts. Each completed attempt, including a capacity rejection, is recorded before the
/// next output starts. volumeIdentity groups roots for batch capacity checks; unknown identity
/// retains a conservative whole-batch estimate. A known no-mutation plugin creation or removal
/// failure is a safe phase failure; a plugin action with uncertain effects stops finalization
/// as unsafe. Accumulates into the caller's result so an escaping exception keeps every
/// attempt and mutation fact already established.
void finalizePlan(const ArchiveFinalizationPlan& plan, TemporaryArtifactRegistry& artifacts,
                  const std::stop_token stop, const CapacityProbe& capacity,
                  const VolumeIdentityProbe& volumeIdentity, EvidenceRecorder& recorder,
                  ArchiveFinalizationResult& result) {
    namespace fs = std::filesystem;
    // Records a known shortfall as this output's attempt, or as a phase-level failure when the
    // Mod Root has no planned output.
    const auto hasCapacity = [&](const fs::path& root, const fs::path& archive,
                                 std::uintmax_t required) {
        auto shortfall = capacityShortfall(capacity, root, required);
        if (!shortfall) return true;
        if (archive.empty()) {
            result.failure = ArchiveFinalizationFailure::InsufficientCapacity;
            result.detail = std::move(*shortfall);
            return false;
        }
        ArchiveFinalizationAttempt attempt{archive};
        attempt.modRoot = root;
        attempt.failure = ArchiveFinalizationFailure::InsufficientCapacity;
        attempt.detail = std::move(*shortfall);
        result.attempts.push_back(std::move(attempt));
        recorder.attempt(result.attempts.back());
        return false;
    };
    if (stop.stop_requested()) {
        result.cancelled = true;
        return;
    }
    // Freeze each canonical root's volume once for all preflight and later rechecks. Rebuilding
    // the remaining dummy reserve must not turn Several Mods into repeated native volume queries.
    std::map<fs::path, std::optional<std::string>> volumeByRoot;
    for (const auto& root : plan.roots)
        volumeByRoot.emplace(root, volumeIdentity ? volumeIdentity(root) : std::nullopt);
    const auto cachedVolumeIdentity = [&](const fs::path& root) { return volumeByRoot.at(root); };
    ArchiveVolumeCapacityRequirements phaseCapacity(cachedVolumeIdentity);
    ArchiveVolumeCapacityRequirements dummyCapacity(cachedVolumeIdentity);
    for (const auto& root : plan.roots) {
        const auto required = plan.dummyCapacityByRoot.at(root);
        phaseCapacity.add(root, required);
        dummyCapacity.add(root, required);
    }
    for (const auto& output : plan.outputs)
        phaseCapacity.add(output.modRoot, output.estimatedCapacityBytes);
    // A volume must fit its complete phase before any root mutates; planned source deletion
    // cannot be treated as available space before it actually happens.
    for (const auto& root : plan.roots) {
        const auto output = std::find_if(plan.outputs.begin(), plan.outputs.end(),
                                         [&](const auto& value) { return value.modRoot == root; });
        // A Mod Root with no planned writes cannot run out of staging space during this phase.
        if (output == plan.outputs.end() && plan.dummyCapacityByRoot.at(root) == 0) continue;
        if (!hasCapacity(root, output == plan.outputs.end() ? fs::path{} : output->archivePath,
                         phaseCapacity.requiredAt(root)))
            return;
    }
    for (std::size_t index = 0; index < plan.outputs.size(); ++index) {
        if (stop.stop_requested()) {
            result.cancelled = true;
            break;
        }
        const auto& output = plan.outputs[index];
        auto attempt = attemptOutput(plan, index, artifacts, capacity,
                                     dummyCapacity.requiredAt(output.modRoot), result.mutations);
        // Only the attempt's own capacity recheck reports InsufficientCapacity. That rejection
        // happens before any mutation and ends the phase without plugin maintenance or pruning.
        const auto capacityRejected =
            attempt.failure == ArchiveFinalizationFailure::InsufficientCapacity;
        if (!capacityRejected) result.safeToContinue = attempt.safeToContinue;
        result.attempts.push_back(std::move(attempt));
        // Evidence owns each atomic result before the next output can start or throw.
        recorder.attempt(result.attempts.back());
        if (capacityRejected) return;
        if (!result.safeToContinue) break;
    }
    result.cancelled = result.cancelled || stop.stop_requested();
    if (!result.cancelled && result.safeToContinue) {
        try {
            for (auto rootIt = plan.roots.begin(); rootIt != plan.roots.end(); ++rootIt) {
                if (stop.stop_requested()) {
                    result.cancelled = true;
                    break;
                }
                const auto& root = *rootIt;
                if (plan.dummyCapacityByRoot.at(root) != 0) {
                    ArchiveVolumeCapacityRequirements remainingDummy(cachedVolumeIdentity);
                    for (auto remaining = rootIt; remaining != plan.roots.end(); ++remaining)
                        remainingDummy.add(*remaining, plan.dummyCapacityByRoot.at(*remaining));
                    // Earlier roots have already consumed their plugin allowance, so rechecking
                    // it would reject later roots despite a sufficient initial phase budget.
                    if (!hasCapacity(root, {}, remainingDummy.requiredAt(root))) return;
                }
                // Capacity and directory probes may block while a cancellation arrives; do not
                // start the next root's plugin mutation after either probe completes.
                if (stop.stop_requested()) {
                    result.cancelled = true;
                    break;
                }
                if (!maintainExistingLoadingPlugins(plan, root, artifacts, stop, result)) return;
            }
            // All planned outputs and plugin work must finish before pruning any Mod Root.
            // Recoverable attempts retain their source evidence; cancellation retains all paths.
            if (!result.cancelled) pruneEmptyDirectories(plan.roots, stop, result);
        } catch (const EvidenceRecordingFailed&) {
            // Keep Run Evidence failures distinct from this cleanup pass's own exceptions.
            throw;
        } catch (const std::exception& error) {
            result.safeToContinue = false;
            result.failure = ArchiveFinalizationFailure::UnexpectedException;
            result.detail = error.what();
        } catch (...) {
            result.safeToContinue = false;
            result.failure = ArchiveFinalizationFailure::UnexpectedException;
            result.detail = "Unexpected Archive finalization cleanup exception.";
        }
    }
}
}  // namespace

ArchiveFinalization::ArchiveFinalization(OptimizerProfileSnapshot profile,
                                         const ArchiveFinalizationSettings settings,
                                         CapacityProbe capacity, VolumeIdentityProbe volumeIdentity)
    : _profile(std::move(profile)),
      _settings(settings),
      _capacity(std::move(capacity)),
      _volumeIdentity(std::move(volumeIdentity)) {
    // The profile snapshot already loaded FilesToNotPack.txt during Preparing, so this only
    // converts its lines to the separator form that packing compares against.
    auto lines = _profile.filesToNotPack;
    for (auto& line : lines) line = QDir::toNativeSeparators(line);

    for (auto&& line : lines)
        _filesToNotPack.emplace_back(btu::common::as_utf8_string(std::move(line).toStdString()));
}

void ArchiveFinalization::run(const RunPreparation& preparation, RunWorkEvidence& evidence,
                              TemporaryArtifactRegistry& artifacts,
                              const std::stop_token stop) const {
    EvidenceRecorder recorder(evidence);
    ArchiveFinalizationResult result;
    try {
        if (!preparation.policy().requests(routing::RequestedWork::ArchiveCreation)) {
            // Empty-directory pruning is the legacy Apply finalization even without packing.
            recorder.plan(0);
            pruneEmptyDirectories(preparation.modRoots(), stop, result);
        } else {
            // The list only filters packing, so its absence matters only when packing runs.
            if (_filesToNotPack.empty()) {
                PLOG_ERROR << "FilesToNotPack.txt not found. This can cause a number of issues. "
                              "For example, for Skyrim, "
                              "animations will be packed to BSA, preventing them from being "
                              "detected by FNIS and Nemesis.";
            }
            std::optional<ArchiveFinalizationPlan> plan;
            try {
                plan.emplace(planFinalization(preparation.modRoots(), _settings,
                                              archiveSettings(_profile), _filesToNotPack, stop));
            } catch (const ArchiveFinalizationPlanningCancelled&) {
                // Planning made no mutations or trustworthy output total before cancellation.
                result.cancelled = true;
                recorder.result(std::move(result));
                return;
            }
            recorder.plan(plan->outputs.size());
            finalizePlan(*plan, artifacts, stop, _capacity, _volumeIdentity, recorder, result);
        }
        recorder.result(std::move(result));
        return;
    } catch (const EvidenceRecordingFailed& failure) {
        std::rethrow_exception(failure.error);
    } catch (const std::exception& error) {
        result.detail = error.what();
    } catch (...) {
        result.detail = "Unexpected Archive finalization exception.";
    }
    // The escaped exception gives no reliable boundary for further durable effects. Every
    // attempt in result was already recorded, so the phase-level failure keeps that prefix.
    result.failure = ArchiveFinalizationFailure::UnexpectedException;
    result.safeToContinue = false;
    // Recorded outside the guarded region so a Run Evidence failure propagates unchanged.
    evidence.recordArchiveFinalization(std::move(result));
}
}  // namespace cao::run
