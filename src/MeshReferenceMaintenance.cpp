#include "MeshReferenceMaintenance.h"

#include <nifly/NifFile.hpp>

#include <algorithm>
#include <cctype>
#include <string>
#include <string_view>

namespace cao::execution {
namespace {
constexpr std::string_view tgaExtension = ".tga";

bool matchesTgaAt(const std::string_view value, const std::size_t position) {
    if (position + tgaExtension.size() > value.size()) return false;

    for (std::size_t index = 0; index < tgaExtension.size(); ++index) {
        const auto character = static_cast<unsigned char>(value[position + index]);
        if (static_cast<char>(std::tolower(character)) != tgaExtension[index]) return false;
    }
    return true;
}

bool containsTga(const std::string_view value) {
    for (std::size_t position = 0; position < value.size(); ++position) {
        if (matchesTgaAt(value, position)) return true;
    }
    return false;
}

bool replaceTga(std::string& value) {
    bool changed = false;
    for (std::size_t position = 0; position < value.size(); ++position) {
        if (!matchesTgaAt(value, position)) continue;
        value.replace(position, tgaExtension.size(), ".dds");
        position += tgaExtension.size() - 1;
        changed = true;
    }
    return changed;
}
}  // namespace

bool hasReferencedTgaTexture(const nifly::NifFile& mesh,
                             const ReferencedTextureFilter& isEligible) {
    for (auto* shape : mesh.GetShapes()) {
        for (const auto texture : mesh.GetTexturePathRefs(shape)) {
            const std::string& reference = texture.get();
            // The filter only ever sees names that would actually be rewritten, so a caller can
            // treat every rejection as one withheld rewrite.
            if (containsTga(reference) && (!isEligible || isEligible(reference))) return true;
        }
    }
    return false;
}

bool replaceReferencedTgaTextureNames(nifly::NifFile& mesh,
                                      const ReferencedTextureFilter& isEligible) {
    bool changed = false;
    for (auto* shape : mesh.GetShapes()) {
        for (auto texture : mesh.GetTexturePathRefs(shape)) {
            std::string& reference = texture.get();
            if (!containsTga(reference)) continue;
            if (isEligible && !isEligible(reference)) continue;
            changed = replaceTga(reference) || changed;
        }
    }
    return changed;
}
}  // namespace cao::execution
