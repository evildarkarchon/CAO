#pragma once

#include <array>
#include <filesystem>
#include <initializer_list>
#include <optional>
#include <span>
#include <string>
#include <variant>
#include <vector>

namespace cao::routing
{
enum class AssetKind
{
    Texture,
    Mesh,
    Animation,
    Archive
};

enum class TextureVariant
{
    Native,
    Convertible
};

enum class MeshVariant
{
    Standard,
    Terrain
};

enum class AssetOperation
{
    Extraction,
    Optimization,
    Conversion,
    MeshReferenceMaintenance
};

enum class ExecutionMode
{
    Apply,
    DryRun
};

/// Closed work choices that an input adapter can place in a run request.
enum class RequestedWork
{
    NativeTextureOptimization,
    ConvertibleTextureConversion,
    StandardMeshOptimization,
    TerrainMeshOptimization,
    AnimationOptimization,
    ArchiveExtraction
};

/// Closed game-profile capabilities understood by Routing Policy compilation.
enum class ProfileCapability
{
    NativeTextureOptimization,
    ConvertibleTextureConversion,
    StandardMeshOptimization,
    TerrainMeshOptimization,
    AnimationOptimization,
    ArchiveExtraction,
    MeshReferenceMaintenance
};

/// Dedicated run-request facts used to compile one run-scoped Routing Policy.
class RunRequest final
{
public:
    /// Owns the requested work as a closed set for the selected execution mode.
    [[nodiscard]] static RunRequest forWork(ExecutionMode mode,
                                            std::initializer_list<RequestedWork> work) noexcept;

    /// Produces the tracer's dedicated request for native Texture optimization.
    [[nodiscard]] static RunRequest optimizeNativeTextures() noexcept;

private:
    friend class RoutingPolicy;

    RunRequest() = default;

    ExecutionMode _executionMode{ExecutionMode::Apply};
    std::array<bool, 6> _work{};
};

/// Dedicated Profile Capability facts used to validate a run request.
class ProfileCapabilities final
{
public:
    /// Owns one raw Archive extension definition and a closed set of supported work.
    [[nodiscard]] static ProfileCapabilities define(
        std::string archiveExtension,
        std::initializer_list<ProfileCapability> capabilities);

    /// Omits the Archive extension while retaining supplied capabilities so compilation can report it as invalid.
    [[nodiscard]] static ProfileCapabilities withoutArchiveExtension(
        std::initializer_list<ProfileCapability> capabilities = {}) noexcept;

private:
    friend class RoutingPolicy;

    ProfileCapabilities() = default;

    std::optional<std::string> _archiveExtension;
    std::array<bool, 7> _capabilities{};
};

enum class MalformedArchiveExtensionReason
{
    MissingLeadingPeriod,
    EmptySuffix,
    InvalidCharacter
};

struct MissingArchiveExtension final
{};

struct MalformedArchiveExtension final
{
    std::string extension;
    MalformedArchiveExtensionReason reason;
};

struct AmbiguousArchiveExtension final
{
    std::string extension;
    std::string conflictingExtension;
    AssetKind conflictingKind;
};

struct UnsupportedRequestedAssetKind final
{
    RequestedWork request;
    AssetKind kind;
};

using AssetVariant = std::variant<TextureVariant, MeshVariant>;

struct UnsupportedRequestedAssetVariant final
{
    RequestedWork request;
    AssetVariant variant;
};

struct UnsupportedDerivedOperation final
{
    RequestedWork cause;
    AssetOperation operation;
};

using PolicyValidationError = std::variant<MissingArchiveExtension,
                                           MalformedArchiveExtension,
                                           AmbiguousArchiveExtension,
                                           UnsupportedRequestedAssetKind,
                                           UnsupportedRequestedAssetVariant,
                                           UnsupportedDerivedOperation>;
using PolicyValidationErrors = std::vector<PolicyValidationError>;

class RoutingPolicyBuildResult;

/// Immutable run-scoped facts used to make Routing Decisions.
class RoutingPolicy final
{
public:
    /// Compiles dedicated request and Profile Capability values into either one policy or every validation error.
    [[nodiscard]] static RoutingPolicyBuildResult compile(RunRequest request,
                                                          ProfileCapabilities capabilities);

