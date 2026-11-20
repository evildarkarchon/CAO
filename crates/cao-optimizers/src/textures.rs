//! Texture loading and the Texture decisions.
//!
//! [`plan`] decides, from a Texture's metadata, its game path, the profile
//! and the user's options, whether the Texture is resized, compressed and
//! given mipmaps. Dry Run reports that decision and Apply acts on it, so both
//! share this one function. Both modes load and decode the whole Texture
//! ([`Texture::load`]) although the decision needs only the metadata, so a
//! Dry Run verifies every Texture exactly as Apply does: a corrupt body fails
//! the load in either mode.
//!
//! [`Texture::optimize`] applies the decision (#491, #494): decompress,
//! resize without WIC, generate mipmaps, and convert or compress to the
//! target format. Mipmap generation may use WIC, so the calling thread must
//! have joined COM ([`crate::device::initialize_com`]); the backend does that
//! on the Run Worker. BC6H and BC7 are encoded on the GPU when the caller has
//! a [`GpuDevice`] and on the CPU otherwise, or when the GPU encode fails
//! (#495, audit BE-10); every other format is always encoded on the CPU.
//!
//! After each step the metadata the Texture carries is checked against the
//! step's result, every compared field alike, so a step that produced
//! something other than what the Texture's metadata describes fails rather
//! than being saved under a header that misdescribes its pixels.

use std::path::Path;

use cao_core::routing::TextureVariant;
use directxtex::{
    Blob, CP_FLAGS_NONE, DDS_FLAGS_NONE, DXGI_FORMAT, HResultError, Image, Rect, ScratchImage,
    TEX_ALPHA_MODE_OPAQUE, TEX_COMPRESS_DEFAULT, TEX_FILTER_DEFAULT, TEX_FILTER_FORCE_NON_WIC,
    TEX_FILTER_SEPARATE_ALPHA, TEX_THRESHOLD_DEFAULT, TGA_FLAGS_NONE, TexMetadata,
};

use crate::device::GpuDevice;

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

/// What the user asked of one Texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextureRequest {
    /// Necessary optimization: convert incompatible Textures and TGAs.
    pub necessary: bool,
    /// Compress uncompressed Textures to the profile's format.
    pub compress: bool,
    /// Generate the full mip chain.
    pub mipmaps: bool,
    /// The size to halve towards, already worked out from a ratio or a fixed
    /// size. A side of 0, which a ratio larger than the Texture gives, counts
    /// as 1.
    pub target: Option<(usize, usize)>,
}

/// The decision for one Texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TexturePlan {
    /// The size after resizing, rounded up to powers of two as Apply writes
    /// it; the current size when there is no resize.
    pub width: usize,
    pub height: usize,
    pub resize: bool,
    /// Convert to the profile's format.
    pub compress: bool,
    /// Generate the full mip chain, for the size after any resize.
    pub mipmaps: bool,
}

impl TexturePlan {
    /// Whether any work is planned.
    pub fn would_change(&self) -> bool {
        self.resize || self.compress || self.mipmaps
    }
}

/// Decides what to do with one Texture: Dry Run reports the decision and
/// Apply acts on it.
///
/// `game_path` is the Texture's path within its Mod Root, with either
/// separator, such as `textures/armor/plate.dds`; whether it is an
/// interface Texture is read from it ([`is_interface_texture`]), never from
/// where the Mod Root lies (audit BE-11).
///
/// A resize halves both sides while both exceed the target, then rounds each
/// side up to a power of two, which is the size Apply writes and Dry Run
/// reports. A target side of 0 counts as 1, so a 1×1 Texture is never planned
/// smaller. Mipmaps are decided for the size after any resize, which leaves
/// a single level; a volume Texture gets none, because DirectXTex's mipmap
/// generation for 2D Textures refuses it (audit BE-12).
pub fn plan(
    info: &TexMetadata,
    game_path: &str,
    variant: TextureVariant,
    profile: &TextureProfile,
    request: &TextureRequest,
) -> TexturePlan {
    let (mut width, mut height) = (info.width, info.height);
    if let Some((target_width, target_height)) = request.target {
        let (target_width, target_height) = (target_width.max(1), target_height.max(1));
        // Each side stays at least 1: halving only a side above a target of
        // at least 1.
        while width > target_width && height > target_height {
            width /= 2;
            height /= 2;
        }
    }
    let resize = width != info.width || height != info.height;
    let mut resized = *info;
    if resize {
        // Rounding a halved side up never reaches the side it was halved
        // from, so a planned resize always shrinks.
        resized.width = width.next_power_of_two();
        resized.height = height.next_power_of_two();
        resized.mip_levels = 1;
    }

    let is_interface = is_interface_texture(game_path);
    let compress = (request.necessary
        && (is_incompatible(info, profile) || variant == TextureVariant::Convertible))
        || (request.compress
            && can_be_compressed(info, is_interface, profile)
            && info.format != profile.format);
    let mipmaps = request.mipmaps && needs_mipmaps(&resized, is_interface, profile);

    TexturePlan {
        width: resized.width,
        height: resized.height,
        resize,
        compress,
        mipmaps,
    }
}

