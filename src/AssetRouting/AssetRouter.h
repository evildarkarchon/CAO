#pragma once

#include <array>
#include <cstddef>
#include <filesystem>
#include <functional>
#include <initializer_list>
#include <map>
#include <optional>
#include <span>
#include <string>
#include <variant>
#include <vector>

namespace cao::routing {
enum class AssetKind { Texture, Mesh, Animation, Archive };

enum class TextureVariant { Native, Convertible };

enum class MeshVariant { Standard, Terrain };

enum class AssetOperation { Extraction, Optimization, Conversion, MeshReferenceMaintenance };

enum class ExecutionMode { Apply, DryRun };

/// The coarse stage of an Optimization Run in which one Routed Asset performs its work.
///
/// This categorizes a single Asset, unlike `run::RunPhase`, which is a stage of the whole run.
enum class RoutedAssetPhase { ArchiveExtraction, LooseAssetProcessing };

enum class OptimizerTarget { Texture, Mesh, Animation, Archive };

enum class SkipReason { DisabledPhase, DisabledAssetKind, ExcludedAssetVariant };

/// Read-only closed set of work that execution must perform for one Routed Asset.
class AssetOperations final {
   public:
    /// Reports whether the Routed Asset carries one closed operation.
    [[nodiscard]] bool contains(AssetOperation operation) const noexcept;

   private:
    friend class AssetRouter;

    /// Adds one operation while the router constructs an internally consistent Routed Asset.
    void include(AssetOperation operation) noexcept;

    /// Reports whether no execution operation applies to the recognized Asset.
    [[nodiscard]] bool empty() const noexcept;

    std::array<bool, 4> _operations{};
};

/// Closed work choices that an input adapter can place in a Run Request.
enum class RequestedWork {
    NativeTextureOptimization,
    ConvertibleTextureConversion,
    StandardMeshOptimization,
    TerrainMeshOptimization,
    AnimationOptimization,
    ArchiveExtraction,
    // Archive creation is a run finalization choice, not a per-Asset Routing Decision, but it is
    // carried here so one compiled policy validates it against the selected profile.
    ArchiveCreation
};

/// Closed game-profile capabilities understood by Routing Policy compilation.
enum class ProfileCapability {
    NativeTextureOptimization,
    ConvertibleTextureConversion,
    StandardMeshOptimization,
    TerrainMeshOptimization,
    AnimationOptimization,
    ArchiveExtraction,
    MeshReferenceMaintenance,
    ArchiveCreation
};

/// The execution mode and closed requested work from which one Routing Policy is compiled.
///
/// This is not the Run Request of the project glossary: it carries no Mod Selection, Archive
/// Precedence, or profile identity, and exists only as the input to `RoutingPolicy::compile`.
class RoutingPolicyRequest final {
   public:
    /// Owns the requested work as a closed set for the selected execution mode.
    [[nodiscard]] static RoutingPolicyRequest forWork(
        ExecutionMode mode, std::initializer_list<RequestedWork> work) noexcept;

    /// Owns a dynamically adapted sequence of closed work choices for the selected execution mode.
    [[nodiscard]] static RoutingPolicyRequest forWork(
        ExecutionMode mode, const std::vector<RequestedWork>& work) noexcept;

    /// Produces the tracer's dedicated request for native Texture optimization.
    [[nodiscard]] static RoutingPolicyRequest optimizeNativeTextures() noexcept;

   private:
    friend class RoutingPolicy;

    RoutingPolicyRequest() = default;

    /// Copies a borrowed sequence of closed choices into one dedicated request value.
    [[nodiscard]] static RoutingPolicyRequest fromWork(
        ExecutionMode mode, std::span<const RequestedWork> work) noexcept;

    ExecutionMode _executionMode{ExecutionMode::Apply};
    std::array<bool, 7> _work{};
};

/// Dedicated Profile Capability facts used to validate a Routing Policy Request.
class ProfileCapabilities final {
   public:
    /// Owns one raw Archive extension definition and a closed set of supported work.
    [[nodiscard]] static ProfileCapabilities define(
        std::string archiveExtension, std::initializer_list<ProfileCapability> capabilities);

    /// Owns one raw Archive extension and a dynamically adapted sequence of closed capabilities.
    [[nodiscard]] static ProfileCapabilities define(
        std::string archiveExtension, const std::vector<ProfileCapability>& capabilities);

    /// Omits the Archive extension while retaining supplied capabilities so compilation can report
    /// it as invalid.
    [[nodiscard]] static ProfileCapabilities withoutArchiveExtension(
        std::initializer_list<ProfileCapability> capabilities = {}) noexcept;

    /// Omits the Archive extension while owning a dynamically adapted sequence of closed
    /// capabilities.
    [[nodiscard]] static ProfileCapabilities withoutArchiveExtension(
        const std::vector<ProfileCapability>& capabilities) noexcept;

   private:
    friend class RoutingPolicy;

    ProfileCapabilities() = default;

