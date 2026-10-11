//! Mesh Reference Maintenance through the production wiring (#506): a TGA
//! Texture converted to DDS rewrites every Mesh in the Mod Root that
//! references it, so no Mesh points at the deleted TGA.
//!
//! These port the selected C++ `MainOptimizerTests` and
//! `MeshReferenceMaintenanceTests` scenarios onto whole runs. The Meshes are
//! nifly's `TestNifFile_Static_SE.nif` with one texture path replaced, and
//! Mesh optimization is off throughout, so Mesh Reference Maintenance is the
//! only Mesh work and the only reason a Mesh is written.

mod common;

use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cao_core::execution::{AssetExecutionFailure, MutationState};
use cao_core::run::{MutationKind, OptimizationRunResult, RunOutcome};
use cao_optimizers::composition::ApplicationRun;
use cao_profiles::{OptimizationMode, Options};
use common::{app_dir, profile_options, scratch_dir, serial, snapshot_tree, write};
use directxtex::{CP_FLAGS_NONE, DXGI_FORMAT_R8G8B8A8_UNORM, ScratchImage, TGA_FLAGS_NONE};
use nifly_sys::{LoadOptions, Nif};

/// An SSE Mesh whose texture paths all name DDS Textures.
const SSE_MESH: &str = "TestNifFile_Static_SE.nif";

/// The bytes of nifly's fixture `name`.
fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../nifly-sys/vendor/nifly/tests")
        .join(name);
    std::fs::read(path).unwrap()
}

/// An 8×8 TGA, small enough to convert quickly and large enough to compress.
fn tga_bytes() -> Vec<u8> {
    let mut scratch = ScratchImage::default();
    scratch
        .initialize_2d(DXGI_FORMAT_R8G8B8A8_UNORM, 8, 8, 1, 1, CP_FLAGS_NONE)
        .unwrap();
    for (index, byte) in scratch.pixels_mut().iter_mut().enumerate() {
        *byte = (index * 13) as u8;
    }
    scratch.images()[0]
        .save_tga(TGA_FLAGS_NONE, Some(scratch.metadata()))
        .unwrap()
        .buffer()
        .to_vec()
}

/// Writes a Mesh at `root/relative` whose first non-empty texture path is
/// `reference`, so it references that Texture and only DDS Textures besides.
fn write_mesh_referencing(root: &Path, relative: &str, reference: &[u8]) {
    let dir = scratch_dir("mesh-reference-source");
    let source = dir.join("source.nif");
    std::fs::write(&source, fixture(SSE_MESH)).unwrap();
    let mut nif = Nif::new();
    nif.load(&source, LoadOptions::default()).unwrap();
    let mut textures = nif.texture_paths().unwrap();
    let index = (0..textures.len())
        .find(|&index| !textures.get(index).unwrap().is_empty())
        .expect("the fixture references a Texture");
    textures.set(index, reference).unwrap();

    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    nif.save(&path).unwrap();
}

/// Every texture path the Mesh at `root/relative` references, decoded lossily.
fn texture_paths(root: &Path, relative: &str) -> Vec<String> {
    let mut nif = Nif::new();
    nif.load(&root.join(relative), LoadOptions::default())
        .unwrap();
    let textures = nif.texture_paths().unwrap();
    (0..textures.len())
        .map(|index| String::from_utf8_lossy(textures.get(index).unwrap()).into_owned())
        .collect()
}

/// Options over `user_path` under the SSE profile, which converts TGAs: only
/// necessary Texture optimization, and no Mesh, Animation or Archive work, so
/// Meshes are routed for Mesh Reference Maintenance alone.
fn convert_tgas(app: &Path, user_path: &Path, dry_run: bool) -> Options {
    let mut options = profile_options(app, "SSE");
    options.dry_run = dry_run;
    options.user_path = user_path.to_string_lossy().into_owned();
    options.textures_necessary = true;
    options.textures_compress = false;
    options.textures_mipmaps = false;
    options.textures_resize_ratio = false;
    options.textures_resize_size = false;
    options.meshes_optimization_level = 0;
    options.meshes_resave = false;
    options.animations_optimization = false;
    options.bsa_extract = false;
    options.bsa_create = false;
    options
}

/// A fresh app directory and its Mod Root `mods/Mod`.
fn app_with_mod(name: &str) -> (PathBuf, PathBuf) {
    let app = app_dir(name);
    let mod_root = app.join("mods").join("Mod");
    (app, mod_root)
}

/// Runs `options` under the SSE profile to completion.
fn run(app: &Path, options: &Options) -> Arc<OptimizationRunResult> {
    let run = ApplicationRun::new(app, "SSE", options).unwrap();
    run.start(None).unwrap().wait()
}

