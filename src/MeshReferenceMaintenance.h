#pragma once

namespace nifly
{
class NifFile;
}

namespace cao::execution
{
/// Reports whether a Mesh contains a referenced Texture name with a case-insensitive TGA suffix.
[[nodiscard]] bool hasReferencedTgaTexture(const nifly::NifFile &mesh);

/// Replaces every referenced TGA Texture name with DDS and reports whether the Mesh changed.
bool replaceReferencedTgaTextureNames(nifly::NifFile &mesh);
}
