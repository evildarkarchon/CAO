//! Texture loading and the Texture decisions, ported from C++ `TexturesOptimizer`.
//!
//! [`plan`] is C++'s `processArguments`: from a Texture's metadata, its name, the
//! profile and the user's options it decides whether the Texture is resized,
//! compressed and given mipmaps. Dry Run reports that decision and Apply acts on
//! it, so both share this one function, as they share `processArguments` in C++.
//!
//! [`Texture::optimize`] is C++ `optimize` on the CPU path the Texture decisions
//! already need (#491): decompress, resize without WIC, and convert or compress
//! to the target format. Mipmap generation may use WIC, which needs COM on the
//! Run Worker, so it arrives with the rest of the Texture behaviour (#494) and
//! fails cleanly until then. GPU BC7/BC6H encoding is a later slice; until then
//! every format is compressed on the CPU, as C++ does without a device.

use std::path::Path;

use cao_core::routing::TextureVariant;
use directxtex::{
    DDS_FLAGS_NONE, DXGI_FORMAT, HResultError, ScratchImage, TEX_ALPHA_MODE_OPAQUE,
    TEX_COMPRESS_DEFAULT, TEX_FILTER_DEFAULT, TEX_FILTER_FORCE_NON_WIC, TEX_FILTER_SEPARATE_ALPHA,
    TEX_THRESHOLD_DEFAULT, TGA_FLAGS_NONE, TexMetadata,
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
    /// One DirectXTex step of applying a decision failed.
    #[error("DirectXTex could not {step} the Texture: {source}")]
    Process {
        step: &'static str,
        #[source]
        source: HResultError,
    },
    /// A conversion produced a format other than the one asked for.
    #[error("the conversion produced {produced:?} instead of {requested:?}")]
    UnexpectedFormat {
        requested: DXGI_FORMAT,
        produced: DXGI_FORMAT,
    },
    /// A step this build cannot perform yet.
    #[error("{0} is not available in this build")]
    Unavailable(&'static str),
}

/// Wraps a DirectXTex error with the step that raised it.
fn step(step: &'static str) -> impl FnOnce(HResultError) -> TextureError {
    move |source| TextureError::Process { step, source }
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

    /// Applies the decision for `request` to the loaded pixels, as C++
    /// `optimize` does, and reports whether they changed
    /// (`modifiedCurrentTexture`).
    ///
    /// The metadata saved with the Texture is updated field by field, as C++
    /// updates `_info`, rather than replaced by each step's result, so the DDS
    /// header matches the oracle's.
    ///
    /// # Errors
    /// [`TextureError`] when a DirectXTex step fails, or when the decision
    /// needs mipmaps, which this build cannot generate yet (#494). The loaded
    /// pixels may then be partly processed; nothing on disk has changed.
    pub fn optimize(
        &mut self,
        profile: &TextureProfile,
        request: &TextureRequest,
    ) -> Result<bool, TextureError> {
        let plan = self.plan(profile, request);
        if !plan.would_change() {
            log::debug!("This texture does not need optimization.");
            return Ok(false);
        }
        let mut modified = false;
        if self.info.format.is_compressed() {
            log::debug!("Decompressing this texture.");
            let image = self
                .image
                .decompress(DXGI_FORMAT::DXGI_FORMAT_UNKNOWN)
                .map_err(step("decompress"))?;
            self.info.format = image.metadata().format;
            self.image = image;
            modified = true;
        }
        let mut mipmaps = plan.mipmaps;
        if plan.resize {
            log::debug!("Resizing this texture.");
            modified |= self.resize(plan.width, plan.height)?;
            mipmaps = request.mipmaps
                && self.info.mip_levels != optimal_mip_count(self.info.width, self.info.height)
                && can_have_mipmaps(&self.info, self.is_interface(), profile);
        }
        if mipmaps {
            return Err(TextureError::Unavailable("Mipmap generation"));
        }
        let mut target = self.info.format;
        if plan.compress {
            target = profile.format;
            log::debug!("Converting this texture to format: {target:?}");
        }
        // Cannot compress once the Texture is smaller than 4x4.
        if !can_be_compressed(&self.info, self.is_interface(), profile) {
            target = DXGI_FORMAT::DXGI_FORMAT_B8G8R8A8_UNORM;
        }
        modified |= self.convert(target)?;
        log::info!("Successfully processed texture: {}", self.name);
        Ok(modified)
    }

    /// The Texture as DDS file bytes, written with the C++-maintained metadata.
    ///
    /// # Errors
    /// [`TextureError::Process`] when DirectXTex cannot encode it.
    pub fn save_dds(&self) -> Result<Vec<u8>, TextureError> {
        let blob = directxtex::save_dds(self.image.images(), &self.info, DDS_FLAGS_NONE)
            .map_err(step("save"))?;
        Ok(blob.buffer().to_vec())
    }

    /// C++ looks for `interface` anywhere in the loaded path, ignoring case.
    fn is_interface(&self) -> bool {
        self.name.to_lowercase().contains("interface")
    }

    /// Halves towards the target as C++ `resize` does: the target is rounded
    /// up to powers of two, and WIC is never used, so a large Texture is not
    /// expanded to 128-bit floats.
    fn resize(&mut self, mut width: usize, mut height: usize) -> Result<bool, TextureError> {
        if self.info.width <= width && self.info.height <= height {
            return Ok(false);
        }
        width = width.next_power_of_two();
        height = height.next_power_of_two();
        let image = self
            .image
            .resize(
                width,
                height,
                TEX_FILTER_SEPARATE_ALPHA | TEX_FILTER_FORCE_NON_WIC,
            )
            .map_err(step("resize"))?;
        let resized = image.metadata();
        self.info.width = resized.width;
        self.info.height = resized.height;
        self.info.mip_levels = 1;
        self.image = image;
        Ok(true)
    }

    /// Converts to `format`, compressing when it is a block format, as C++
    /// `convert` does. Returns whether the pixels changed.
    fn convert(&mut self, format: DXGI_FORMAT) -> Result<bool, TextureError> {
        if format.is_compressed() {
            if self.info.format.is_compressed() || self.image.metadata().format == format {
                return Ok(false);
            }
            // C++ also passed TEX_FILTER_SEPARATE_ALPHA, which Compress ignores.
            let image = self
                .image
                .compress(format, TEX_COMPRESS_DEFAULT, TEX_THRESHOLD_DEFAULT)
                .map_err(step("compress"))?;
            self.info.format = image.metadata().format;
            self.image = image;
            return Ok(true);
        }
        if self.info.format == format {
            return Ok(false);
        }
        let image = self
            .image
            .convert(format, TEX_FILTER_DEFAULT, TEX_THRESHOLD_DEFAULT)
            .map_err(step("convert"))?;
        let produced = image.metadata().format;
        if produced != format {
            return Err(TextureError::UnexpectedFormat {
                requested: format,
                produced,
            });
        }
        self.info.format = produced;
        self.image = image;
        Ok(true)
    }
}
