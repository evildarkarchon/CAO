//! GPU BC7 and BC6H encoding with a CPU fallback (#495).
//!
//! C++ `TexturesOptimizer` creates a D3D11 device on the first adapter and
//! encodes BC6H and BC7 with DirectXTex's DirectCompute encoder when it has one,
//! and with the CPU encoder when it does not. These scenarios need a Windows host
//! with a D3D11 device on adapter 0: a GPU, or the Basic Render Driver that
//! Windows provides without one.

mod common;

use std::path::{Path, PathBuf};

use cao_core::routing::TextureVariant;
use cao_optimizers::device::{DeviceUnavailable, GpuDevice};
use cao_optimizers::textures::{Texture, TextureProfile, TextureRequest};
use common::{scratch_dir, write_dds};
use directxtex::{
    CP_FLAGS_NONE, DXGI_FORMAT, DXGI_FORMAT_BC1_UNORM, DXGI_FORMAT_BC6H_UF16,
    DXGI_FORMAT_BC7_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R32G32B32A32_FLOAT,
    ScratchImage, TEX_COMPRESS_DEFAULT, TEX_THRESHOLD_DEFAULT,
};

/// An adapter index no host has: `createDevice` fails on it, which is how
/// these scenarios force the CPU fallback.
const MISSING_ADAPTER: u32 = u32::MAX;

/// A decoded GPU encoding must reach this PSNR against its source. BC7 and BC6H
/// on the synthetic gradients below reach far more; this only catches garbage.
const MIN_PSNR_DB: f64 = 30.0;

/// A `size`×`size` single-mip Texture in `format`, filled with smooth
/// gradients in every channel, converted from RGBA8.
fn gradient(format: DXGI_FORMAT, size: usize) -> ScratchImage {
    let mut scratch = ScratchImage::default();
    scratch
        .initialize_2d(DXGI_FORMAT_R8G8B8A8_UNORM, size, size, 1, 1, CP_FLAGS_NONE)
        .unwrap();
    let row_pitch = scratch.images()[0].row_pitch;
    for (y, row) in scratch.pixels_mut().chunks_exact_mut(row_pitch).enumerate() {
        for (x, texel) in row.as_chunks_mut::<4>().0.iter_mut().take(size).enumerate() {
            let (x, y) = ((x * 255 / size) as u8, (y * 255 / size) as u8);
            *texel = [x, y, x / 2 + y / 2, 255 - x];
        }
    }
    if format == DXGI_FORMAT_R8G8B8A8_UNORM {
        return scratch;
    }
    scratch
        .convert(
            format,
            directxtex::TEX_FILTER_DEFAULT,
            TEX_THRESHOLD_DEFAULT,
        )
        .unwrap()
}

/// The PSNR, in decibels, of `encoded` decoded against `source`, both compared
/// as 32-bit floats so BC6H's half floats and BC7's bytes share one scale, with
/// a peak of 1.
fn psnr(source: &ScratchImage, encoded: &ScratchImage) -> f64 {
    let float = DXGI_FORMAT_R32G32B32A32_FLOAT;
    let decoded = encoded.decompress(float).unwrap();
    let source = if source.metadata().format == float {
        source.pixels().to_vec()
    } else {
        source
            .convert(float, directxtex::TEX_FILTER_DEFAULT, TEX_THRESHOLD_DEFAULT)
            .unwrap()
            .pixels()
            .to_vec()
    };
    let as_floats = |bytes: &[u8]| -> Vec<f32> {
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|chunk| f32::from_le_bytes(*chunk))
            .collect()
    };
    let (expected, actual) = (as_floats(&source), as_floats(decoded.pixels()));
    assert_eq!(expected.len(), actual.len());
    // BC6H stores no alpha and decodes it as 1, so only colour is compared.
    let channels = if encoded.metadata().format == DXGI_FORMAT_BC6H_UF16 {
        3
    } else {
        4
    };
    let squared: f64 = expected
        .iter()
        .zip(&actual)
        .enumerate()
        .filter(|(index, _)| index % 4 < channels)
        .map(|(_, (&a, &b))| (f64::from(a) - f64::from(b)).powi(2))
        .sum();
    let mse = squared / (expected.len() / 4 * channels) as f64;
    10.0 * (1.0 / mse).log10()
}

/// The SSE profile's Texture settings: BC7, with interface Textures compressed.
fn sse_profile() -> TextureProfile {
    TextureProfile {
        format: DXGI_FORMAT_BC7_UNORM,
        unwanted_formats: Vec::new(),
        compress_interface: true,
    }
}

/// Necessary optimization and compression only: no resize and no mipmaps, so
/// the only DirectXTex step is the compression under test.
fn compress_only() -> TextureRequest {
    TextureRequest {
        necessary: true,
        compress: true,
        mipmaps: false,
        target: None,
    }
}

/// Writes a fresh 64×64 RGBA8 `plain.dds` in a scratch directory named `name`.
fn plain_texture(name: &str) -> PathBuf {
    let path = scratch_dir(name).join("plain.dds");
    write_dds(&path, DXGI_FORMAT_R8G8B8A8_UNORM, 64);
    path
}

/// The DDS at `path` encoded to `format` by the CPU encoder, with the flags
/// CAO passes it: the reference a CPU-encoded Texture must match.
fn cpu_encoded(path: &Path, format: DXGI_FORMAT) -> ScratchImage {
    Texture::load(path, TextureVariant::Native)
        .unwrap()
        .image()
        .compress(format, TEX_COMPRESS_DEFAULT, TEX_THRESHOLD_DEFAULT)
        .unwrap()
}

