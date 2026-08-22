#include "AssetRouter.h"

#include <array>
#include <utility>

namespace
{
/// Compares the terminal extension using the ASCII case rules that define supported Asset extensions.
bool hasNativeTextureExtension(const std::filesystem::path &executionPath)
{
    using PathCharacter = std::filesystem::path::value_type;
    constexpr std::array expectedExtension{
        static_cast<PathCharacter>('.'),
        static_cast<PathCharacter>('d'),
        static_cast<PathCharacter>('d'),
        static_cast<PathCharacter>('s')};

    const auto extension = executionPath.filename().extension().native();
    if (extension.size() != expectedExtension.size())
        return false;

    for (std::size_t index = 0; index < extension.size(); ++index) {
        auto character = extension[index];
        if (character >= static_cast<PathCharacter>('A')
            && character <= static_cast<PathCharacter>('Z')) {
            character += static_cast<PathCharacter>('a' - 'A');
        }

        if (character != expectedExtension[index])
            return false;
    }

    return true;
}
}

namespace cao::routing
{
RunRequest RunRequest::optimizeNativeTextures() noexcept
{
    return RunRequest();
}

ProfileCapabilities ProfileCapabilities::withNativeTextures() noexcept
{
    return ProfileCapabilities();
}

RoutingPolicy RoutingPolicy::compile(const RunRequest, const ProfileCapabilities) noexcept
{
    return RoutingPolicy();
}

TextureAsset::TextureAsset(const TextureVariant variant) noexcept
    : _variant(variant)
{}

TextureVariant TextureAsset::variant() const noexcept
{
    return _variant;
}

RoutedAsset::RoutedAsset(std::filesystem::path executionPath, const TextureAsset texture)
    : _executionPath(std::move(executionPath))
    , _texture(texture)
{}

const std::filesystem::path &RoutedAsset::executionPath() const noexcept
{
    return _executionPath;
}

const TextureAsset &RoutedAsset::texture() const noexcept
{
    return _texture;
}

AssetRouter::AssetRouter(RoutingPolicy policy) noexcept
    : _policy(std::move(policy))
{}

RoutingDecision AssetRouter::route(const std::filesystem::path &executionPath) const
{
    if (!hasNativeTextureExtension(executionPath))
        return UnsupportedDecision{};

    return RoutedAsset(executionPath, TextureAsset(TextureVariant::Native));
}
}
