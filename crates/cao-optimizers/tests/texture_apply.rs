//! Apply over loose Textures through the production wiring (#491, #494): the
//! CPU Texture path with mipmaps, staged publication and Quarantine.
//!
//! TES5 targets BC3, which DirectXTex encodes on the CPU in both builds, so
//! these outputs are what the parity oracle produces too. SSE's BC7 is encoded
//! on the GPU when there is one, which arrives with #495.

mod common;

use std::path::Path;

use cao_core::execution::MutationState;
use cao_core::run::RunOutcome;
use cao_optimizers::composition::ApplicationRun;
use cao_profiles::Options;
use common::{app_dir, profile_options, serial, write, write_dds};
use directxtex::{
    CP_FLAGS_NONE, DDS_FLAGS_NONE, DXGI_FORMAT_BC1_UNORM, DXGI_FORMAT_BC3_UNORM,
    DXGI_FORMAT_R8G8B8A8_UNORM, ScratchImage, TexMetadata,
};

/// Apply options over `mod_root` under `profile` with only Texture work:
/// necessary optimization and compression, resizing to 32×32, no mipmaps.
fn apply_textures(app: &Path, profile: &str, mod_root: &Path) -> Options {
    let mut options = profile_options(app, profile);
    options.dry_run = false;
    options.user_path = mod_root.to_string_lossy().into_owned();
    options.textures_necessary = true;
    options.textures_compress = true;
    options.textures_mipmaps = false;
    options.textures_resize_ratio = false;
    options.textures_resize_size = true;
    options.textures_target_width = 32;
    options.textures_target_height = 32;
    options.meshes_optimization_level = 0;
    options.meshes_resave = false;
    options.animations_optimization = false;
    options.bsa_extract = false;
    options.bsa_create = false;
    options
}

/// The metadata of the DDS file at `path`.
fn dds_metadata(path: &Path) -> TexMetadata {
    let bytes = std::fs::read(path).unwrap();
    let mut info = TexMetadata::default();
    ScratchImage::load_dds(&bytes, DDS_FLAGS_NONE, Some(&mut info), None).unwrap();
    info
}

