//! The comparator's Texture rule (#467, #494): byte-identical DDS headers,
//! byte-identical pixels, and a per-image PSNR floor for BC7 and BC6H only.

mod common;

use cao_parity::compare::Verdict;
use cao_parity::rules::ParityRules;
use cao_parity::textures::{MIN_BC7_BC6H_PSNR_DB, TextureRule, compare_dds, psnr_per_image};
use cao_parity::tree::{ArtifactRule, ArtifactVerdict, RuleOutcome, TreeSide, compare_trees};
use common::{TempDir, write};
use directxtex::{
    CP_FLAGS_NONE, DDS_FLAGS_FORCE_DX10_EXT, DDS_FLAGS_NONE, DXGI_FORMAT, ScratchImage,
    TEX_COMPRESS_BC7_QUICK, TEX_COMPRESS_DEFAULT, TEX_COMPRESS_FLAGS, TEX_FILTER_FORCE_NON_WIC,
    TEX_THRESHOLD_DEFAULT,
};

const BC7: DXGI_FORMAT = DXGI_FORMAT::DXGI_FORMAT_BC7_UNORM;
const BC6H: DXGI_FORMAT = DXGI_FORMAT::DXGI_FORMAT_BC6H_UF16;
const BC3: DXGI_FORMAT = DXGI_FORMAT::DXGI_FORMAT_BC3_UNORM;
const RGBA8: DXGI_FORMAT = DXGI_FORMAT::DXGI_FORMAT_R8G8B8A8_UNORM;
const FLOAT: DXGI_FORMAT = DXGI_FORMAT::DXGI_FORMAT_R32G32B32A32_FLOAT;

/// A `size`×`size` RGBA8 texture (six faces for a cubemap) with its full mip
/// chain, each texel from `texel(x, y, item)`.
///
/// The mip chain is generated without WIC: test threads never join COM.
fn source(size: usize, cube: bool, texel: impl Fn(usize, usize, usize) -> [u8; 4]) -> ScratchImage {
    let mut scratch = ScratchImage::default();
    if cube {
        scratch.initialize_cube(RGBA8, size, size, 1, 1, CP_FLAGS_NONE)
    } else {
        scratch.initialize_2d(RGBA8, size, size, 1, 1, CP_FLAGS_NONE)
    }
    .unwrap();
    let items = scratch.metadata().array_size;
    let (row_pitch, slice_pitch) = (
        scratch.images()[0].row_pitch,
        scratch.images()[0].slice_pitch,
    );
    for item in 0..items {
        let slice = &mut scratch.pixels_mut()[item * slice_pitch..(item + 1) * slice_pitch];
        for (y, row) in slice.chunks_exact_mut(row_pitch).enumerate() {
            for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().take(size).enumerate() {
                pixel.copy_from_slice(&texel(x, y, item));
            }
        }
    }
    scratch
        .generate_mip_maps(TEX_FILTER_FORCE_NON_WIC, 0)
        .unwrap()
}

/// Colour that changes along one axis only, with an alpha ramp: each 4×4
/// block's colours lie on a line, which BC7 fits closely.
fn smooth(x: usize, _y: usize, item: usize) -> [u8; 4] {
    let x = x as u8;
    [x * 4, x * 2 + item as u8 * 20, 255 - x * 4, 128 + x * 2]
}

fn compress(source: &ScratchImage, format: DXGI_FORMAT, flags: TEX_COMPRESS_FLAGS) -> ScratchImage {
    source
        .compress(format, flags, TEX_THRESHOLD_DEFAULT)
        .unwrap()
}

fn dds(image: &ScratchImage) -> Vec<u8> {
    image.save_dds(DDS_FLAGS_NONE).unwrap().buffer().to_vec()
}

/// XORs every byte of one image (`mip` of `item`), as a broken encoder might
/// garble one level.
fn damage(image: &mut ScratchImage, mip: usize, item: usize) {
    let index = image.metadata().compute_index(mip, item, 0).unwrap();
    let target = &image.images()[index];
    let start = target.pixels as usize - image.pixels().as_ptr() as usize;
    let length = target.slice_pitch;
    for byte in &mut image.pixels_mut()[start..start + length] {
        *byte ^= 0xA5;
    }
}

fn different(outcome: RuleOutcome) -> String {
    match outcome {
        RuleOutcome::Different(detail) => detail,
        RuleOutcome::Equivalent => panic!("expected Different, got Equivalent"),
    }
}