/// Asserts the run succeeded, showing its Run Failures otherwise.
fn assert_succeeded(result: &OptimizationRunResult) {
    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
}

/// Each attempt's file name and mutation state, sorted by name.
fn mutations(result: &OptimizationRunResult) -> Vec<(String, MutationState)> {
    let mut mutations: Vec<_> = result
        .asset_attempts()
        .iter()
        .map(|attempt| {
            let name = attempt.asset.execution_path().file_name().unwrap();
            (
                name.to_string_lossy().into_owned(),
                attempt.result.mutation_state(),
            )
        })
        .collect();
    mutations.sort();
    mutations
}

/// C++ `successfulRunStillMaintainsMeshReferences`, with a bystander: the
/// converted TGA's referencing Mesh now names the DDS, and a Mesh that never
/// referenced it is not written at all.
#[test]
fn a_converted_tga_rewrites_only_the_meshes_that_reference_it() {
    let _serial = serial();
    let (app, mod_root) = app_with_mod("mesh-references-apply");
    write(&mod_root, "textures/armor/body.tga", &tga_bytes());
    write_mesh_referencing(
        &mod_root,
        "meshes/armor/body.nif",
        b"textures\\armor\\body.TgA",
    );
    write(&mod_root, "meshes/armor/other.nif", &fixture(SSE_MESH));

    let result = run(&app, &convert_tgas(&app, &mod_root, false));

    assert_succeeded(&result);
    assert!(mod_root.join("textures/armor/body.dds").is_file());
    assert!(!mod_root.join("textures/armor/body.tga").exists());
    let body = texture_paths(&mod_root, "meshes/armor/body.nif");
    assert!(
        body.contains(&"textures\\armor\\body.dds".to_owned()),
        "{body:?}"
    );
    assert!(
        !body
            .iter()
            .any(|path| path.to_ascii_lowercase().contains(".tga")),
        "{body:?}"
    );
    assert_eq!(
        std::fs::read(mod_root.join("meshes/armor/other.nif")).unwrap(),
        fixture(SSE_MESH),
        "a Mesh without a TGA reference is untouched"
    );
}

/// The Texture's publication and the rewritten Mesh's publication are each a
/// Committed Mutation of the Mod Root; the untouched Mesh is none.
#[test]
fn the_texture_and_the_mesh_publications_record_mutation_facts() {
    let _serial = serial();
    let (app, mod_root) = app_with_mod("mesh-references-mutations");
    write(&mod_root, "textures/body.tga", &tga_bytes());
    write_mesh_referencing(&mod_root, "meshes/body.nif", b"textures\\body.tga");
    write(&mod_root, "meshes/other.nif", &fixture(SSE_MESH));

    let result = run(&app, &convert_tgas(&app, &mod_root, false));

    assert_succeeded(&result);
    assert_eq!(
        mutations(&result),
        [
            ("body.nif".to_owned(), MutationState::Committed),
            ("body.tga".to_owned(), MutationState::Committed),
            ("other.nif".to_owned(), MutationState::None),
        ]
    );
    let summaries = result.mutation_summaries();
    assert_eq!(summaries.len(), 1, "{summaries:?}");
    assert_eq!(summaries[0].kind, MutationKind::AssetProcessing);
    assert_eq!(
        (summaries[0].committed, summaries[0].partial_or_unknown),
        (2, 0)
    );
}

/// C++ `failedConversionSuppressesMeshReferenceMaintenance` and
/// `failedConversionKeepsUnrelatedMeshReferences`: a TGA that fails to load
/// never becomes a DDS, so the Mesh naming it keeps the TGA name (and is not
/// written), while a Mesh naming a TGA that did convert is still rewritten.
#[test]
fn a_failed_conversion_keeps_only_the_references_to_that_texture() {
    let _serial = serial();
    let (app, mod_root) = app_with_mod("mesh-references-failed");
    write(&mod_root, "textures/broken.tga", b"not a TGA");
    write(&mod_root, "textures/converted.tga", &tga_bytes());
    write_mesh_referencing(&mod_root, "meshes/broken.nif", b"textures\\broken.tga");
    write_mesh_referencing(
        &mod_root,
        "meshes/converted.nif",
        b"textures\\converted.tga",
    );
    let broken_mesh = std::fs::read(mod_root.join("meshes/broken.nif")).unwrap();

    let result = run(&app, &convert_tgas(&app, &mod_root, false));

    assert_eq!(result.outcome(), RunOutcome::CompletedWithFailures);
    let failed: Vec<_> = result
        .asset_attempts()
        .iter()
        .filter(|attempt| !attempt.result.succeeded())
        .map(|attempt| attempt.asset.execution_path().file_name().unwrap())
        .collect();
    assert_eq!(failed, ["broken.tga"]);
    assert!(!mod_root.join("textures/broken.dds").exists());
    assert_eq!(
        std::fs::read(mod_root.join("meshes/broken.nif")).unwrap(),
        broken_mesh,
        "the Mesh naming the failed Texture is not rewritten"
    );
    assert!(
        texture_paths(&mod_root, "meshes/converted.nif")
            .contains(&"textures\\converted.dds".to_owned())
    );
}

