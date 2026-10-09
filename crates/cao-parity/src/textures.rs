//! The Texture rule of the output-tree comparator (#467, #494).
//!
//! Two DDS files whose bytes differ are Equivalent when:
//!
//! - their DDS headers are byte-identical: the magic, `DDS_HEADER`, and the
//!   DX10 extension when there is one. With one DirectXTex on both sides, this
//!   covers format, size, mip count, array size, cubemap and alpha flags, and
//!   the choice of header;
//! - their pixel data is byte-identical, except for BC7 and BC6H. There, every
//!   image (each mip of each array item or cubemap face, and each slice of a
//!   volume) must reach [`MIN_BC7_BC6H_PSNR_DB`] after decoding.
//!
//! The BC7 and BC6H tolerance exists because C++'s own output already depends
//! on the host: it encodes them on the GPU when it can (with
//! `TEX_COMPRESS_BC7_USE_3SUBSETS`) and on the CPU otherwise, which are
//! different encoders.
//!
//! PSNR is measured over pixels decoded to `R32G32B32A32_FLOAT`, in Rust,
//! because the `directxtex` crate's `compute_mse` result has no accessors
//! (#479). Colour uses DirectXTex `texdiag compare`'s per-image figure,
//! `10·log10(3 / (MSE_R + MSE_G + MSE_B))`; alpha must reach the same bar on
//! its own, so that damaged alpha cannot hide behind good colour and opaque
//! alpha cannot raise the colour figure. The threshold calibration (#513)
//! must measure with [`psnr_per_image`] too, so its floor means what this rule
//! applies.

use std::cmp::Ordering;
use std::path::Path;

use directxtex::{
    DDS_FLAGS_NONE, DXGI_FORMAT, HResultError, ScratchImage, TEX_DIMENSION_TEXTURE3D,
    TEX_FILTER_DEFAULT, TEX_THRESHOLD_DEFAULT,
};

use crate::HarnessError;
use crate::case::read_file;
use crate::tree::{ArtifactRule, RuleOutcome};

/// The PSNR every decoded BC7 or BC6H image must reach, in decibels.
///
/// Provisional: #513 calibrates it once by running the oracle against itself,
/// GPU against forced CPU, and lowers it to the observed floor minus 1 dB if
/// that floor is below 40 dB.
pub const MIN_BC7_BC6H_PSNR_DB: f64 = 40.0;

/// The format PSNR is measured in.
const FLOAT: DXGI_FORMAT = DXGI_FORMAT::DXGI_FORMAT_R32G32B32A32_FLOAT;

/// The comparator rule for `.dds` files.
pub struct TextureRule;

impl ArtifactRule for TextureRule {
    fn name(&self) -> &'static str {
        "Texture Header and Pixels"
    }

    fn compare(&self, oracle: &Path, rust: &Path) -> Result<RuleOutcome, HarnessError> {
        Ok(compare_dds(&read_file(oracle)?, &read_file(rust)?))
    }
}

/// Compares two DDS files under the Texture rule.
///
/// Also meant for DDS entries inside Archives, which the archive rule compares
/// with the Texture rule once `ba2` has rebuilt them.
pub fn compare_dds(oracle: &[u8], rust: &[u8]) -> RuleOutcome {
    match try_compare_dds(oracle, rust) {
        Ok(()) => RuleOutcome::Equivalent,
        Err(detail) => RuleOutcome::Different(detail),
    }
}

/// [`compare_dds`], with every broken part of the rule as an `Err`.
fn try_compare_dds(oracle: &[u8], rust: &[u8]) -> Result<(), String> {
    let oracle_header = header_len(oracle).map_err(|error| format!("oracle side: {error}"))?;
    let rust_header = header_len(rust).map_err(|error| format!("Rust side: {error}"))?;
    let (oracle_head, rust_head) = (&oracle[..oracle_header], &rust[..rust_header]);
    if oracle_head != rust_head {
        return Err(format!(
            "the DDS headers differ{}",
            first_difference(oracle_head, rust_head)
        ));
    }

    let oracle_image = load(oracle).map_err(|error| format!("oracle side: {error}"))?;
    let rust_image = load(rust).map_err(|error| format!("Rust side: {error}"))?;
    let format = oracle_image.metadata().format;
    if !is_bc7_or_bc6h(format) {
        return Err(format!(
            "the {format:?} pixel data differs{}; only BC7 and BC6H may differ",
            first_difference(&oracle[oracle_header..], &rust[rust_header..])
        ));
    }

    let decode = |side: &str, image: &ScratchImage| {
        decode_to_float(image).map_err(|error| format!("{side} side: cannot decode: {error}"))
    };
    let (oracle_float, rust_float) = (
        decode("oracle", &oracle_image)?,
        decode("Rust", &rust_image)?,
    );
    for image in psnr_per_image(&oracle_float, &rust_float)? {
        // A NaN from a broken decode is unordered, and fails too.
        if matches!(
            image.db.partial_cmp(&MIN_BC7_BC6H_PSNR_DB),
            None | Some(Ordering::Less)
        ) {
            return Err(format!(
                "{} reaches {:.2} dB; {format:?} needs {MIN_BC7_BC6H_PSNR_DB} dB in every mip \
                 and face",
                image.label(),
                image.db
            ));
        }
    }
    Ok(())
}