/// Whether the Texture at `game_path`, its path within its Mod Root with
/// either separator, is interface art: one of its folders, not its file
/// name, is named `interface`, in any ASCII case.
///
/// Interface art ships both at the top level (`interface/...`) and under
/// `textures/interface/...`, so any folder counts. Only whole folder names
/// within the Mod Root do: a Mod Root beneath a folder whose name contains
/// `interface` no longer turns every Texture in it into interface art
/// (audit BE-11).
pub fn is_interface_texture(game_path: &str) -> bool {
    let components: Vec<&str> = game_path
        .split(['/', '\\'])
        .filter(|component| !component.is_empty())
        .collect();
    let folders = components.len().saturating_sub(1);
    components[..folders]
        .iter()
        .any(|folder| folder.eq_ignore_ascii_case("interface"))
}

/// An unwanted format, an uncompressed cubemap without usable alpha, or a
/// compressed Texture whose sides are not powers of two.
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

/// Uncompressed, of a size a block format takes ([`has_block_size`]), and
/// not an interface Texture unless the profile compresses those.
fn can_be_compressed(info: &TexMetadata, is_interface: bool, profile: &TextureProfile) -> bool {
    let interface_okay = profile.compress_interface || !is_interface;
    interface_okay && !info.format.is_compressed() && has_block_size(info)
}

/// At least 4×4 with power-of-two sides: the sizes CAO compresses to a block
/// format.
fn has_block_size(info: &TexMetadata) -> bool {
    info.width >= 4 && info.height >= 4 && is_power_of_two(info)
}

/// Whether a Texture with `info` lacks the full mip chain and may have one:
/// a 2D Texture of at least 4×4, and not an interface Texture unless the
/// profile compresses those. A volume Texture never may (audit BE-12).
fn needs_mipmaps(info: &TexMetadata, is_interface: bool, profile: &TextureProfile) -> bool {
    let interface_okay = !is_interface || profile.compress_interface;
    interface_okay
        && !info.is_volumemap()
        && info.width >= 4
        && info.height >= 4
        && info.mip_levels != optimal_mip_count(info.width, info.height)
}

/// The length of the full mip chain.
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
    /// DirectXTex refused the bytes. `hresult` is its code and `message` the
    /// code's rendering; the error itself is not kept, because every accessor
    /// of `HResultError` consumes it and it is neither `Clone` nor `Copy`.
    #[error("DirectXTex could not decode the file: {message}")]
    Decode { hresult: u32, message: String },
    /// A typeless DDS format with no UNORM equivalent, which no step could
    /// interpret.
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
    /// A step's result does not have the metadata the Texture should have
    /// after it, so saving it would write a header that misdescribes it.
    #[error("the result of the {step} step does not match the Texture: {detail}")]
    MetadataMismatch { step: &'static str, detail: String },
    /// An array item or cubemap face has no top-level image to copy when a
    /// partial mip chain is cut back, which a `ScratchImage` holding that
    /// many items never lacks.
    #[error("the Texture has no top-level image for item {0}")]
    MissingImage(usize),
}

impl From<HResultError> for TextureError {
    /// A decode failure, keeping the code so it can be classified later.
    fn from(error: HResultError) -> Self {
        let message = error.to_string();
        Self::Decode {
            hresult: error.into_underlying(),
            message,
        }
    }
}

impl TextureError {
    /// `E_OUTOFMEMORY`, which DirectXTex returns when an allocation fails.
    const E_OUTOFMEMORY: u32 = 0x8007_000E;

