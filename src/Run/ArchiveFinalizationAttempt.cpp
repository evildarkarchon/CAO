#include "Run/ArchiveFinalizationInternals.h"

#include "Run/NativeFilePins.h"
#include "Run/TemporaryArtifactRegistry.h"

#include <btu/bsa/archive.hpp>
#include <btu/bsa/pack.hpp>

#include <algorithm>
#include <array>
#include <fstream>

namespace cao::run::archive_finalization {
namespace {
/// Proves retained source bytes can still be read after a failed deletion, without loading them
/// all into memory. A directory or substituted link is not usable retained source material.
bool readablePackedSource(const std::filesystem::path& source) {
    if (!std::filesystem::is_regular_file(std::filesystem::symlink_status(source))) return false;
    const auto expectedSize = std::filesystem::file_size(source);
    std::ifstream input(source, std::ios::binary);
    if (!input) return false;
    std::array<char, 8192> buffer{};
    std::uintmax_t bytesRead = 0;
    do {
        input.read(buffer.data(), buffer.size());
        bytesRead += static_cast<std::uintmax_t>(input.gcount());
    } while (input);
    // Some file buffers report OS read failures as EOF (for example Windows byte locks).
    // Require every expected byte before treating the retained file as usable recovery data.
    return input.eof() && !input.bad() && bytesRead == expectedSize;
}
}  // namespace

ArchiveFinalizationAttempt attemptOutput(const ArchiveFinalizationPlan& plan,
                                         const std::size_t index,
                                         TemporaryArtifactRegistry& artifacts,
                                         const CapacityProbe& capacity,
                                         const std::uintmax_t dummyReserve,
                                         std::vector<ArchiveFinalizationMutation>& mutations) {
    namespace fs = std::filesystem;
    using execution::MutationState;
    const auto& output = plan.outputs[index];
    ArchiveFinalizationAttempt attempt{output.archivePath};
    attempt.modRoot = output.modRoot;
    auto boundary = ArchiveFinalizationFailure::WriteFailed;
    std::size_t removedSources = 0;
    try {
        // Sources can grow after planning. Re-stat before mutation, retaining the frozen
        // allowance if files shrink; capacity itself is still only a momentary sample.
        const auto currentCapacity = estimatePackedCapacity(output, plan.settings);
        if (auto shortfall = capacityShortfall(
                capacity, output.modRoot,
                saturatedCapacityAdd(std::max(output.estimatedCapacityBytes, currentCapacity),
                                     dummyReserve))) {
            attempt.failure = ArchiveFinalizationFailure::InsufficientCapacity;
            attempt.detail = std::move(*shortfall);
            return attempt;
        }
#ifdef _WIN32
        std::vector<SourceFilePin> sourcePins;
        if (plan.deleteSources) {
            sourcePins.reserve(output.sources.size());
            const auto directoryPins = SourceFilePin::sharedDirectoryPins(output.modRoot);
            for (const auto& source : output.sources)
                sourcePins.emplace_back(source, output.modRoot, directoryPins);
        }
#endif
        auto staged = artifacts.stageArchiveFileForPublication(output.modRoot);
        auto archive = plan.archives[index];
        archive.set_out_path(staged.path());
        const auto errors = btu::bsa::write(plan.compress, std::move(archive), output.modRoot);
        if (!errors.empty()) throw std::runtime_error(errors.front().second);
        boundary = ArchiveFinalizationFailure::CommitFailed;
        const auto archivePublication =
            staged.publish(output.archivePath, PublicationPolicy::NoReplace);
        // Native publication commits the Archive before durable ownership release.
        if (archivePublication.state != PublicationState::NotPublished)
            attempt.mutation = MutationState::Committed;
        if (archivePublication.state != PublicationState::PublishedAndReleased)
            throw std::runtime_error(archivePublication.errorDetail.empty()
                                         ? "Archive publication did not complete."
                                         : archivePublication.errorDetail);
        boundary = ArchiveFinalizationFailure::PluginCreationFailed;
#ifdef _WIN32
        std::optional<LoadingPluginPin> loadingPluginPin;
#endif
        if (!output.loadingPluginPaths.empty()) {
            [[maybe_unused]] const auto loadingPlugin =
                ensureOutputLoadingPlugin(plan, output, artifacts, mutations);
#ifdef _WIN32
            // Retain the selected entry after discovery or publication; acquiring the pin
            // must finish before source deletion, and any race during acquisition fails safe.
            if (plan.deleteSources) loadingPluginPin.emplace(loadingPlugin, output.modRoot);
#endif
        }
        boundary = ArchiveFinalizationFailure::SourceCleanupFailed;
        if (plan.deleteSources) {
#ifdef _WIN32
            for (auto& pin : sourcePins) {
                pin.releaseForCleanup();
                pin.removeIfUnchanged();
                ++removedSources;
            }
#else
            for (const auto& source : output.sources) {
                if (!fs::remove(source))
                    throw std::runtime_error("A packed source could not be removed.");
                ++removedSources;
            }
#endif
        }
    } catch (const std::exception& error) {
        attempt.failure = boundary;
        attempt.detail = error.what();
        // Publication or plugin errors leave sources intact. Once committed, only a usable
        // Archive and surviving sources can justify continuing after later failures. The
        // publication result remains authoritative even when continuation is unsafe.
        if (attempt.mutation == MutationState::Committed) {
            try {
                attempt.safeToContinue =
                    boundary != ArchiveFinalizationFailure::PluginCreationFailed &&
                    btu::bsa::read_archive(output.archivePath).has_value();
                for (auto remaining = removedSources;
                     attempt.safeToContinue && remaining < output.sources.size(); ++remaining)
                    attempt.safeToContinue = readablePackedSource(output.sources[remaining]);
            } catch (...) {
                // Verification failure preserves the original error and stops later attempts.
                attempt.safeToContinue = false;
            }
        }
    } catch (...) {
        attempt.failure = ArchiveFinalizationFailure::UnexpectedException;
        attempt.detail = "Unexpected Archive finalization exception.";
        attempt.safeToContinue = false;
    }
    return attempt;
}
}  // namespace cao::run::archive_finalization
