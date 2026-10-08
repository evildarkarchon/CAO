//! The Texture decisions C++ `TexturesOptimizer::processArguments` makes, which
//! Dry Run reports and Apply acts on: resize, compress and mipmaps.
//!
//! Each scenario is a texture's metadata and name against a profile and the
//! user's Texture options; none needs a file.

use cao_core::routing::TextureVariant;
use cao_optimizers::textures::{TexturePlan, TextureProfile, TextureRequest, plan};
use directxtex::{
    DXGI_FORMAT, DXGI_FORMAT_B5G6R5_UNORM, DXGI_FORMAT_BC1_UNORM, DXGI_FORMAT_BC7_UNORM,
    DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, TEX_ALPHA_MODE_OPAQUE,
    TEX_DIMENSION_TEXTURE2D, TEX_MISC_TEXTURECUBE, TexMetadata,
};

/// The shipped SSE profile's Texture settings: BC7, three unwanted 16-bit
/// formats, and interface Textures compressed.
fn sse() -> TextureProfile {
    TextureProfile {
        format: DXGI_FORMAT_BC7_UNORM,
        unwanted_formats: [85, 86, 115].map(DXGI_FORMAT::from).to_vec(),
        compress_interface: true,
    }
}

fn metadata(format: DXGI_FORMAT, width: usize, height: usize, mip_levels: usize) -> TexMetadata {
    TexMetadata {
        width,
        height,
        depth: 1,
        array_size: 1,
        mip_levels,
        misc_flags: 0,
        misc_flags2: 0,
        format,
        dimension: TEX_DIMENSION_TEXTURE2D,
    }
}

const NAME: &str = "C:/mods/Mod/textures/armor/plate.dds";

fn necessary() -> TextureRequest {
    TextureRequest {
        necessary: true,
        ..TextureRequest::default()
    }
}

fn native(info: &TexMetadata, request: &TextureRequest) -> TexturePlan {
    plan(info, NAME, TextureVariant::Native, &sse(), request)
}

#[test]
fn a_texture_already_in_shape_needs_no_work() {
    let info = metadata(DXGI_FORMAT_BC7_UNORM, 256, 256, 9);
    let request = TextureRequest {
        necessary: true,
        compress: true,
        mipmaps: true,
        target: Some((512, 512)),
    };
    let decided = native(&info, &request);
    assert!(!decided.would_change(), "{decided:?}");
    assert_eq!((decided.width, decided.height), (256, 256));
}

#[test]
fn an_unwanted_format_is_compressed_only_when_necessary_optimization_is_on() {
    let info = metadata(DXGI_FORMAT_B5G6R5_UNORM, 64, 64, 7);
    assert!(native(&info, &necessary()).compress);
    assert!(!native(&info, &TextureRequest::default()).compress);
}

#[test]
fn a_tga_is_always_compressed_by_necessary_optimization() {
    let info = metadata(DXGI_FORMAT_R8G8B8A8_UNORM, 64, 64, 1);
    let decided = plan(
        &info,
        NAME,
        TextureVariant::Convertible,
        &sse(),
        &necessary(),
    );
    assert!(decided.compress);
    let decided = native(&info, &necessary());
    assert!(!decided.compress, "the same pixels as a DDS are compatible");
}

#[test]
fn a_compressed_texture_that_is_not_a_power_of_two_is_incompatible() {
    let info = metadata(DXGI_FORMAT_BC1_UNORM, 12, 16, 1);
    assert!(native(&info, &necessary()).compress);
    let info = metadata(DXGI_FORMAT_BC1_UNORM, 16, 16, 1);
    assert!(!native(&info, &necessary()).compress);
}

#[test]
fn an_uncompressed_cubemap_without_alpha_is_incompatible() {
    let mut cubemap = metadata(DXGI_FORMAT_R8G8B8A8_UNORM, 16, 16, 1);
    cubemap.array_size = 6;
    cubemap.misc_flags = TEX_MISC_TEXTURECUBE.bits();
    assert!(
        !native(&cubemap, &necessary()).compress,
        "alpha that is not declared opaque is compatible"
    );
    cubemap.misc_flags2 = TEX_ALPHA_MODE_OPAQUE.bits();
    assert!(native(&cubemap, &necessary()).compress);
}

#[test]
fn compression_needs_an_uncompressed_power_of_two_texture_of_at_least_4x4_in_another_format() {
    let compress = TextureRequest {
        compress: true,
        ..TextureRequest::default()
    };
    let cases = [
        (metadata(DXGI_FORMAT_R8G8B8A8_UNORM, 64, 64, 1), true),
        (metadata(DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, 4, 4, 1), true),
        (metadata(DXGI_FORMAT_R8G8B8A8_UNORM, 2, 64, 1), false),
        (metadata(DXGI_FORMAT_R8G8B8A8_UNORM, 48, 64, 1), false),
        (metadata(DXGI_FORMAT_BC1_UNORM, 64, 64, 1), false),
    ];
    for (info, expected) in cases {
        assert_eq!(native(&info, &compress).compress, expected, "{info:?}");
    }
}

#[test]
fn interface_textures_are_left_uncompressed_and_unmipped_unless_the_profile_allows_them() {
    let info = metadata(DXGI_FORMAT_R8G8B8A8_UNORM, 64, 64, 1);
    let request = TextureRequest {
        compress: true,
        mipmaps: true,
        ..TextureRequest::default()
    };
    let mut profile = sse();
    profile.compress_interface = false;
    // C++ looks for "interface" anywhere in the full path, ignoring case.
    let name = "C:/mods/Mod/textures/InterFace/map.dds";
    let decided = plan(&info, name, TextureVariant::Native, &profile, &request);
    assert!(!decided.compress && !decided.mipmaps, "{decided:?}");

    let decided = plan(&info, name, TextureVariant::Native, &sse(), &request);
    assert!(decided.compress && decided.mipmaps, "{decided:?}");
}

#[test]
fn mipmaps_are_generated_when_the_chain_is_not_the_full_one() {
    let mipmaps = TextureRequest {
        mipmaps: true,
        ..TextureRequest::default()
    };
    // 64x32 has a full chain of 7 levels: 64, 32, 16, 8, 4, 2, 1.
    assert!(native(&metadata(DXGI_FORMAT_BC7_UNORM, 64, 32, 1), &mipmaps).mipmaps);
    assert!(!native(&metadata(DXGI_FORMAT_BC7_UNORM, 64, 32, 7), &mipmaps).mipmaps);
    assert!(!native(&metadata(DXGI_FORMAT_BC7_UNORM, 2, 2, 1), &mipmaps).mipmaps);
    assert!(!native(&metadata(DXGI_FORMAT_BC7_UNORM, 64, 32, 1), &necessary()).mipmaps);
}

#[test]
fn resizing_halves_both_sides_while_both_exceed_the_target() {
    let target = |width, height| TextureRequest {
        target: Some((width, height)),
        ..TextureRequest::default()
    };
    let info = metadata(DXGI_FORMAT_BC7_UNORM, 2048, 1024, 12);
    let decided = native(&info, &target(512, 512));
    assert!(decided.resize);
    assert_eq!((decided.width, decided.height), (1024, 512));

    let decided = native(&info, &target(4096, 512));
    assert!(
        !decided.resize,
        "one side within the target stops the halving"
    );
    assert_eq!((decided.width, decided.height), (2048, 1024));
}