    /// Whether loading failed for a reason outside the file's content: it
    /// could not be read, or decoding it ran out of memory, as a very large
    /// Texture can on a busy machine. Such a file is not known to be corrupt.
    pub fn is_unreadable(&self) -> bool {
        match self {
            Self::Read(_) => true,
            Self::Decode { hresult, .. } => *hresult == Self::E_OUTOFMEMORY,
            _ => false,
        }
    }
}

/// Wraps a DirectXTex error with the step that raised it.
fn step(step: &'static str) -> impl FnOnce(HResultError) -> TextureError {
    move |source| TextureError::Process { step, source }
}

/// Checks that a step's `produced` metadata has every compared field equal
/// to the `expected` metadata the Texture carries after the step. The mip
/// count, which a step may change, and `misc_flags2`'s alpha mode, which
/// steps do not keep, are not compared.
///
/// Every field must match, so a step whose result differs in any one of them
/// fails rather than being saved under metadata that misdescribes it.
///
/// # Errors
/// [`TextureError::MetadataMismatch`] naming `step` and the first field that
/// differs.
fn check_metadata(
    step: &'static str,
    expected: &TexMetadata,
    produced: &TexMetadata,
) -> Result<(), TextureError> {
    let mismatch = |field: &str, expected: &dyn std::fmt::Debug, produced: &dyn std::fmt::Debug| {
        Err(TextureError::MetadataMismatch {
            step,
            detail: format!("its {field} is {produced:?}, not {expected:?}"),
        })
    };
    if expected.width != produced.width {
        return mismatch("width", &expected.width, &produced.width);
    }
    if expected.height != produced.height {
        return mismatch("height", &expected.height, &produced.height);
    }
    if expected.depth != produced.depth {
        return mismatch("depth", &expected.depth, &produced.depth);
    }
    if expected.array_size != produced.array_size {
        return mismatch("array size", &expected.array_size, &produced.array_size);
    }
    if expected.misc_flags != produced.misc_flags {
        return mismatch("misc flags", &expected.misc_flags, &produced.misc_flags);
    }
    if expected.format != produced.format {
        return mismatch("format", &expected.format, &produced.format);
    }
    if expected.dimension != produced.dimension {
        return mismatch("dimension", &expected.dimension, &produced.dimension);
    }
    Ok(())
}

/// Reinterprets a typeless DDS format in `info` as its UNORM equivalent and
/// returns the format to give the pixels, or `None` when the format was not
/// typeless. `IsTypeless` counts partially typeless formats too.
///
/// # Errors
/// [`TextureError::Typeless`] when the format has no UNORM equivalent.
fn untypeless(info: &mut TexMetadata) -> Result<Option<DXGI_FORMAT>, TextureError> {
    if !info.format.is_typeless(true) {
        return Ok(None);
    }
    let format = info.format.make_typeless_unorm();
    if format.is_typeless(true) {
        return Err(TextureError::Typeless(info.format));
    }
    info.format = format;
    Ok(Some(format))
}

/// One loaded Texture: its pixels, its metadata and the path it came from.
pub struct Texture {
    image: ScratchImage,
    info: TexMetadata,
    name: String,
    variant: TextureVariant,
}