/// C++ `failedConversionInAnotherModRootKeepsMeshReferences`: under Several
/// Mods, one mod's failed `textures\shared.tga` withholds only its own
/// Meshes' rewrites, not a sibling mod's rewrite of the same name, whose TGA
/// converted and was deleted.
#[test]
fn a_failed_conversion_in_another_mod_root_keeps_this_mods_rewrite() {
    let _serial = serial();
    let app = app_dir("mesh-references-several-mods");
    let mods = app.join("mods");
    write(&mods, "a/textures/shared.tga", b"not a TGA");
    write(&mods, "b/textures/shared.tga", &tga_bytes());
    write_mesh_referencing(&mods, "a/meshes/m.nif", b"textures\\shared.tga");
    write_mesh_referencing(&mods, "b/meshes/m.nif", b"textures\\shared.tga");
    let mut options = convert_tgas(&app, &mods, false);
    options.mode = OptimizationMode::SeveralMods;

    let result = run(&app, &options);

    assert_eq!(result.outcome(), RunOutcome::CompletedWithFailures);
    assert!(mods.join("b/textures/shared.dds").is_file());
    assert!(
        texture_paths(&mods, "a/meshes/m.nif").contains(&"textures\\shared.tga".to_owned()),
        "mod a's Mesh keeps naming its failed Texture"
    );
    assert!(
        texture_paths(&mods, "b/meshes/m.nif").contains(&"textures\\shared.dds".to_owned()),
        "mod b's Mesh names the DDS its own TGA became"
    );
}

/// C++ `committedConversionWithRetainedSourceMaintainsMeshReferences`: the
/// DDS committed but the TGA could not be removed (another process holds it
/// without delete sharing). The DDS exists, so the Mesh is still rewritten.
#[test]
fn a_committed_conversion_whose_tga_is_retained_still_rewrites_the_mesh() {
    let _serial = serial();
    let (app, mod_root) = app_with_mod("mesh-references-retained");
    write(&mod_root, "textures/retained.tga", &tga_bytes());
    write_mesh_referencing(&mod_root, "meshes/retained.nif", b"textures\\retained.tga");
    // FILE_SHARE_READ | FILE_SHARE_WRITE, without FILE_SHARE_DELETE: the run can
    // read and convert the TGA, but not delete it.
    let _held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x1 | 0x2)
        .open(mod_root.join("textures/retained.tga"))
        .unwrap();

    let result = run(&app, &convert_tgas(&app, &mod_root, false));

    let texture = result
        .asset_attempts()
        .iter()
        .find(|attempt| attempt.asset.execution_path().ends_with("retained.tga"))
        .unwrap();
    assert_eq!(
        texture.result.failure(),
        Some(AssetExecutionFailure::SourceRemovalFailed)
    );
    assert_eq!(texture.result.mutation_state(), MutationState::Committed);
    assert!(texture.result.safe_to_continue());
    assert!(mod_root.join("textures/retained.dds").is_file());
    assert!(mod_root.join("textures/retained.tga").is_file());
    assert!(
        texture_paths(&mod_root, "meshes/retained.nif")
            .contains(&"textures\\retained.dds".to_owned())
    );
}

/// Dry Run evaluates both the conversion and the Mesh rewrite, reporting each
/// as an attempt that commits nothing, and leaves the Mod Root byte-for-byte
/// as it was.
#[test]
fn a_dry_run_reports_the_conversion_and_changes_nothing() {
    let _serial = serial();
    let (app, mod_root) = app_with_mod("mesh-references-dry-run");
    write(&mod_root, "textures/body.tga", &tga_bytes());
    write_mesh_referencing(&mod_root, "meshes/body.nif", b"textures\\body.tga");
    let before = snapshot_tree(&mod_root);

    let result = run(&app, &convert_tgas(&app, &mod_root, true));

    assert_succeeded(&result);
    assert_eq!(
        mutations(&result),
        [
            ("body.nif".to_owned(), MutationState::None),
            ("body.tga".to_owned(), MutationState::None),
        ]
    );
    assert!(result.mutation_summaries().is_empty());
    assert_eq!(snapshot_tree(&mod_root), before);
}
