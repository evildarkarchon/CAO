#pragma once

#include "OptimizerProfileSnapshot.h"
#include "Run/ArchiveCapacity.h"

#include <stop_token>
#include <string>
#include <vector>

namespace cao::run {
class RunPreparation;
class RunWorkEvidence;
class TemporaryArtifactRegistry;

/// Archive Finalization choices captured from the run's option snapshot before scheduling.
/// Whether packing runs at all is the Routing Policy's Archive creation request, not a setting.
struct ArchiveFinalizationSettings final {
    bool compress{};
    bool deleteSources{};
    bool createDummyPlugins{};
    bool mergeIncompressible{};
    bool mergeTextures{};
};

/// The Apply-only Archive Finalization Run Phase for one Optimization Run.
///
/// When the Routing Policy requests Archive creation, it freezes an output plan for every Mod
/// Root, checks staging capacity, publishes each planned Archive and any required Loading Plugin
/// through no-replace staging, cleans packed sources, and maintains Loading Plugins for existing
/// Archives. Empty-directory pruning runs whether or not Archive creation is requested. The phase
/// records its own output total, each completed attempt before the next output starts, and one
/// final result into Run Evidence.
class ArchiveFinalization final {
   public:
    /// Owns the run's profile snapshot and finalization choices. The capacity and volume probes
    /// are the phase's only filesystem seams besides the Mod Roots themselves; the defaults
    /// sample the real volumes. Reads no global Profiles state.
    ArchiveFinalization(OptimizerProfileSnapshot profile, ArchiveFinalizationSettings settings,
                        CapacityProbe capacity = availableArchiveCapacity,
                        VolumeIdentityProbe volumeIdentity = archiveVolumeIdentity);

    /// Runs the phase over the preparation's Mod Roots, borrowing every argument until return.
    ///
    /// Evidence must already be in the executed Archive Finalization phase. Cancellation is
    /// observed between outputs and between Mod Roots, never inside an atomic output attempt;
    /// cancelled planning records a cancelled result without an output total. Exceptions from
    /// the phase's own work are recorded once as a phase-level UnexpectedException that keeps
    /// every attempt already recorded and forbids continuation. Exceptions raised by Run
    /// Evidence itself, such as RunEvidenceInvariantViolation, propagate unchanged so the Run
    /// Executor still performs Safety Cleanup. The caller owns artifacts through Safety Cleanup.
    void run(const RunPreparation& preparation, RunWorkEvidence& evidence,
             TemporaryArtifactRegistry& artifacts, std::stop_token stop) const;

   private:
    OptimizerProfileSnapshot _profile;
    ArchiveFinalizationSettings _settings;
    CapacityProbe _capacity;
    VolumeIdentityProbe _volumeIdentity;
    /// Native-separator substrings from the profile's FilesToNotPack list; a matching path
    /// is never packed or deleted as a packed source.
    std::vector<std::u8string> _filesToNotPack;
};
}  // namespace cao::run