    /// Copies a borrowed capability sequence and optional Archive extension into one dedicated
    /// value.
    [[nodiscard]] static ProfileCapabilities fromDefinition(
        std::optional<std::string> archiveExtension,
        std::span<const ProfileCapability> capabilities) noexcept;

    std::optional<std::string> _archiveExtension;
    std::array<bool, 8> _capabilities{};
};

enum class MalformedArchiveExtensionReason { MissingLeadingPeriod, EmptySuffix, InvalidCharacter };

struct MissingArchiveExtension final {};

struct MalformedArchiveExtension final {
    std::string extension;
    MalformedArchiveExtensionReason reason;
};

struct AmbiguousArchiveExtension final {
    std::string extension;
    std::string conflictingExtension;
    AssetKind conflictingKind;
};

struct UnsupportedRequestedAssetKind final {
    RequestedWork request;
    AssetKind kind;
};

using AssetVariant = std::variant<TextureVariant, MeshVariant>;

struct UnsupportedRequestedAssetVariant final {
    RequestedWork request;
    AssetVariant variant;
};

struct UnsupportedDerivedOperation final {
    RequestedWork cause;
    AssetOperation operation;
};

using PolicyValidationError =
    std::variant<MissingArchiveExtension, MalformedArchiveExtension, AmbiguousArchiveExtension,
                 UnsupportedRequestedAssetKind, UnsupportedRequestedAssetVariant,
                 UnsupportedDerivedOperation>;
using PolicyValidationErrors = std::vector<PolicyValidationError>;

class RoutingPolicyBuildResult;

/// Immutable run-scoped facts used to make Routing Decisions.
class RoutingPolicy final {
   public:
    /// Compiles dedicated request and Profile Capability values into either one policy or every
    /// validation error.
    [[nodiscard]] static RoutingPolicyBuildResult compile(RoutingPolicyRequest request,
                                                          ProfileCapabilities capabilities);

    RoutingPolicy(const RoutingPolicy&) = default;
    RoutingPolicy(RoutingPolicy&&) noexcept = default;
    RoutingPolicy& operator=(const RoutingPolicy&) = delete;
    RoutingPolicy& operator=(RoutingPolicy&&) = delete;

    /// Returns the apply-or-Dry-Run mode fixed for the duration of the run.
    [[nodiscard]] ExecutionMode executionMode() const noexcept;

    /// Reports whether the compiled policy contains one explicit work choice.
    [[nodiscard]] bool requests(RequestedWork work) const noexcept;

    /// Reports whether convertible Texture work derived Mesh Reference Maintenance.
    [[nodiscard]] bool maintainsMeshReferences() const noexcept;

    /// Returns the validated, ASCII-lowercase profile Archive extension.
    [[nodiscard]] const std::string& archiveExtension() const noexcept;

   private:
    friend class AssetRouter;

    /// Owns already-validated compiled facts, including the normalized Archive extension, for one
    /// immutable run.
    RoutingPolicy(ExecutionMode executionMode, std::array<bool, 7> work,
                  bool meshReferenceMaintenance, std::string archiveExtension);

    const ExecutionMode _executionMode;
    const std::array<bool, 7> _work;
    const bool _meshReferenceMaintenance;
    const std::string _archiveExtension;
};

/// A policy-build outcome containing either one usable policy or every structured validation error.
class RoutingPolicyBuildResult final {
   public:
    /// Reports whether compilation produced a usable Routing Policy.
    [[nodiscard]] bool hasPolicy() const noexcept;

    /// Returns the compiled policy, or nullptr when validation failed.
    [[nodiscard]] const RoutingPolicy* policy() const noexcept;

    /// Returns all validation errors, or an empty span when compilation succeeded.
    [[nodiscard]] std::span<const PolicyValidationError> errors() const noexcept;

   private:
    friend class RoutingPolicy;

    explicit RoutingPolicyBuildResult(RoutingPolicy policy);
    explicit RoutingPolicyBuildResult(PolicyValidationErrors errors);

    std::variant<RoutingPolicy, PolicyValidationErrors> _outcome;
};

/// Kind-specific identity for a recognized Texture Asset.
class TextureAsset final {
   public:
    /// Creates a Texture identity carrying its Texture-specific Variant.
    explicit TextureAsset(TextureVariant variant) noexcept;

    /// Returns the Texture-specific Asset Variant.
    [[nodiscard]] TextureVariant variant() const noexcept;

   private:
    TextureVariant _variant;
};

/// Kind-specific identity for a recognized Mesh Asset.
class MeshAsset final {
   public:
    /// Creates a Mesh identity carrying its Mesh-specific Variant.
    explicit MeshAsset(MeshVariant variant) noexcept;

    /// Returns the Mesh-specific Asset Variant.
    [[nodiscard]] MeshVariant variant() const noexcept;

   private:
    MeshVariant _variant;
};

/// Kind-specific identity for a recognized Animation Asset.
struct AnimationAsset final {};

/// Kind-specific identity for a recognized Archive Asset.
struct ArchiveAsset final {};

using AssetIdentity = std::variant<TextureAsset, MeshAsset, AnimationAsset, ArchiveAsset>;

/// A recognized Asset selected to participate in the optimization run.
class RoutedAsset final {
   public:
    /// Returns the caller-provided execution path exactly as supplied to the router.
    [[nodiscard]] const std::filesystem::path& executionPath() const noexcept;

