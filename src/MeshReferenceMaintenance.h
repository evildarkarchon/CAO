#pragma once

#include <functional>
#include <string>

namespace nifly {
class NifFile;
}

namespace cao::execution {
/// Decides whether one referenced Texture name may be rewritten from TGA to DDS. It is consulted
/// only for names that already reference a TGA, so a rejected name is always a withheld rewrite
/// rather than an unrelated reference. An empty filter leaves every referenced TGA eligible.
using ReferencedTextureFilter = std::function<bool(const std::string&)>;

/// Reports whether a Mesh contains an eligible referenced Texture name with a case-insensitive TGA
/// suffix.
[[nodiscard]] bool hasReferencedTgaTexture(const nifly::NifFile& mesh,
                                           const ReferencedTextureFilter& isEligible = {});

/// Replaces every eligible referenced TGA Texture name with DDS and reports whether the Mesh
/// changed.
bool replaceReferencedTgaTextureNames(nifly::NifFile& mesh,
                                      const ReferencedTextureFilter& isEligible = {});
}  // namespace cao::execution
