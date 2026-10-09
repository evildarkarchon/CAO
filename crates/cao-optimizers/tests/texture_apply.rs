//! Apply over loose Textures through the production wiring (#491): the CPU
//! Texture path, staged publication and Quarantine.
//!
//! TES5 targets BC3, which DirectXTex encodes on the CPU in both builds, so
//! these outputs are what the parity oracle produces too. SSE's BC7 path needs
//! the GPU encoder and the PSNR rule, which arrive with #494 and #495.

mod common;

use std::path::Path;

use cao_core::execution::{AssetExecutionFailure, MutationState};
use cao_core::run::RunOutcome;
use cao_optimizers::composition::ApplicationRun;
use cao_profiles::Options;
use common::{app_dir, profile_options, serial, snapshot_tree, write, write_dds};
use directxtex::{
    DDS_FLAGS_NONE, DXGI_FORMAT_BC3_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM, ScratchImage, TexMetadata,
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

/// A decision that needs mipmaps fails cleanly until mipmap generation lands
/// (#494): nothing is staged over the original, and the run continues.
#[test]
fn a_decision_needing_mipmaps_fails_without_mutating() {
    let _serial = serial();
    let app = app_dir("apply-tes5-mipmaps");
    let mod_root = app.join("mods").join("Mod");
    write_dds(
        &mod_root.join("textures/plain.dds"),
        DXGI_FORMAT_R8G8B8A8_UNORM,
        64,
    );
    let before = snapshot_tree(&mod_root);
    let mut options = apply_textures(&app, "TES5", &mod_root);
    options.textures_mipmaps = true;

    let run = ApplicationRun::new(&app, "TES5", &options).unwrap();
    let result = run.start(None).unwrap().wait();

    assert_eq!(result.outcome(), RunOutcome::CompletedWithFailures);
    let attempt = &result.asset_attempts()[0].result;
    assert_eq!(
        attempt.failure(),
        Some(AssetExecutionFailure::OperationFailed)
    );
    assert_eq!(attempt.mutation_state(), MutationState::None);
    assert_eq!(snapshot_tree(&mod_root), before);
}