    /// Returns the Asset Kind implied by the carried kind-specific identity.
    [[nodiscard]] AssetKind kind() const noexcept;

    /// Returns the kind-specific identity selected by routing.
    [[nodiscard]] const AssetIdentity& identity() const noexcept;

    /// Returns the Routed Asset Phase selected without requiring the caller to reinterpret policy.
    [[nodiscard]] RoutedAssetPhase phase() const noexcept;

    /// Returns the optimizer target selected for execution.
    [[nodiscard]] OptimizerTarget target() const noexcept;

    /// Returns the apply-or-Dry-Run mode fixed by the Routing Policy.
    [[nodiscard]] ExecutionMode executionMode() const noexcept;

    /// Returns the complete closed operation set selected for execution.
    [[nodiscard]] const AssetOperations& operations() const noexcept;

   private:
    friend class AssetRouter;

    /// Owns all execution facts selected by one policy-aware routing decision.
    RoutedAsset(std::filesystem::path executionPath, AssetIdentity identity, RoutedAssetPhase phase,
                OptimizerTarget target, ExecutionMode executionMode, AssetOperations operations);

    std::filesystem::path _executionPath;
    AssetIdentity _identity;
    RoutedAssetPhase _phase;
    OptimizerTarget _target;
    ExecutionMode _executionMode;
    AssetOperations _operations;
};

/// A recognized Asset excluded by Routing Policy before execution.
class SkippedAsset final {
   public:
    /// Returns the caller-provided execution path exactly as supplied to the router.
    [[nodiscard]] const std::filesystem::path& executionPath() const noexcept;

    /// Returns the Asset Kind implied by the carried kind-specific identity.
    [[nodiscard]] AssetKind kind() const noexcept;

    /// Returns the kind-specific identity recognized before policy exclusion.
    [[nodiscard]] const AssetIdentity& identity() const noexcept;

    /// Returns the stable highest-precedence explanation for the exclusion.
    [[nodiscard]] SkipReason reason() const noexcept;

   private:
    friend class AssetRouter;

    /// Owns the recognized Asset facts and stable reason selected by policy-aware routing.
    SkippedAsset(std::filesystem::path executionPath, AssetIdentity identity, SkipReason reason);

    std::filesystem::path _executionPath;
    AssetIdentity _identity;
    SkipReason _reason;
};

/// A Routing Decision for a path whose terminal extension is not supported by this tracer.
struct UnsupportedDecision final {};

using RoutingDecision = std::variant<RoutedAsset, SkippedAsset, UnsupportedDecision>;
using RoutedAssetReferences = std::vector<std::reference_wrapper<const RoutedAsset>>;

/// Owned summary of batch Routing Decisions that retains Routed Assets in caller input order.
class RoutingLedger final {
   public:
    /// Returns every owned Routed Asset as a read-only span valid until this ledger is destroyed or
    /// replaced.
    [[nodiscard]] std::span<const RoutedAsset> routedAssets() const noexcept;

    /// Returns read-only references matching one Routed Asset Phase in original relative order.
    /// References remain valid until this ledger is destroyed or replaced.
    [[nodiscard]] RoutedAssetReferences routedAssets(RoutedAssetPhase phase) const;

    /// Returns read-only references matching one optimizer target in original relative order.
    /// References remain valid until this ledger is destroyed or replaced.
    [[nodiscard]] RoutedAssetReferences routedAssets(OptimizerTarget target) const;

    /// Returns how many recognized Assets were excluded for one stable Skip Reason.
    [[nodiscard]] std::size_t skippedAssetCount(SkipReason reason) const noexcept;

   private:
    friend class AssetRouter;

    /// Takes ownership of Routed Assets and the aggregated recognized exclusions from a borrowed
    /// batch input.
    RoutingLedger(std::vector<RoutedAsset> routedAssets,
                  std::map<SkipReason, std::size_t> skippedAssetCounts) noexcept;

    std::vector<RoutedAsset> _routedAssets;
    std::map<SkipReason, std::size_t> _skippedAssetCounts;
};

/// Makes deterministic, filename-only Routing Decisions from one immutable policy.
class AssetRouter final {
   public:
    /// Owns the immutable Routing Policy used for every decision made by this router.
    explicit AssetRouter(RoutingPolicy policy) noexcept;

    /// Routes one execution path without filesystem access or path normalization.
    /// Returns a tagged Routed Asset, recognized-but-skipped Asset, or unsupported decision.
    [[nodiscard]] RoutingDecision route(const std::filesystem::path& executionPath) const;

    /// Routes borrowed execution paths once and returns an owned ledger preserving routed input
    /// order and duplicates.
    [[nodiscard]] RoutingLedger route(std::span<const std::filesystem::path> executionPaths) const;

   private:
    RoutingPolicy _policy;
};
}  // namespace cao::routing