/// The length of a DDS file's header: the magic and `DDS_HEADER`, plus the
/// DX10 extension when the pixel format's FourCC asks for one.
fn header_len(bytes: &[u8]) -> Result<usize, String> {
    const MAGIC: &[u8] = b"DDS ";
    // The magic, then `DDS_HEADER`'s 124 bytes.
    const LEGACY: usize = 4 + 124;
    // `DDS_HEADER_DXT10`.
    const DX10_EXTENSION: usize = 20;
    // `ddspf.dwFlags` and `ddspf.dwFourCC`, counted from the file's start.
    const PIXEL_FORMAT_FLAGS: usize = 4 + 76;
    const FOUR_CC: usize = 4 + 80;
    const DDPF_FOURCC: u32 = 0x4;

    if bytes.len() < LEGACY || &bytes[..4] != MAGIC {
        return Err("not a DDS file".to_owned());
    }
    let flags = u32::from_le_bytes(
        bytes[PIXEL_FORMAT_FLAGS..PIXEL_FORMAT_FLAGS + 4]
            .try_into()
            .expect("four bytes"),
    );
    let dx10 = flags & DDPF_FOURCC != 0 && &bytes[FOUR_CC..FOUR_CC + 4] == b"DX10";
    let length = if dx10 {
        LEGACY + DX10_EXTENSION
    } else {
        LEGACY
    };
    if bytes.len() < length {
        return Err("the DDS file ends inside its header".to_owned());
    }
    Ok(length)
}

/// Loads a DDS file, reading a typeless format as its UNORM equivalent so it
/// can be decoded.
fn load(bytes: &[u8]) -> Result<ScratchImage, String> {
    let mut image = ScratchImage::load_dds(bytes, DDS_FLAGS_NONE, None, None)
        .map_err(|error| format!("DirectXTex cannot load the DDS file: {error}"))?;
    let format = image.metadata().format;
    if format.is_typeless(true) {
        // Only fails when the bits per pixel differ, which they never do here.
        let _ = image.override_format(format.make_typeless_unorm());
    }
    Ok(image)
}

/// Whether `format` is one of the formats the PSNR rule applies to.
fn is_bc7_or_bc6h(format: DXGI_FORMAT) -> bool {
    matches!(
        format,
        DXGI_FORMAT::DXGI_FORMAT_BC7_TYPELESS
            | DXGI_FORMAT::DXGI_FORMAT_BC7_UNORM
            | DXGI_FORMAT::DXGI_FORMAT_BC7_UNORM_SRGB
            | DXGI_FORMAT::DXGI_FORMAT_BC6H_TYPELESS
            | DXGI_FORMAT::DXGI_FORMAT_BC6H_UF16
            | DXGI_FORMAT::DXGI_FORMAT_BC6H_SF16
    )
}

/// Describes where two byte strings first differ, as a suffix for a message.
fn first_difference(oracle: &[u8], rust: &[u8]) -> String {
    match oracle.iter().zip(rust).position(|(a, b)| a != b) {
        Some(index) => format!(" first at byte {index}"),
        None if oracle.len() != rust.len() => {
            format!(" in length: {} bytes against {}", oracle.len(), rust.len())
        }
        None => String::new(),
    }
}

/// Decodes `image` to `R32G32B32A32_FLOAT`: decompressing a block format,
/// converting any other.
///
/// # Errors
/// The DirectXTex error when the format cannot be decoded.
pub fn decode_to_float(image: &ScratchImage) -> Result<ScratchImage, HResultError> {
    if image.metadata().format.is_compressed() {
        image.decompress(FLOAT)
    } else {
        image.convert(FLOAT, TEX_FILTER_DEFAULT, TEX_THRESHOLD_DEFAULT)
    }
}