/// The rule's reason to exist: C++ encodes BC7 on the GPU or the CPU, which
/// are different encoders. Two encodes of one source are Equivalent.
#[test]
fn bc7_encodes_of_one_source_by_different_encoders_are_equivalent() {
    let source = source(32, false, smooth);
    let thorough = dds(&compress(&source, BC7, TEX_COMPRESS_DEFAULT));
    let quick = dds(&compress(&source, BC7, TEX_COMPRESS_BC7_QUICK));
    assert_ne!(thorough, quick, "the encoders must disagree for this test");

    assert_eq!(compare_dds(&thorough, &quick), RuleOutcome::Equivalent);
}

/// The PSNR failure: the floor applies to every mip on its own, so one
/// garbled level fails even though the whole texture is nearly all fine.
#[test]
fn a_bc7_texture_with_one_damaged_mip_is_different() {
    let source = source(64, false, smooth);
    let good = compress(&source, BC7, TEX_COMPRESS_DEFAULT);
    let mut bad = compress(&source, BC7, TEX_COMPRESS_DEFAULT);
    damage(&mut bad, 2, 0);

    let detail = different(compare_dds(&dds(&good), &dds(&bad)));

    assert!(detail.starts_with("mip 2 of item 0 reaches "), "{detail}");
    assert!(
        detail.contains(&format!("needs {MIN_BC7_BC6H_PSNR_DB} dB")),
        "{detail}"
    );
}

/// Each face of a cubemap is held to the floor on its own.
#[test]
fn a_bc7_cubemap_with_one_damaged_face_is_different() {
    let source = source(16, true, smooth);
    let good = compress(&source, BC7, TEX_COMPRESS_DEFAULT);
    let mut bad = compress(&source, BC7, TEX_COMPRESS_DEFAULT);
    damage(&mut bad, 0, 3);

    let detail = different(compare_dds(&dds(&good), &dds(&bad)));

    assert!(detail.starts_with("mip 0 of item 3 reaches "), "{detail}");
}

/// BC6H follows the PSNR rule too: a close encode with other bytes passes,
/// and a garbled one fails.
#[test]
fn bc6h_is_held_to_the_psnr_floor() {
    let base = source(16, false, smooth);
    let first = compress(&base, BC6H, TEX_COMPRESS_DEFAULT);
    // Re-encoding the decoded first generation gives other bytes for nearly
    // the same pixels. (Encoding a source nudged by one level in a few
    // texels moved the CPU encoder's output to about 31 dB, so BC6H is
    // sensitive to its input; calibration, #513, measures the real spread.)
    let second = compress(
        &first.decompress(FLOAT).unwrap(),
        BC6H,
        TEX_COMPRESS_DEFAULT,
    );
    let (first_bytes, second_bytes) = (dds(&first), dds(&second));
    assert_ne!(
        first_bytes, second_bytes,
        "the encodes must differ for this test"
    );
    assert_eq!(
        compare_dds(&first_bytes, &second_bytes),
        RuleOutcome::Equivalent
    );

    let mut bad = compress(&base, BC6H, TEX_COMPRESS_DEFAULT);
    damage(&mut bad, 0, 0);
    let detail = different(compare_dds(&first_bytes, &dds(&bad)));
    assert!(detail.starts_with("mip 0 of item 0 reaches "), "{detail}");
}

/// Every format except BC7 and BC6H must match byte for byte, however close
/// the pixels are.
#[test]
fn other_formats_must_match_byte_for_byte() {
    let source = source(16, false, smooth);
    for image in [compress(&source, BC3, TEX_COMPRESS_DEFAULT), source] {
        let original = dds(&image);
        let mut nudged = original.clone();
        let last = nudged.len() - 1;
        nudged[last] ^= 1;

        let detail = different(compare_dds(&original, &nudged));

        assert!(
            detail.contains("pixel data differs first at byte"),
            "{detail}"
        );
        assert!(detail.ends_with("only BC7 and BC6H may differ"), "{detail}");
    }
}

/// The header covers format, size, mips, arrays and flags, and the choice
/// between a legacy and a DX10 header; any difference in it is Different,
/// even for BC7.
#[test]
fn differing_headers_are_different() {
    let source = source(16, false, smooth);
    // BC7 always needs the DX10 header, so the header choice shows on BC3.
    let bc3 = compress(&source, BC3, TEX_COMPRESS_DEFAULT);
    let legacy = dds(&bc3);
    let dx10 = bc3
        .save_dds(DDS_FLAGS_FORCE_DX10_EXT)
        .unwrap()
        .buffer()
        .to_vec();
    let detail = different(compare_dds(&legacy, &dx10));
    assert!(detail.starts_with("the DDS headers differ"), "{detail}");

    let full_chain = dds(&compress(&source, BC7, TEX_COMPRESS_DEFAULT));
    let top_only = source.image(0, 0, 0).unwrap();
    let mut single = ScratchImage::default();
    single
        .initialize_from_image(top_only, false, CP_FLAGS_NONE)
        .unwrap();
    let single = dds(&compress(&single, BC7, TEX_COMPRESS_DEFAULT));
    let detail = different(compare_dds(&full_chain, &single));
    assert!(detail.starts_with("the DDS headers differ"), "{detail}");
}

