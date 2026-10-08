//! Loading Textures as C++ `TexturesOptimizer::open` does: DDS and TGA, with
//! typeless DDS formats reinterpreted as UNORM, and nothing on disk changed.

mod common;

use cao_core::routing::TextureVariant;
use cao_optimizers::textures::{Texture, TextureError};
use common::{scratch_dir, write, write_dds};
use directxtex::{
    CP_FLAGS_NONE, DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM,
    DXGI_FORMAT_R32G32B32A32_TYPELESS, ScratchImage, TGA_FLAGS_NONE,
};

#[test]
fn a_typeless_dds_loads_as_its_unorm_equivalent() {
    let path = scratch_dir("load-typeless").join("typeless.dds");
    write_dds(&path, DXGI_FORMAT_R8G8B8A8_TYPELESS, 8);
    let texture = Texture::load(&path, TextureVariant::Native).unwrap();
    assert_eq!(texture.metadata().format, DXGI_FORMAT_R8G8B8A8_UNORM);
    assert_eq!(
        texture.image().metadata().format,
        DXGI_FORMAT_R8G8B8A8_UNORM
    );
}

#[test]
fn a_texture_path_longer_than_the_old_fixed_buffer_loads() {
    // Deviation 5: C++ copied the path into a 1024-character buffer.
    let mut path = scratch_dir("load-long-path");
    while path.as_os_str().len() <= 1100 {
        path.push("a-deliberately-long-folder-name-for-deep-mod-trees");
    }
    let path = path.join("deep.dds");
    write_dds(&path, DXGI_FORMAT_R8G8B8A8_UNORM, 4);

    let texture = Texture::load(&path, TextureVariant::Native).unwrap();

    assert_eq!(texture.metadata().width, 4);
}

#[test]
fn a_typeless_dds_without_a_unorm_equivalent_fails_to_load() {
    let path = scratch_dir("load-float-typeless").join("float.dds");
    write_dds(&path, DXGI_FORMAT_R32G32B32A32_TYPELESS, 4);
    let error = Texture::load(&path, TextureVariant::Native).err().unwrap();
    assert!(matches!(error, TextureError::Typeless(_)), "{error}");
}

#[test]
fn a_file_that_is_not_a_texture_fails_to_load_and_is_left_alone() {
    let root = scratch_dir("load-broken");
    write(&root, "broken.dds", b"not a texture");
    let error = Texture::load(&root.join("broken.dds"), TextureVariant::Native)
        .err()
        .unwrap();
    assert!(matches!(error, TextureError::Decode(_)), "{error}");
    assert_eq!(
        std::fs::read(root.join("broken.dds")).unwrap(),
        b"not a texture"
    );
}

#[test]
fn a_tga_loads_as_a_convertible_texture() {
    let mut scratch = ScratchImage::default();
    scratch
        .initialize_2d(DXGI_FORMAT_R8G8B8A8_UNORM, 8, 8, 1, 1, CP_FLAGS_NONE)
        .unwrap();
    let tga = scratch.images()[0]
        .save_tga(TGA_FLAGS_NONE, Some(scratch.metadata()))
        .unwrap();
    let root = scratch_dir("load-tga");
    write(&root, "source.tga", tga.buffer());

    let texture = Texture::load(&root.join("source.tga"), TextureVariant::Convertible).unwrap();

    assert_eq!(
        (texture.metadata().width, texture.metadata().height),
        (8, 8)
    );
}