    RoutingPolicy(const RoutingPolicy &) = default;
    RoutingPolicy(RoutingPolicy &&) noexcept = default;
    RoutingPolicy &operator=(const RoutingPolicy &) = delete;
    RoutingPolicy &operator=(RoutingPolicy &&) = delete;

    /// Returns the apply-or-Dry-Run mode fixed for the duration of the run.
    [[nodiscard]] ExecutionMode executionMode() const noexcept;

    /// Reports whether the compiled policy contains one explicit work choice.
    [[nodiscard]] bool requests(RequestedWork work) const noexcept;

    /// Reports whether convertible Texture work derived Mesh Reference Maintenance.
    [[nodiscard]] bool maintainsMeshReferences() const noexcept;

    /// Returns the validated, ASCII-lowercase profile Archive extension.
    [[nodiscard]] const std::string &archiveExtension() const noexcept;

private:
    friend class AssetRouter;

    /// Owns already-validated compiled facts, including the normalized Archive extension, for one immutable run.
    RoutingPolicy(ExecutionMode executionMode,
                  std::array<bool, 6> work,
                  bool meshReferenceMaintenance,
                  std::string archiveExtension);

    const ExecutionMode _executionMode;
    const std::array<bool, 6> _work;
    const bool _meshReferenceMaintenance;
    const std::string _archiveExtension;
};

/// A policy-build outcome containing either one usable policy or every structured validation error.
class RoutingPolicyBuildResult final
{
public:
    /// Reports whether compilation produced a usable Routing Policy.
    [[nodiscard]] bool hasPolicy() const noexcept;

    /// Returns the compiled policy, or nullptr when validation failed.
    [[nodiscard]] const RoutingPolicy *policy() const noexcept;

    /// Returns all validation errors, or an empty span when compilation succeeded.
    [[nodiscard]] std::span<const PolicyValidationError> errors() const noexcept;

private:
    friend class RoutingPolicy;

    explicit RoutingPolicyBuildResult(RoutingPolicy policy);
    explicit RoutingPolicyBuildResult(PolicyValidationErrors errors);

    std::variant<RoutingPolicy, PolicyValidationErrors> _outcome;
};

/// Kind-specific identity for a recognized Texture Asset.
class TextureAsset final
{
public:
    /// Creates a Texture identity carrying its Texture-specific Variant.
    explicit TextureAsset(TextureVariant variant) noexcept;

    /// Returns the Texture-specific Asset Variant.
    [[nodiscard]] TextureVariant variant() const noexcept;

private:
    TextureVariant _variant;
};

/// A recognized Asset selected to participate in the optimization run.
class RoutedAsset final
{
public:
    /// Owns the caller's execution path without normalizing it and carries the recognized Texture identity.
    RoutedAsset(std::filesystem::path executionPath, TextureAsset texture);

    /// Returns the caller-provided execution path exactly as supplied to the router.
    [[nodiscard]] const std::filesystem::path &executionPath() const noexcept;

    /// Returns the kind-specific Texture identity selected by routing.
    [[nodiscard]] const TextureAsset &texture() const noexcept;

private:
    std::filesystem::path _executionPath;
    TextureAsset _texture;
};

/// A Routing Decision for a path whose terminal extension is not supported by this tracer.
struct UnsupportedDecision final
{};

using RoutingDecision = std::variant<RoutedAsset, UnsupportedDecision>;

/// Makes deterministic, filename-only Routing Decisions from one immutable policy.
class AssetRouter final
{
public:
    /// Owns the immutable Routing Policy used for every decision made by this router.
    explicit AssetRouter(RoutingPolicy policy) noexcept;

    /// Routes one execution path without filesystem access or path normalization.
    [[nodiscard]] RoutingDecision route(const std::filesystem::path &executionPath) const;

private:
    // The tracer policy is a validity proof today; ownership keeps the router run-scoped as later facts are added.
    [[maybe_unused]] RoutingPolicy _policy;
};
}