impl Texture {
    /// Loads a DDS (a Native Texture) or a TGA (a Convertible one).
    ///
    /// A typeless DDS format is reinterpreted as its UNORM equivalent; one that
    /// has none fails to load. The file is read whole and decoded from memory,
    /// so its path has no length limit. Nothing on disk changes.
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
        // A TGA never decodes to a typeless format.
        if variant == TextureVariant::Native
            && let Some(format) = untypeless(&mut info)?
        {
            // `OverrideFormat` fails only when the bits per pixel differ,
            // which `MakeTypelessUNORM` never changes, so its result is moot.
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

    /// Decides what to do with this Texture, whose path within its Mod Root
    /// is `game_path`; see [`plan`].
    pub fn plan(
        &self,
        game_path: &str,
        profile: &TextureProfile,
        request: &TextureRequest,
    ) -> TexturePlan {
        plan(&self.info, game_path, self.variant, profile, request)
    }

    /// Applies the decision for `request` to the loaded pixels and reports
    /// whether they changed. `game_path` is the Texture's path within its Mod
    /// Root, as [`plan`] takes it.
    ///
    /// The metadata saved with the Texture is updated field by field rather
    /// than replaced by each step's result, so [`check_metadata`] can compare
    /// the two.
    ///
    /// Mipmap generation may go through WIC, so the calling thread must have
    /// joined COM ([`crate::device::initialize_com`]). BC6H and BC7 are
    /// encoded with `gpu` when there is one, and on the CPU otherwise or when
    /// the GPU encode fails.
    ///
    /// # Errors
    /// [`TextureError`] when a DirectXTex step fails or its result's metadata
    /// does not match the Texture's. The loaded pixels may then be partly
    /// processed; nothing on disk has changed.
    pub fn optimize(
        &mut self,
        game_path: &str,
        profile: &TextureProfile,
        request: &TextureRequest,
        gpu: Option<&GpuDevice>,
    ) -> Result<bool, TextureError> {
        let plan = self.plan(game_path, profile, request);
        if !plan.would_change() {
            log::debug!("This texture does not need optimization.");
            return Ok(false);
        }
        // The target is picked before decompressing, so a compressed Texture
        // that is only resized or mipmapped is compressed back to its format.
        let mut target = self.info.format;
        let mut modified = false;
        if self.info.format.is_compressed() {
            log::debug!("Decompressing this texture.");
            let image = self
                .image
                .decompress(DXGI_FORMAT::DXGI_FORMAT_UNKNOWN)
                .map_err(step("decompress"))?;
            self.info.format = image.metadata().format;
            check_metadata("decompress", &self.info, image.metadata())?;
            self.image = image;
            modified = true;
        }
        if plan.resize {
            log::debug!("Resizing this texture.");
            modified |= self.resize(plan.width, plan.height)?;
        }
        // The plan decided mipmaps for the size after the resize.
        if plan.mipmaps {
            log::debug!("Generating mipmaps for this texture.");
            self.generate_mipmaps()?;
            modified = true;
        }
        if plan.compress {
            target = profile.format;
            log::debug!("Converting this texture to format: {target:?}");
        }
        // A block format needs a size it takes, which a resize or the Texture
        // itself may not give, and interface art the profile keeps
        // uncompressed is not newly compressed to the profile's format. Such
        // a Texture is written as uncompressed BGRA8, which every game reads
        // (audit BE-11). A Texture that was already compressed and is only
        // resized keeps its own block format.
        let keeps_interface_uncompressed =
            plan.compress && is_interface_texture(game_path) && !profile.compress_interface;
        if target.is_compressed() && (!has_block_size(&self.info) || keeps_interface_uncompressed) {
            target = DXGI_FORMAT::DXGI_FORMAT_B8G8R8A8_UNORM;
        }
        modified |= self.convert(target, gpu)?;
        log::info!("Successfully processed texture: {}", self.name);
        Ok(modified)
    }

    /// The Texture encoded as a DDS file, with the metadata it carries, in
    /// DirectXTex's buffer, so the caller writes it without another copy
    /// (audit BE-9).
    ///
    /// # Errors
    /// [`TextureError::Process`] when DirectXTex cannot encode it.
    pub fn encode_dds(&self) -> Result<Blob, TextureError> {
        directxtex::save_dds(self.image.images(), &self.info, DDS_FLAGS_NONE).map_err(step("save"))
    }

    /// Resizes to `width` × `height`, the planned size, which is already
    /// rounded up to powers of two. WIC is never used, so a large Texture is
    /// not expanded to 128-bit floats.
    fn resize(&mut self, width: usize, height: usize) -> Result<bool, TextureError> {
        if self.info.width <= width && self.info.height <= height {
            return Ok(false);
        }
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
        check_metadata("resize", &self.info, resized)?;
        self.image = image;
        Ok(true)
    }

    /// Generates the full mip chain, filtering alpha separately.
    ///
    /// A partial chain is first cut back to its top level, because DirectXTex
    /// generates mips from a single base image. The filter does not force the
    /// non-WIC path, so DirectXTex uses WIC for most 8-bit formats; the
    /// calling thread must have joined COM.
    fn generate_mipmaps(&mut self) -> Result<(), TextureError> {
        let levels = optimal_mip_count(self.info.width, self.info.height);
        if self.info.mip_levels != 1 && self.info.mip_levels != levels {
            let mut top = ScratchImage::default();
            let mut top_info = self.info;
            top_info.mip_levels = 1;
            top.initialize(&top_info, CP_FLAGS_NONE)
                .map_err(step("copy the top mip level of"))?;
            let whole = Rect {
                x: 0,
                y: 0,
                w: self.info.width,
                h: self.info.height,
            };
            for item in 0..self.info.array_size {
                let (Some(source), Some(destination)) =
                    (self.image.image(0, item, 0), top.image(0, item, 0))
                else {
                    return Err(TextureError::MissingImage(item));
                };
                // `ScratchImage` lends its images only by shared reference, so
                // the destination is described by a copy of its fields; the
                // pixels pointer still points into `top`, which nothing else
                // borrows while DirectXTex writes through it (#459).
                let mut destination = Image {
                    width: destination.width,
                    height: destination.height,
                    format: destination.format,
                    row_pitch: destination.row_pitch,
                    slice_pitch: destination.slice_pitch,
                    pixels: destination.pixels,
                };
                destination
                    .copy_rectangle(source, &whole, TEX_FILTER_SEPARATE_ALPHA, 0, 0)
                    .map_err(step("copy the top mip level of"))?;
            }
            self.info.mip_levels = top.metadata().mip_levels;
            self.image = top;
        }
        if self.info.width > 1 || self.info.height > 1 || self.info.depth > 1 {
            let image = self
                .image
                .generate_mip_maps(TEX_FILTER_SEPARATE_ALPHA, levels)
                .map_err(step("generate mipmaps for"))?;
            self.info.mip_levels = image.metadata().mip_levels;
            check_metadata("mipmap", &self.info, image.metadata())?;
            self.image = image;
        }
        Ok(())
    }

    /// Converts to `format`, compressing when it is a block format, and
    /// returns whether the pixels changed.
    ///
    /// BC6H and BC7 go to `gpu` when there is one. A GPU encode that fails,
    /// as on a driver reset or when video memory runs out, is retried on the
    /// CPU with a warning, so one Texture does not fail on a GPU fault; a
    /// device found lost is not tried again, and the backend drops it (audit
    /// BE-10).
    fn convert(
        &mut self,
        format: DXGI_FORMAT,
        gpu: Option<&GpuDevice>,
    ) -> Result<bool, TextureError> {
        if format.is_compressed() {
            if self.info.format.is_compressed() || self.image.metadata().format == format {
                return Ok(false);
            }
            let on_cpu = |image: &ScratchImage| {
                image.compress(format, TEX_COMPRESS_DEFAULT, TEX_THRESHOLD_DEFAULT)
            };
            let image = match gpu.filter(|gpu| GpuDevice::encodes(format) && !gpu.is_lost()) {
                Some(gpu) => gpu.compress(&self.image, format).or_else(|error| {
                    log::warn!(
                        "The GPU could not compress {} ({error}); compressing it on the CPU.",
                        self.name
                    );
                    on_cpu(&self.image)
                }),
                None => on_cpu(&self.image),
            }
            .map_err(step("compress"))?;
            self.info.format = image.metadata().format;
            check_metadata("compress", &self.info, image.metadata())?;
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
        check_metadata("convert", &self.info, image.metadata())?;
        self.image = image;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use directxtex::{
        DXGI_FORMAT_BC7_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM, TEX_DIMENSION_TEXTURE2D,
        TEX_DIMENSION_TEXTURE3D, TEX_MISC_TEXTURECUBE,
    };

    use super::*;

    fn metadata() -> TexMetadata {
        TexMetadata {
            width: 64,
            height: 32,
            depth: 1,
            array_size: 1,
            mip_levels: 7,
            misc_flags: 0,
            misc_flags2: 0,
            format: DXGI_FORMAT::DXGI_FORMAT_R8G8B8A8_UNORM,
            dimension: TEX_DIMENSION_TEXTURE2D,
        }
    }

    /// A result that differs from the Texture's metadata in any compared
    /// field fails the step, even when every other field matches.
    ///
    /// This pins the check itself rather than a run, because no DirectXTex
    /// step CAO calls returns metadata that differs from what the Texture
    /// expects, so no Texture can reach the mismatch through the backend. The
    /// Apply scenarios show the check never rejects a real step.
    #[test]
    fn a_step_result_differing_in_any_compared_field_is_a_mismatch() {
        /// A compared field's name and a change to just that field.
        type Change = (&'static str, fn(&mut TexMetadata));
        let expected = metadata();
        let changes: [Change; 7] = [
            ("width", |info| info.width = 32),
            ("height", |info| info.height = 64),
            ("depth", |info| info.depth = 2),
            ("array size", |info| info.array_size = 6),
            ("misc flags", |info| {
                info.misc_flags = TEX_MISC_TEXTURECUBE.bits()
            }),
            ("format", |info| {
                info.format = DXGI_FORMAT::DXGI_FORMAT_B8G8R8A8_UNORM;
            }),
            ("dimension", |info| info.dimension = TEX_DIMENSION_TEXTURE3D),
        ];
        for (field, change) in changes {
            let mut produced = expected;
            change(&mut produced);
            let error = check_metadata("resize", &expected, &produced).unwrap_err();
            let TextureError::MetadataMismatch { step, detail } = &error else {
                panic!("{field}: {error}");
            };
            assert_eq!(*step, "resize");
            assert!(detail.starts_with(&format!("its {field} is")), "{detail}");
        }
    }

    /// The fields that are never compared still pass: the mip count, which a
    /// step is expected to change, and `misc_flags2`'s alpha mode.
    #[test]
    fn the_mip_count_and_alpha_mode_are_not_compared() {
        let expected = metadata();
        let mut produced = expected;
        produced.mip_levels = 1;
        produced.misc_flags2 = TEX_ALPHA_MODE_OPAQUE.bits();
        check_metadata("mipmap", &expected, &produced).unwrap();
    }

    /// A 64×64 RGBA8 Texture with smooth gradients, held in memory, as
    /// [`Texture::load`] would hold it.
    fn gradient_texture() -> Texture {
        let mut image = ScratchImage::default();
        image
            .initialize_2d(DXGI_FORMAT_R8G8B8A8_UNORM, 64, 64, 1, 1, CP_FLAGS_NONE)
            .unwrap();
        let row_pitch = image.images()[0].row_pitch;
        for (y, row) in image.pixels_mut().chunks_exact_mut(row_pitch).enumerate() {
            for (x, texel) in row.as_chunks_mut::<4>().0.iter_mut().take(64).enumerate() {
                let (x, y) = ((x * 4) as u8, (y * 4) as u8);
                *texel = [x, y, x / 2 + y / 2, 255 - x];
            }
        }
        Texture {
            info: *image.metadata(),
            image,
            name: "textures/gradient.dds".to_owned(),
            variant: TextureVariant::Native,
        }
    }

    /// Compresses a fresh gradient Texture to BC7 with `gpu` and returns its
    /// pixels.
    fn compressed_with(gpu: &GpuDevice) -> Vec<u8> {
        let profile = TextureProfile {
            format: DXGI_FORMAT_BC7_UNORM,
            unwanted_formats: Vec::new(),
            compress_interface: true,
        };
        let request = TextureRequest {
            compress: true,
            ..TextureRequest::default()
        };
        let mut texture = gradient_texture();
        assert!(
            texture
                .optimize("textures/gradient.dds", &profile, &request, Some(gpu))
                .unwrap()
        );
        assert_eq!(texture.metadata().format, DXGI_FORMAT_BC7_UNORM);
        texture.image().pixels().to_vec()
    }

    /// Audit BE-10: a GPU encode that fails is retried on the CPU, so the
    /// Texture is still compressed, to the CPU encoder's bytes. A failure that
    /// left the device lost marks it so, and later Textures go straight to
    /// the CPU; one that did not keeps the device in use.
    #[test]
    fn a_failed_gpu_encode_falls_back_to_the_cpu() {
        let on_cpu = gradient_texture()
            .image()
            .compress(
                DXGI_FORMAT_BC7_UNORM,
                TEX_COMPRESS_DEFAULT,
                TEX_THRESHOLD_DEFAULT,
            )
            .unwrap()
            .pixels()
            .to_vec();

        let gpu = GpuDevice::create(0).unwrap();
        let on_gpu = compressed_with(&gpu);
        // Three-subset BC7 modes make the GPU encoder's bytes differ.
        assert_ne!(on_gpu, on_cpu, "the encoders must differ");

        gpu.set_fault(Some(false));
        assert_eq!(compressed_with(&gpu), on_cpu);
        assert!(!gpu.is_lost());
        gpu.set_fault(None);
        assert_eq!(
            compressed_with(&gpu),
            on_gpu,
            "a device not lost is used again"
        );

        gpu.set_fault(Some(true));
        assert_eq!(compressed_with(&gpu), on_cpu);
        assert!(gpu.is_lost());
        gpu.set_fault(None);
        assert_eq!(compressed_with(&gpu), on_cpu, "a lost device is not tried");
    }
}
