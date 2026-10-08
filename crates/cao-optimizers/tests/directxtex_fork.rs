//! Checks that the workspace builds Textures through CAO's `directxtex` fork (#479).
//!
//! The fork bumps DirectXTex to `may2026` so the Rust port and the C++ parity
//! oracle encode with the same library. It also makes `ScratchImage` `Send` so the
//! Run Worker can own one. `ba2` must link the same copy.

use directxtex::{
    CP_FLAGS_NONE, DDS_FLAGS_NONE, DXGI_FORMAT_BC7_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM, ScratchImage,
    TEX_COMPRESS_DEFAULT, TEX_FILTER_FLAGS, TEX_FILTER_FORCE_NON_WIC, TEX_FILTER_SEPARATE_ALPHA,
    TEX_THRESHOLD_DEFAULT, TGA_FLAGS_NONE,
};

/// Edge length of the synthetic texture. It is a power of two, so the full mip
/// chain is `log2(SIZE) + 1` levels. It is kept small because the CPU BC7
/// encoder is single-threaded, and in a dev build `cc` compiles DirectXTex
/// unoptimised.
const SIZE: usize = 32;

/// The number of levels in `SIZE`'s full mip chain.
const FULL_MIP_COUNT: usize = SIZE.ilog2() as usize + 1;

/// The decoded BC7 chain must reach this PSNR against its mipmapped source.
const MIN_BC7_PSNR_DB: f64 = 30.0;

/// CAO's mipmap filter. C++ passes `TEX_FILTER_SEPARATE_ALPHA` alone, which can
/// route through WIC and so needs COM. The test threads never call
/// `CoInitializeEx` (that belongs to the texture backend's worker, #494), so the
/// non-WIC path is forced here.
fn mip_filter() -> TEX_FILTER_FLAGS {
    TEX_FILTER_SEPARATE_ALPHA | TEX_FILTER_FORCE_NON_WIC
}

/// Builds a `SIZE`×`SIZE` RGBA8 texture with smooth gradients in every channel,
/// including alpha, so mipmapping and BC7 have real content to work on.
fn synthetic_texture() -> ScratchImage {
    let mut scratch = ScratchImage::default();
    scratch
        .initialize_2d(DXGI_FORMAT_R8G8B8A8_UNORM, SIZE, SIZE, 1, 1, CP_FLAGS_NONE)
        .expect("initialize a 2D RGBA8 texture");
    let row_pitch = scratch.images()[0].row_pitch;
    for (y, row) in scratch.pixels_mut().chunks_exact_mut(row_pitch).enumerate() {
        for (x, texel) in row.as_chunks_mut::<4>().0.iter_mut().take(SIZE).enumerate() {
            let (x, y) = (x as u8, y as u8);
            *texel = [x * 4, y * 4, (x + y) * 2, 255 - x * 2];
        }
    }
    scratch
}

/// Returns the peak signal-to-noise ratio between two equally sized 8-bit
/// buffers, in decibels. Identical buffers give infinity.
fn psnr(expected: &[u8], actual: &[u8]) -> f64 {
    assert_eq!(expected.len(), actual.len(), "buffers differ in size");
    let squared_error: f64 = expected
        .iter()
        .zip(actual)
        .map(|(&a, &b)| (f64::from(a) - f64::from(b)).powi(2))
        .sum();
    let mse = squared_error / expected.len() as f64;
    10.0 * (255.0 * 255.0 / mse).log10()
}

/// Mipmaps and CPU-compresses `scratch` to BC7 the way CAO does, returning the
/// mipmapped source and the compressed result.
fn mipmap_and_compress(scratch: &ScratchImage) -> (ScratchImage, ScratchImage) {
    let mipped = scratch
        .generate_mip_maps(mip_filter(), 0)
        .expect("generate the full mip chain");
    // CAO's C++ also ORs in TEX_FILTER_SEPARATE_ALPHA, which DirectXTex masks
    // off; the port passes TEX_COMPRESS_DEFAULT alone (#459).
    let compressed = mipped
        .compress(
            DXGI_FORMAT_BC7_UNORM,
            TEX_COMPRESS_DEFAULT,
            TEX_THRESHOLD_DEFAULT,
        )
        .expect("CPU-compress to BC7");
    (mipped, compressed)
}