/// The PSNR of one image of a texture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImagePsnr {
    pub mip: usize,
    /// The array item, or the face for a cubemap (six per cube).
    pub item: usize,
    /// The depth slice; always 0 except in a volume texture.
    pub slice: usize,
    /// The lower of the colour and alpha PSNR, in decibels; infinite when
    /// the images are equal.
    pub db: f64,
}

impl ImagePsnr {
    /// Names the image for a report, such as `mip 2 of item 3`.
    pub fn label(&self) -> String {
        let mut label = format!("mip {} of item {}", self.mip, self.item);
        if self.slice != 0 {
            label.push_str(&format!(", slice {}", self.slice));
        }
        label
    }
}

/// The PSNR of every image of two decoded textures with the same layout, in
/// DirectXTex's image order.
///
/// # Errors
/// A description when either texture is not `R32G32B32A32_FLOAT` or their
/// layouts differ.
pub fn psnr_per_image(
    oracle: &ScratchImage,
    rust: &ScratchImage,
) -> Result<Vec<ImagePsnr>, String> {
    let info = *oracle.metadata();
    let rust_info = rust.metadata();
    if info.format != FLOAT || rust_info.format != FLOAT {
        return Err("PSNR is measured over R32G32B32A32_FLOAT images".to_owned());
    }
    let layout = |info: &directxtex::TexMetadata| {
        (
            info.width,
            info.height,
            info.depth,
            info.array_size,
            info.mip_levels,
            info.dimension,
        )
    };
    if layout(&info) != layout(rust_info) {
        return Err(format!(
            "the decoded textures have different layouts: {info:?} against {rust_info:?}"
        ));
    }

    let mut results = Vec::new();
    for item in 0..info.array_size {
        for mip in 0..info.mip_levels {
            let slices = if info.dimension == TEX_DIMENSION_TEXTURE3D {
                (info.depth >> mip).max(1)
            } else {
                1
            };
            for slice in 0..slices {
                let index = info
                    .compute_index(mip, item, slice)
                    .ok_or_else(|| format!("mip {mip} of item {item} has no image"))?;
                let db = image_psnr(oracle, rust, index);
                results.push(ImagePsnr {
                    mip,
                    item,
                    slice,
                    db,
                });
            }
        }
    }
    Ok(results)
}

/// The PSNR of the image at `index` in two decoded textures of one layout:
/// the lower of the colour figure and the alpha figure.
fn image_psnr(oracle: &ScratchImage, rust: &ScratchImage, index: usize) -> f64 {
    let (oracle_texels, rust_texels) = (texels(oracle, index), texels(rust, index));
    let mut squared = [0.0f64; 4];
    let mut count = 0usize;
    for (a, b) in oracle_texels.zip(rust_texels) {
        for channel in 0..4 {
            squared[channel] += (f64::from(a[channel]) - f64::from(b[channel])).powi(2);
        }
        count += 1;
    }
    let mse = squared.map(|sum| sum / count.max(1) as f64);
    let colour = psnr(3.0, mse[0] + mse[1] + mse[2]);
    let alpha = psnr(1.0, mse[3]);
    colour.min(alpha)
}

/// `10·log10(peak / mse)`, infinite when nothing differs.
fn psnr(peak: f64, mse: f64) -> f64 {
    if mse == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (peak / mse).log10()
    }
}

/// The RGBA texels of the `R32G32B32A32_FLOAT` image at `index`, row by row.
///
/// `ScratchImage` exposes each image as a pointer into its one pixel buffer,
/// so the image is found by its offset in that buffer; no `unsafe` is needed.
fn texels(texture: &ScratchImage, index: usize) -> impl Iterator<Item = [f32; 4]> + '_ {
    const TEXEL: usize = 16;
    let image = &texture.images()[index];
    let buffer = texture.pixels();
    let start = image.pixels as usize - buffer.as_ptr() as usize;
    let (width, row_pitch) = (image.width, image.row_pitch);
    (0..image.height).flat_map(move |row| {
        let offset = start + row * row_pitch;
        buffer[offset..offset + width * TEXEL]
            .as_chunks::<TEXEL>()
            .0
            .iter()
            .map(|texel| {
                let channel = |at: usize| {
                    f32::from_le_bytes(texel[at..at + 4].try_into().expect("four bytes"))
                };
                [channel(0), channel(4), channel(8), channel(12)]
            })
    })
}