/// Spec (#491): a TES5 Apply resizes and BC3-compresses a loose Texture on the
/// CPU, publishes it over the original, records the Committed Mutation, and
/// quarantines a Texture that fails to load.
#[test]
fn a_tes5_apply_publishes_bc3_textures_and_quarantines_broken_ones() {
    let _serial = serial();
    let app = app_dir("apply-tes5-textures");
    let mod_root = app.join("mods").join("Mod");
    write_dds(
        &mod_root.join("textures/plain.dds"),
        DXGI_FORMAT_R8G8B8A8_UNORM,
        64,
    );
    write(&mod_root, "textures/broken.dds", b"not a texture");

    let run = ApplicationRun::new(&app, "TES5", &apply_textures(&app, "TES5", &mod_root)).unwrap();
    let result = run.start(None).unwrap().wait();

    assert_eq!(
        result.outcome(),
        RunOutcome::CompletedWithFailures,
        "{:?}",
        result.failures()
    );
    let info = dds_metadata(&mod_root.join("textures/plain.dds"));
    assert_eq!(info.format, DXGI_FORMAT_BC3_UNORM);
    assert_eq!((info.width, info.height, info.mip_levels), (32, 32, 1));
    assert!(mod_root.join("textures/broken.dds.caobad").exists());
    assert!(!mod_root.join("textures/broken.dds").exists());
    for attempt in result.asset_attempts() {
        assert_eq!(attempt.result.mutation_state(), MutationState::Committed);
    }
    assert_eq!(result.mutation_summaries()[0].committed, 2);
    let mut staging: Vec<String> = std::fs::read_dir(mod_root.join(".cao-staging"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    staging.sort();
    assert_eq!(staging, ["owner.lock", "ownership.manifest"]);
}

/// Spec (#494): mipmaps are generated on the Run Worker. An 8-bit Texture's
/// mip chain goes through WIC, which only works once the worker has called
/// `CoInitializeEx`, so this run proves the backend initialized COM there.
#[test]
fn a_tes5_apply_generates_the_full_mip_chain_on_the_run_worker() {
    let _serial = serial();
    let app = app_dir("apply-tes5-mipmaps");
    let mod_root = app.join("mods").join("Mod");
    write_dds(
        &mod_root.join("textures/plain.dds"),
        DXGI_FORMAT_R8G8B8A8_UNORM,
        64,
    );
    let mut options = apply_textures(&app, "TES5", &mod_root);
    options.textures_mipmaps = true;

    let run = ApplicationRun::new(&app, "TES5", &options).unwrap();
    let result = run.start(None).unwrap().wait();

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.asset_attempts()
    );
    let info = dds_metadata(&mod_root.join("textures/plain.dds"));
    assert_eq!(info.format, DXGI_FORMAT_BC3_UNORM);
    // Resized to 32×32 first, so the full chain is 32, 16, 8, 4, 2, 1.
    assert_eq!((info.width, info.height, info.mip_levels), (32, 32, 6));
}

/// A partial mip chain is stripped to its top level and regenerated whole, as
/// C++ `generateMipMaps` does, keeping a cubemap's every face.
#[test]
fn a_partial_mip_chain_is_regenerated_whole() {
    let _serial = serial();
    let app = app_dir("apply-tes5-partial-mips");
    let mod_root = app.join("mods").join("Mod");
    let mut scratch = ScratchImage::default();
    scratch
        .initialize_cube(DXGI_FORMAT_R8G8B8A8_UNORM, 16, 16, 1, 3, CP_FLAGS_NONE)
        .unwrap();
    for (index, byte) in scratch.pixels_mut().iter_mut().enumerate() {
        *byte = (index * 13) as u8;
    }
    write(
        &mod_root,
        "textures/sky.dds",
        scratch.save_dds(DDS_FLAGS_NONE).unwrap().buffer(),
    );
    let mut options = apply_textures(&app, "TES5", &mod_root);
    options.textures_mipmaps = true;
    options.textures_resize_size = false;

    let run = ApplicationRun::new(&app, "TES5", &options).unwrap();
    let result = run.start(None).unwrap().wait();

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.asset_attempts()
    );
    let info = dds_metadata(&mod_root.join("textures/sky.dds"));
    assert!(info.is_cubemap());
    assert_eq!(info.format, DXGI_FORMAT_BC3_UNORM);
    assert_eq!((info.array_size, info.mip_levels), (6, 5));
}

/// A compressed Texture that is resized is compressed back to its own format,
/// because C++ picks the target before decompressing.
#[test]
fn a_resized_compressed_texture_keeps_its_format() {
    let _serial = serial();
    let app = app_dir("apply-tes5-resize-bc1");
    let mod_root = app.join("mods").join("Mod");
    write_dds(
        &mod_root.join("textures/worn.dds"),
        DXGI_FORMAT_BC1_UNORM,
        64,
    );

    let run = ApplicationRun::new(&app, "TES5", &apply_textures(&app, "TES5", &mod_root)).unwrap();
    let result = run.start(None).unwrap().wait();

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.asset_attempts()
    );
    let info = dds_metadata(&mod_root.join("textures/worn.dds"));
    assert_eq!(info.format, DXGI_FORMAT_BC1_UNORM);
    assert_eq!((info.width, info.height), (32, 32));
}

/// Deviation 5 through a whole Apply: a Texture whose path is longer than
/// C++'s 1024-character load buffer is loaded, staged, saved and published.
#[test]
fn a_texture_deeper_than_the_old_fixed_buffer_is_optimized() {
    let _serial = serial();
    let app = app_dir("apply-long-path");
    let mod_root = app.join("mods").join("Mod");
    let mut folder = mod_root.join("textures");
    while folder.as_os_str().len() <= 1100 {
        folder.push("a-deliberately-long-folder-name-for-deep-mod-trees");
    }
    let path = folder.join("deep.dds");
    write_dds(&path, DXGI_FORMAT_R8G8B8A8_UNORM, 64);

    let run = ApplicationRun::new(&app, "TES5", &apply_textures(&app, "TES5", &mod_root)).unwrap();
    let result = run.start(None).unwrap().wait();

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.asset_attempts()
    );
    let info = dds_metadata(&path);
    assert_eq!(info.format, DXGI_FORMAT_BC3_UNORM);
    assert_eq!((info.width, info.height), (32, 32));
}
