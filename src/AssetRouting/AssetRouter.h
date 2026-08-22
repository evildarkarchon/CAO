#pragma once

#include <filesystem>
#include <variant>

namespace cao::routing
{
/// Dedicated run request facts needed by the native Texture routing tracer.
class RunRequest final
{
public:
    /// Produces the tracer's dedicated request for native Texture optimization.
    [[nodiscard]] static RunRequest optimizeNativeTextures() noexcept;

private:
    RunRequest() = default;
};

/// Dedicated Profile Capability facts needed by the native Texture routing tracer.
class ProfileCapabilities final
{
public:
    /// Produces the tracer's dedicated capability value for native Texture optimization.
    [[nodiscard]] static ProfileCapabilities withNativeTextures() noexcept;

private:
    ProfileCapabilities() = default;
};

/// Immutable run-scoped facts used by the native Texture routing tracer.
class RoutingPolicy final
{
public:
    /// Compiles dedicated request and Profile Capability values into a policy for a valid tracer scenario.
    [[nodiscard]] static RoutingPolicy compile(RunRequest request, ProfileCapabilities capabilities) noexcept;

    RoutingPolicy(const RoutingPolicy &) = default;
    RoutingPolicy(RoutingPolicy &&) noexcept = default;
    RoutingPolicy &operator=(const RoutingPolicy &) = delete;
    RoutingPolicy &operator=(RoutingPolicy &&) = delete;

private:
    RoutingPolicy() = default;
};

enum class TextureVariant
{
    Native
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