/// Loads the DDS at `path`, optimizes it with `gpu`, and returns its pixels.
fn optimized_pixels(path: &Path, gpu: Option<&GpuDevice>) -> (DXGI_FORMAT, Vec<u8>) {
    let mut texture = Texture::load(path, TextureVariant::Native).unwrap();
    assert!(
        texture
            .optimize(&sse_profile(), &compress_only(), gpu)
            .unwrap()
    );
    let format = texture.metadata().format;
    (format, texture.image().pixels().to_vec())
}

/// `createDevice(0)`: the first adapter gives a device.
#[test]
fn the_first_adapter_gives_a_device() {
    GpuDevice::create(0).unwrap();
}

/// `createDevice` fails on an adapter index the host does not have, as C++
/// logs "Invalid GPU adapter index".
#[test]
fn an_adapter_the_host_does_not_have_gives_no_device() {
    let error = GpuDevice::create(MISSING_ADAPTER).err().unwrap();
    assert!(
        matches!(error, DeviceUnavailable::NoAdapter { adapter, .. } if adapter == MISSING_ADAPTER),
        "{error}"
    );
}

/// The DirectCompute encoder produces BC7 that decodes close to its source.
/// That it is a different encoder from the CPU one is pinned by
/// `a_texture_with_a_device_is_compressed_to_bc7_on_the_gpu`.
#[test]
fn bc7_is_encoded_on_the_gpu() {
    let gpu = GpuDevice::create(0).unwrap();
    let source = gradient(DXGI_FORMAT_R8G8B8A8_UNORM, 64);

    let encoded = gpu.compress(&source, DXGI_FORMAT_BC7_UNORM).unwrap();

    assert_eq!(encoded.metadata().format, DXGI_FORMAT_BC7_UNORM);
    let quality = psnr(&source, &encoded);
    assert!(
        quality >= MIN_PSNR_DB,
        "GPU BC7 reached only {quality:.1} dB"
    );
}

/// BC6H, from a float source, goes through the same DirectCompute encoder.
#[test]
fn bc6h_is_encoded_on_the_gpu() {
    let gpu = GpuDevice::create(0).unwrap();
    let source = gradient(DXGI_FORMAT_R32G32B32A32_FLOAT, 64);

    let encoded = gpu.compress(&source, DXGI_FORMAT_BC6H_UF16).unwrap();

    assert_eq!(encoded.metadata().format, DXGI_FORMAT_BC6H_UF16);
    let quality = psnr(&source, &encoded);
    assert!(
        quality >= MIN_PSNR_DB,
        "GPU BC6H reached only {quality:.1} dB"
    );
}

/// The DirectCompute encoder handles only BC6H and BC7; DirectXTex refuses
/// any other target rather than encoding it.
#[test]
fn the_gpu_encoder_refuses_other_block_formats() {
    let gpu = GpuDevice::create(0).unwrap();
    let source = gradient(DXGI_FORMAT_R8G8B8A8_UNORM, 16);

    assert!(gpu.compress(&source, DXGI_FORMAT_BC1_UNORM).is_err());
}

/// A Texture compressed to BC7 with a device gets exactly the DirectCompute
/// encoder's bytes, not the CPU encoder's.
#[test]
fn a_texture_with_a_device_is_compressed_to_bc7_on_the_gpu() {
    let path = plain_texture("gpu-bc7-texture");
    let gpu = GpuDevice::create(0).unwrap();
    let source = Texture::load(&path, TextureVariant::Native).unwrap();
    let on_gpu = gpu.compress(source.image(), DXGI_FORMAT_BC7_UNORM).unwrap();
    let on_cpu = cpu_encoded(&path, DXGI_FORMAT_BC7_UNORM);
    // C++ asks the GPU encoder for three-subset modes, which the CPU encoder
    // only tries when told to, so the two encoders' bytes differ.
    assert_ne!(on_gpu.pixels(), on_cpu.pixels(), "the encoders must differ");

    let (format, pixels) = optimized_pixels(&path, Some(&gpu));

    assert_eq!(format, DXGI_FORMAT_BC7_UNORM);
    assert_eq!(pixels, on_gpu.pixels());
}

/// With device creation forced to fail, BC7 falls back to the CPU encoder, as
/// C++ does, and the Texture gets exactly its bytes. On the oracle the same
/// fallback is forced with `CAO_ORACLE_FORCE_CPU_BC`, and the two CPU encoders
/// are byte-identical (#494).
#[test]
fn a_texture_without_a_device_is_compressed_to_bc7_on_the_cpu() {
    let path = plain_texture("cpu-bc7-texture");
    let gpu = GpuDevice::create(MISSING_ADAPTER).ok();
    assert!(gpu.is_none());
    let on_cpu = cpu_encoded(&path, DXGI_FORMAT_BC7_UNORM);

    let (format, pixels) = optimized_pixels(&path, gpu.as_ref());

    assert_eq!(format, DXGI_FORMAT_BC7_UNORM);
    assert_eq!(pixels, on_cpu.pixels());
}

/// Formats other than BC6H and BC7 stay on the CPU even with a device, as in
/// C++, so their output stays byte-identical to the oracle's.
#[test]
fn other_block_formats_stay_on_the_cpu_with_a_device() {
    let path = plain_texture("gpu-bc1-texture");
    let gpu = GpuDevice::create(0).unwrap();
    let on_cpu = cpu_encoded(&path, DXGI_FORMAT_BC1_UNORM);
    let mut texture = Texture::load(&path, TextureVariant::Native).unwrap();
    let profile = TextureProfile {
        format: DXGI_FORMAT_BC1_UNORM,
        ..sse_profile()
    };

    assert!(
        texture
            .optimize(&profile, &compress_only(), Some(&gpu))
            .unwrap()
    );

    assert_eq!(texture.metadata().format, DXGI_FORMAT_BC1_UNORM);
    assert_eq!(texture.image().pixels(), on_cpu.pixels());
}
