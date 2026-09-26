#pragma once

#include "Run/RunSetup.h"

#include <filesystem>
#include <span>
#include <string_view>
#include <utility>
#include <vector>

namespace cao::run {
/// Whether Archive ordering comes from deterministic discovery or caller-supplied precedence.
enum class ArchivePrecedenceMode { DeterministicDiscovery, ExplicitOrder };

/// Owned ordering intent; discovery later validates an explicit order against enabled Archives.
class ArchivePrecedence final {
   public:
    /// Requests the run's deterministic discovery order within each Mod Root.
    [[nodiscard]] static ArchivePrecedence deterministicDiscovery() {
        return ArchivePrecedence(ArchivePrecedenceMode::DeterministicDiscovery, {});
    }
    /// Owns high-to-low Archive paths relative to the Mod Root; an empty list remains explicit.
    [[nodiscard]] static ArchivePrecedence explicitOrder(std::vector<std::filesystem::path> paths) {
        return ArchivePrecedence(ArchivePrecedenceMode::ExplicitOrder, std::move(paths));
    }
    /// Returns the caller-selected ordering mode, including for an empty explicit order.
    [[nodiscard]] ArchivePrecedenceMode mode() const noexcept { return _mode; }
    /// Borrows supplied Archive paths in precedence order; empty for deterministic discovery.
    [[nodiscard]] std::span<const std::filesystem::path> highToLow() const noexcept {
        return _paths;
    }

   private:
    /// Owns the mutually exclusive precedence mode and its ordered paths without filesystem reads.
    ArchivePrecedence(ArchivePrecedenceMode mode, std::vector<std::filesystem::path> paths)
        : _mode(mode), _paths(std::move(paths)) {}

    ArchivePrecedenceMode _mode;
    std::vector<std::filesystem::path> _paths;
};

/// Owned configuration and profile facts loaded for one run, without application state.
class RunConfiguration final {
   public:
    /// Snapshots profile capabilities and child exclusions; Single Mod selections ignore no roots.
    /// Separator markers are case-sensitive UTF-8 substrings supplied by the provider. The
    /// application provider can preserve its legacy "separator" rule without baking that policy
    /// into the executor.
    RunConfiguration(SelectedProfileFacts profile, std::vector<std::string> ignoredMods = {},
                     std::vector<std::string> separatorMarkers = {})
        : _profile(std::move(profile)),
          _ignoredMods(std::move(ignoredMods)),
          _separatorMarkers(std::move(separatorMarkers)) {}

    /// Borrows the selected profile's facts for this snapshot's lifetime.
    [[nodiscard]] const SelectedProfileFacts& profile() const noexcept { return _profile; }
    /// Borrows UTF-8 child names matched case-insensitively during Preparing, in provider order.
    [[nodiscard]] std::span<const std::string> ignoredMods() const noexcept { return _ignoredMods; }

    /// Borrows separator markers matched against child names only; empty markers match nothing.
    [[nodiscard]] std::span<const std::string> separatorMarkers() const noexcept {
        return _separatorMarkers;
    }

   private:
    SelectedProfileFacts _profile;
    std::vector<std::string> _ignoredMods;
    std::vector<std::string> _separatorMarkers;
};

/// Loads independent values inside Preparing, without mutating Assets, Archives, or app settings.
class RunConfigurationProvider {
   public:
    virtual ~RunConfigurationProvider() = default;

    /// Loads the named profile and configuration on the execution thread. Must return owned facts,
    /// never borrow application singletons; loading failures may throw and become run failures.
    [[nodiscard]] virtual RunConfiguration load(std::string_view profileIdentity) const = 0;
};

/// Immutable preparation facts retained by a terminal result, including after later failure.
class RunPreparation final {
   public:
    /// Owns the canonical roots, loaded configuration, and successfully compiled Routing Policy.
    RunPreparation(std::vector<std::filesystem::path> modRoots, RunConfiguration configuration,
                   routing::RoutingPolicy policy, ArchivePrecedence archivePrecedence)
        : _modRoots(std::move(modRoots)),
          _configuration(std::move(configuration)),
          _policy(std::move(policy)),
          _archivePrecedence(std::move(archivePrecedence)) {}

    /// Borrows the resolved Mod Roots in run order for this value's lifetime.
    [[nodiscard]] std::span<const std::filesystem::path> modRoots() const noexcept {
        return _modRoots;
    }
    /// Borrows the owned configuration snapshot for this value's lifetime.
    [[nodiscard]] const RunConfiguration& configuration() const noexcept { return _configuration; }
    /// Borrows the compiled policy for this value's lifetime.
    [[nodiscard]] const routing::RoutingPolicy& policy() const noexcept { return _policy; }
    /// Borrows ordering intent; completeness cannot be checked until Archive discovery.
    [[nodiscard]] const ArchivePrecedence& archivePrecedence() const noexcept {
        return _archivePrecedence;
    }

   private:
    std::vector<std::filesystem::path> _modRoots;
    RunConfiguration _configuration;
    routing::RoutingPolicy _policy;
    ArchivePrecedence _archivePrecedence;
};
}  // namespace cao::run
