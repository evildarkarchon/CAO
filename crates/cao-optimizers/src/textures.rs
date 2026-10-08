//! Texture loading and the Texture decisions, ported from C++ `TexturesOptimizer`.
//!
//! [`plan`] is C++'s `processArguments`: from a Texture's metadata, its name, the
//! profile and the user's options it decides whether the Texture is resized,
//! compressed and given mipmaps. Dry Run reports that decision and Apply acts on
//! it, so both share this one function, as they share `processArguments` in C++.
//!
//! Only loading and deciding are ported so far. Applying the decision (decompress,
//! resize, mipmaps, convert and GPU BC7 encoding) arrives with the remaining
//! Texture behaviour (#494), together with the per-thread COM and D3D11 setup it
//! needs; Dry Run never calls WIC, so it needs neither.

use std::path::Path;

use cao_core::routing::TextureVariant;
use directxtex::{
    DDS_FLAGS_NONE, DXGI_FORMAT, HResultError, ScratchImage, TEX_ALPHA_MODE_OPAQUE, TGA_FLAGS_NONE,
    TexMetadata,
};

/// The profile's Texture settings a decision depends on, from `profile.ini`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureProfile {
    /// The format Textures are compressed to (`texturesFormat`).
    pub format: DXGI_FORMAT,
    /// Formats a Texture is converted away from (`texturesUnwantedFormats`).
    pub unwanted_formats: Vec<DXGI_FORMAT>,
    /// Whether interface Textures are compressed and mipmapped
    /// (`texturesCompressInterface`).
    pub compress_interface: bool,
}

/// What the user asked of one Texture: C++ `processArguments`' parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextureRequest {
    /// Necessary optimization: convert incompatible Textures and TGAs.
    pub necessary: bool,
    /// Compress uncompressed Textures to the profile's format.
    pub compress: bool,
    /// Generate the full mip chain.
    pub mipmaps: bool,
    /// The size to halve towards, already worked out from a ratio or a fixed size.
    pub target: Option<(usize, usize)>,
}

/// The decision for one Texture: C++ `TexOptOptionsResult`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TexturePlan {
    /// The size after resizing; the current size when there is no resize.
    pub width: usize,
    pub height: usize,
    pub resize: bool,
    /// Convert to the profile's format.
    pub compress: bool,
    pub mipmaps: bool,
}

impl TexturePlan {
    /// Whether any work is planned: C++ `dryOptimize`'s result.
    pub fn would_change(&self) -> bool {
        self.resize || self.compress || self.mipmaps
    }
}

/// Decides what to do with one Texture, as C++ `processArguments` does.
///
/// `name` is the path the Texture was loaded from. C++ looks for `interface`
/// anywhere in that full path, ignoring case, not just in the game path; that
/// quirk is kept, so a Mod Root under a folder named `interface` treats every
/// Texture as an interface Texture.
pub fn plan(
    info: &TexMetadata,
    name: &str,
    variant: TextureVariant,
    profile: &TextureProfile,
    request: &TextureRequest,
) -> TexturePlan {
    let (mut width, mut height) = (info.width, info.height);
    if let Some((target_width, target_height)) = request.target {
        while width > target_width && height > target_height {
            width /= 2;
            height /= 2;
        }
    }
    let resize = width != info.width || height != info.height;

    let is_interface = name.to_lowercase().contains("interface");
    let compress = (request.necessary
        && (is_incompatible(info, profile) || variant == TextureVariant::Convertible))
        || (request.compress
            && can_be_compressed(info, is_interface, profile)
            && info.format != profile.format);
    let mipmaps = request.mipmaps
        && info.mip_levels != optimal_mip_count(info.width, info.height)
        && can_have_mipmaps(info, is_interface, profile);

    TexturePlan {
        width,
        height,
        resize,
        compress,
        mipmaps,
    }
}

/// C++ `isIncompatible`: an unwanted format, an uncompressed cubemap without
/// usable alpha, or a compressed Texture whose sides are not powers of two.
fn is_incompatible(info: &TexMetadata, profile: &TextureProfile) -> bool {
    if profile.unwanted_formats.contains(&info.format) {
        return true;
    }
    let uncompressed = !info.format.is_compressed();
    let bad_alpha = info.get_alpha_mode() == TEX_ALPHA_MODE_OPAQUE || !info.format.has_alpha();
    let bad_cubemap = info.is_cubemap() && uncompressed && bad_alpha;
    let compressed_and_not_pow2 = !uncompressed && !is_power_of_two(info);
    bad_cubemap || compressed_and_not_pow2
}