#[test]
fn a_file_that_is_not_a_dds_is_different() {
    let real = dds(&source(4, false, smooth));
    let detail = different(compare_dds(&real, b"not a texture"));
    assert_eq!(detail, "Rust side: not a DDS file");
}

/// A `R32G32B32A32_FLOAT` texture whose every texel is `texel`.
fn flat(texel: [f32; 4]) -> ScratchImage {
    let mut scratch = ScratchImage::default();
    scratch
        .initialize_2d(FLOAT, 8, 8, 1, 1, CP_FLAGS_NONE)
        .unwrap();
    for chunk in scratch.pixels_mut().as_chunks_mut::<16>().0 {
        for (channel, value) in chunk.as_chunks_mut::<4>().0.iter_mut().zip(texel) {
            channel.copy_from_slice(&value.to_le_bytes());
        }
    }
    scratch
}

/// Colour is DirectXTex `texdiag compare`'s `10·log10(3 / ΣMSE_rgb)`; alpha
/// is held to the same bar on its own, and the image gets the lower figure.
#[test]
fn psnr_follows_texdiag_for_colour_and_holds_alpha_on_its_own() {
    let base = flat([0.5, 0.5, 0.5, 1.0]);
    let psnr = |other: [f32; 4]| psnr_per_image(&base, &flat(other)).unwrap()[0].db;

    assert_eq!(psnr([0.5, 0.5, 0.5, 1.0]), f64::INFINITY);
    // Red alone off by 0.1: MSE_r = 0.01, so 10·log10(3 / 0.01).
    let red = psnr([0.6, 0.5, 0.5, 1.0]);
    assert!((red - 10.0 * 300f64.log10()).abs() < 1e-3, "{red}");
    // Alpha alone off by 0.01: 10·log10(1 / 0.0001) = 40 dB, not diluted by
    // the three exact colour channels.
    let alpha = psnr([0.5, 0.5, 0.5, 0.99]);
    assert!((alpha - 40.0).abs() < 1e-3, "{alpha}");
}

#[test]
fn psnr_needs_decoded_float_images() {
    let rgba = source(4, false, smooth);
    assert!(psnr_per_image(&rgba, &rgba).is_err());
}

/// Through the comparator: `.dds` files, in any case, get the Texture rule;
/// a quarantined `.dds.caobad` stays byte-identical.
#[test]
fn parity_rules_route_dds_files_to_the_texture_rule() {
    let temp = TempDir::new("texture-rule-routing");
    let (oracle, rust) = (temp.path().join("oracle"), temp.path().join("rust"));
    let source = source(16, false, smooth);
    let thorough = dds(&compress(&source, BC7, TEX_COMPRESS_DEFAULT));
    let quick = dds(&compress(&source, BC7, TEX_COMPRESS_BC7_QUICK));
    for name in ["mods/A/textures/a.dds", "mods/A/textures/B.DDS"] {
        write(&oracle, name, &thorough);
        write(&rust, name, &quick);
    }
    write(&oracle, "mods/A/textures/c.dds.caobad", &thorough);
    write(&rust, "mods/A/textures/c.dds.caobad", &quick);

    let comparison = compare_trees(
        TreeSide {
            root: &oracle,
            run_id: None,
        },
        TreeSide {
            root: &rust,
            run_id: None,
        },
        &ParityRules,
    )
    .unwrap();

    let verdict = |path: &str| {
        comparison
            .artifacts
            .iter()
            .find(|artifact| artifact.path == path)
            .map(|artifact| artifact.verdict.clone())
            .unwrap()
    };
    let rule = TextureRule.name();
    assert_eq!(
        verdict("mods/A/textures/a.dds"),
        ArtifactVerdict::Equivalent { rule }
    );
    assert_eq!(
        verdict("mods/A/textures/B.DDS"),
        ArtifactVerdict::Equivalent { rule }
    );
    let ArtifactVerdict::Different(caobad) = verdict("mods/A/textures/c.dds.caobad") else {
        panic!("a quarantined file must be byte-identical");
    };
    assert_eq!(caobad.rule, "Byte Equality");
    assert!(matches!(comparison.verdict(), Verdict::Different(_)));
}