/// Runs one synthetic texture through the DirectXTex calls CAO's texture path
/// makes: TGA and DDS load, full mip chain, CPU BC7 and a BC7 DDS round trip.
#[test]
fn loads_mipmaps_and_cpu_compresses_a_synthetic_texture() {
    let source = synthetic_texture();

    // Load it back through both readers CAO uses for Textures.
    let tga = source.images()[0]
        .save_tga(TGA_FLAGS_NONE, None)
        .expect("encode TGA");
    let from_tga =
        ScratchImage::load_tga(tga.buffer(), TGA_FLAGS_NONE, None).expect("load the TGA");
    assert_eq!(from_tga.metadata().format, DXGI_FORMAT_R8G8B8A8_UNORM);
    assert_eq!(from_tga.pixels(), source.pixels());

    let dds = source.save_dds(DDS_FLAGS_NONE).expect("encode DDS");
    let loaded =
        ScratchImage::load_dds(dds.buffer(), DDS_FLAGS_NONE, None, None).expect("load the DDS");
    assert_eq!(loaded.metadata(), source.metadata());
    assert_eq!(loaded.pixels(), source.pixels());

    let (mipped, compressed) = mipmap_and_compress(&loaded);
    assert_eq!(mipped.metadata().mip_levels, FULL_MIP_COUNT);

    let meta = compressed.metadata();
    assert_eq!(meta.format, DXGI_FORMAT_BC7_UNORM);
    assert!(meta.format.is_compressed());
    assert_eq!((meta.width, meta.height), (SIZE, SIZE));
    assert_eq!(meta.mip_levels, FULL_MIP_COUNT);
    assert_eq!(compressed.images().len(), FULL_MIP_COUNT);

    // BC7 is lossy, so decode and compare instead of matching bytes. This is a
    // sanity floor against the source, not the spec's parity threshold (#467),
    // which compares two encodes of the same source. The gradients here put
    // each 4×4 block's colours on a plane, which BC7's per-subset line cannot fit
    // exactly, so a working encoder lands near 39.6 dB. A broken one (garbage
    // blocks, swapped channels, a misread mip) falls far below 30 dB.
    let decoded = compressed
        .decompress(DXGI_FORMAT_R8G8B8A8_UNORM)
        .expect("decode the BC7 chain");
    let quality = psnr(mipped.pixels(), decoded.pixels());
    assert!(
        quality >= MIN_BC7_PSNR_DB,
        "BC7 round trip only reached {quality:.1} dB"
    );

    // The compressed chain survives a DDS round trip.
    let written = compressed.save_dds(DDS_FLAGS_NONE).expect("encode BC7 DDS");
    let reread = ScratchImage::load_dds(written.buffer(), DDS_FLAGS_NONE, None, None)
        .expect("load the BC7 DDS");
    assert_eq!(reread.metadata(), compressed.metadata());
    assert_eq!(reread.pixels(), compressed.pixels());
}

/// Proves the fork's `Send` impl from the consumer side: this only compiles
/// when the patched `directxtex` is in use.
#[test]
fn scratch_image_moves_to_another_thread() {
    let source = synthetic_texture();
    let pixels = source.pixels().to_vec();

    // The worker takes ownership, transforms the texture and hands a new one
    // back, so the image crosses threads in both directions and the original
    // allocation is freed on the worker.
    let compressed = std::thread::spawn(move || {
        assert_eq!(source.pixels(), pixels.as_slice());
        mipmap_and_compress(&source).1
    })
    .join()
    .expect("the worker thread panicked");

    assert_eq!(compressed.metadata().format, DXGI_FORMAT_BC7_UNORM);
    assert_eq!(compressed.metadata().mip_levels, FULL_MIP_COUNT);
}

/// The fork's git URL, as `Cargo.lock` records it in a package `source`.
const FORK_SOURCE: &str = "git+https://github.com/evildarkarchon/directxtex-rs?rev=";

/// Splits `Cargo.lock` into its `[[package]]` tables. Each table is returned as
/// its raw text. Only the line-oriented subset that Cargo writes is handled.
fn lock_packages(lock: &str) -> Vec<&str> {
    lock.split("[[package]]").skip(1).collect()
}

/// Returns a package table's quoted `key = "value"` field, if present.
fn lock_field<'a>(package: &'a str, key: &str) -> Option<&'a str> {
    package.lines().find_map(|line| {
        let value = line.strip_prefix(key)?.trim_start().strip_prefix('=')?;
        value.trim().strip_prefix('"')?.strip_suffix('"')
    })
}

/// Returns the entries of a package table's `dependencies = [...]` list, or an
/// empty list when the table has none.
fn lock_dependencies(package: &str) -> Vec<&str> {
    let Some((_, list)) = package.split_once("dependencies = [") else {
        return Vec::new();
    };
    let (list, _) = list
        .split_once(']')
        .expect("an unterminated dependencies list");
    list.split(',')
        .map(|entry| entry.trim().trim_matches('"'))
        .filter(|entry| !entry.is_empty())
        .collect()
}

/// Guards the `[patch.crates-io]` entry. Cargo only warns "patch was not used"
/// when a patch stops applying, for example after a locked version drifts from
/// the fork's. That would put a second, crates.io `directxtex` in the graph, and
/// this test fails instead.
#[test]
fn workspace_resolves_one_directxtex_from_the_fork() {
    let lock_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock");
    let lock = std::fs::read_to_string(lock_path).expect("read the workspace Cargo.lock");
    let packages = lock_packages(&lock);

    let directxtex: Vec<_> = packages
        .iter()
        .filter(|package| lock_field(package, "name") == Some("directxtex"))
        .collect();
    assert_eq!(
        directxtex.len(),
        1,
        "expected exactly one directxtex package"
    );
    let source = lock_field(directxtex[0], "source").expect("directxtex has a source");
    assert!(
        source.starts_with(FORK_SOURCE),
        "directxtex resolves from {source}, not CAO's fork"
    );

    // With a single copy in the graph, Cargo lists the dependency by bare name.
    // A second copy would make it qualify the entry with a version instead.
    for dependent in ["ba2", "cao-optimizers"] {
        let package = packages
            .iter()
            .find(|package| lock_field(package, "name") == Some(dependent))
            .unwrap_or_else(|| panic!("{dependent} is not in Cargo.lock"));
        assert!(
            lock_dependencies(package).contains(&"directxtex"),
            "{dependent} does not depend on the single directxtex"
        );
    }
}