/// C++ `canBeCompressed`: uncompressed, at least 4×4, power-of-two sides, and
/// not an interface Texture unless the profile compresses those.
fn can_be_compressed(info: &TexMetadata, is_interface: bool, profile: &TextureProfile) -> bool {
    let interface_okay = profile.compress_interface || !is_interface;
    let bad_size = info.width < 4 || info.height < 4;
    interface_okay && !info.format.is_compressed() && !bad_size && is_power_of_two(info)
}

/// C++ `canHaveMipMaps`: at least 4×4, and not an interface Texture unless the
/// profile compresses those.
fn can_have_mipmaps(info: &TexMetadata, is_interface: bool, profile: &TextureProfile) -> bool {
    let interface_okay = !is_interface || profile.compress_interface;
    interface_okay && info.width >= 4 && info.height >= 4
}

/// C++ `calculateOptimalMipMapsNumber`: the length of the full mip chain.
fn optimal_mip_count(mut width: usize, mut height: usize) -> usize {
    let mut count = 1;
    while width > 1 || height > 1 {
        width = (width >> 1).max(1);
        height = (height >> 1).max(1);
        count += 1;
    }
    count
}

fn is_power_of_two(info: &TexMetadata) -> bool {
    info.width.is_power_of_two() && info.height.is_power_of_two()
}

/// Why a Texture could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum TextureError {
    #[error("the file could not be read: {0}")]
    Read(#[from] std::io::Error),
    #[error("DirectXTex could not decode the file: {0}")]
    Decode(#[from] HResultError),
    /// A typeless DDS format with no UNORM equivalent, which C++ refused to load.
    #[error("the typeless format {0:?} has no UNORM equivalent")]
    Typeless(DXGI_FORMAT),
}

/// One loaded Texture: its pixels, its metadata and the path it came from.
pub struct Texture {
    image: ScratchImage,
    info: TexMetadata,
    name: String,
    variant: TextureVariant,
}

impl Texture {
    /// Loads a DDS (a Native Texture) or a TGA (a Convertible one), as C++ `open` does.
    ///
    /// A typeless DDS format is reinterpreted as its UNORM equivalent; one that
    /// has none fails to load. The file is read whole and decoded from memory,
    /// so its path has no length limit (deviation 5: C++ copied it into a
    /// 1024-character buffer). Nothing on disk changes.
    ///
    /// # Errors
    /// [`TextureError`] when the file cannot be read or decoded.
    pub fn load(path: &Path, variant: TextureVariant) -> Result<Self, TextureError> {
        let bytes = std::fs::read(path)?;
        let mut info = TexMetadata::default();
        let mut image = match variant {
            TextureVariant::Native => {
                ScratchImage::load_dds(&bytes, DDS_FLAGS_NONE, Some(&mut info), None)?
            }
            TextureVariant::Convertible => {
                ScratchImage::load_tga(&bytes, TGA_FLAGS_NONE, Some(&mut info))?
            }
        };
        // C++ checks typeless formats on DDS only; a TGA never decodes to one.
        // `IsTypeless` defaults to counting partially typeless formats.
        if variant == TextureVariant::Native && info.format.is_typeless(true) {
            let format = info.format.make_typeless_unorm();
            if format.is_typeless(true) {
                return Err(TextureError::Typeless(info.format));
            }
            info.format = format;
            // C++ ignores `OverrideFormat`'s result too. It fails only when the
            // bits per pixel differ, which `MakeTypelessUNORM` never changes.
            let _ = image.override_format(format);
        }
        Ok(Self {
            image,
            info,
            name: path.to_string_lossy().into_owned(),
            variant,
        })
    }

    /// The Texture's metadata, with a typeless format already made UNORM.
    pub fn metadata(&self) -> &TexMetadata {
        &self.info
    }

    /// The path the Texture was loaded from, as text.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The loaded pixels.
    pub fn image(&self) -> &ScratchImage {
        &self.image
    }

    /// Decides what to do with this Texture; see [`plan`].
    pub fn plan(&self, profile: &TextureProfile, request: &TextureRequest) -> TexturePlan {
        plan(&self.info, &self.name, self.variant, profile, request)
    }
}
