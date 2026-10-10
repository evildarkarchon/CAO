//! Meshes through the production wiring (#504): nifly's `OptimizeFor` at the
//! C++ mesh levels, staged publication, and Quarantine.
//!
//! The inputs are nifly's own fixtures. All of them are SSE-compatible (no
//! triangle strips), so under the SSE profile they scan as good: the
//! necessary level leaves them alone, the medium level only resaves them, and
//! the full level converts them. The TES5 profile shows `scan()`'s
//! target-version quirk, which makes every Mesh a critical issue.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cao_core::execution::{AssetExecutionFailure, MutationState};
use cao_core::run::{OptimizationRunResult, RunOutcome};
use cao_optimizers::composition::ApplicationRun;
use cao_profiles::Options;
use common::{
    STAGING_CONTROL_FILES, app_dir, edit_profile, profile_options, scratch_dir, serial,
    snapshot_tree, staging_leftovers, write,
};
use nifly_sys::{LoadOptions, Nif};

/// A Skyrim LE Mesh (stream 83).
const LE_MESH: &str = "TestNifFile_Optimize_LE_to_SE.nif";
/// A Skyrim SE Mesh (stream 100).
const SSE_MESH: &str = "TestNifFile_Static_SE.nif";

/// The bytes of nifly's fixture `name`.
fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../nifly-sys/vendor/nifly/tests")
        .join(name);
    std::fs::read(path).unwrap()
}

/// The Bethesda stream version in a NIF's header: after the header line, the
/// file version (4 bytes), the endian flag (1), the user version (4) and the
/// block count (4).
fn stream_version(bytes: &[u8]) -> u32 {
    let line_end = bytes.iter().position(|&byte| byte == b'\n').unwrap() + 1;
    let at = line_end + 4 + 1 + 4 + 4;
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

/// `bytes` loaded and saved by nifly with no optimization, as a resave does.
fn resaved(bytes: &[u8], is_terrain: bool) -> Vec<u8> {
    let dir = scratch_dir("mesh-resave-reference");
    let (input, output) = (dir.join("input.nif"), dir.join("output.nif"));
    std::fs::write(&input, bytes).unwrap();
    let mut nif = Nif::new();
    nif.load(&input, LoadOptions { is_terrain }).unwrap();
    nif.save(&output).unwrap();
    std::fs::read(output).unwrap()
}

/// Apply options over `mod_root` under `profile` with only Mesh work at
/// `level`, resaving when `resave` is set, and head parts processed.
fn apply_meshes(app: &Path, profile: &str, mod_root: &Path, level: i32, resave: bool) -> Options {
    let mut options = profile_options(app, profile);
    options.dry_run = false;
    options.user_path = mod_root.to_string_lossy().into_owned();
    options.textures_necessary = false;
    options.textures_compress = false;
    options.textures_mipmaps = false;
    options.textures_resize_ratio = false;
    options.textures_resize_size = false;
    options.meshes_optimization_level = level;
    options.meshes_headparts = true;
    options.meshes_resave = resave;
    options.animations_optimization = false;
    options.bsa_extract = false;
    options.bsa_create = false;
    options
}

/// A Mod Root under a fresh app directory holding `meshes` as
/// `(relative path, fixture)` pairs.
fn mod_with_meshes(name: &str, meshes: &[(&str, &str)]) -> (PathBuf, PathBuf) {
    let app = app_dir(name);
    let mod_root = app.join("mods").join("Mod");
    for (relative, fixture_name) in meshes {
        write(&mod_root, relative, &fixture(fixture_name));
    }
    (app, mod_root)
}

/// Runs `options` under `profile` to completion.
fn run(app: &Path, profile: &str, options: &Options) -> Arc<OptimizationRunResult> {
    let run = ApplicationRun::new(app, profile, options).unwrap();
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

/// The Mesh at `relative` under `mod_root`.
fn read(mod_root: &Path, relative: &str) -> Vec<u8> {
    std::fs::read(mod_root.join(relative)).unwrap()
}

/// Spec (#504), C++ levels: the necessary level optimizes only critical issues, and
/// SSE-compatible Meshes are not one under the SSE profile, so nothing is
/// written.
#[test]
fn the_necessary_level_leaves_sse_compatible_meshes_alone() {
    let _serial = serial();
    let (app, mod_root) = mod_with_meshes(
        "mesh-necessary",
        &[("meshes/le.nif", LE_MESH), ("meshes/sse.nif", SSE_MESH)],
    );
    let before = snapshot_tree(&mod_root);

    let result = run(&app, "SSE", &apply_meshes(&app, "SSE", &mod_root, 1, false));

    assert_succeeded(&result);
    assert_eq!(
        mutations(&result),
        [
            ("le.nif".to_owned(), MutationState::None),
            ("sse.nif".to_owned(), MutationState::None)
        ]
    );
    let after: Vec<_> = snapshot_tree(&mod_root)
        .into_iter()
        .filter(|(path, _)| !path.starts_with(".cao-staging"))
        .collect();
    let before: Vec<_> = before.into_iter().collect();
    assert_eq!(after, before);
}

/// Spec (#504), C++ levels: the medium level saves every Mesh again but runs `OptimizeFor`
/// only on critical issues, so an SSE-compatible LE Mesh stays LE.
#[test]
fn the_medium_level_resaves_without_converting() {
    let _serial = serial();
    let (app, mod_root) = mod_with_meshes("mesh-medium", &[("meshes/le.nif", LE_MESH)]);

    let result = run(&app, "SSE", &apply_meshes(&app, "SSE", &mod_root, 2, false));

    assert_succeeded(&result);
    assert_eq!(
        mutations(&result),
        [("le.nif".to_owned(), MutationState::Committed)]
    );
    let output = read(&mod_root, "meshes/le.nif");
    assert_eq!(stream_version(&output), 83);
    assert_eq!(output, resaved(&fixture(LE_MESH), false));
    assert_eq!(staging_leftovers(&mod_root), STAGING_CONTROL_FILES);
}

/// Spec (#504), C++ levels: the full level runs `OptimizeFor` on every Mesh, converting an
/// LE Mesh to SSE; an SSE Mesh is already at the target and is only resaved.
#[test]
fn the_full_level_converts_le_meshes_to_sse() {
    let _serial = serial();
    let (app, mod_root) = mod_with_meshes(
        "mesh-full",
        &[("meshes/le.nif", LE_MESH), ("meshes/sse.nif", SSE_MESH)],
    );

    let result = run(&app, "SSE", &apply_meshes(&app, "SSE", &mod_root, 3, false));

    assert_succeeded(&result);
    assert_eq!(
        mutations(&result),
        [
            ("le.nif".to_owned(), MutationState::Committed),
            ("sse.nif".to_owned(), MutationState::Committed)
        ]
    );
    assert_eq!(stream_version(&read(&mod_root, "meshes/le.nif")), 100);
    assert_eq!(
        read(&mod_root, "meshes/sse.nif"),
        resaved(&fixture(SSE_MESH), false)
    );
}

/// Spec (#504): resaving is independent of the level: at level 0 every Mesh, terrain
/// included, is loaded and saved again unchanged by `OptimizeFor`.
#[test]
fn resaving_alone_saves_every_mesh_again() {
    let _serial = serial();
    let (app, mod_root) = mod_with_meshes(
        "mesh-resave",
        &[
            ("meshes/le.nif", LE_MESH),
            ("meshes/terrain/tamriel.4.0.0.btr", SSE_MESH),
        ],
    );

    let result = run(&app, "SSE", &apply_meshes(&app, "SSE", &mod_root, 0, true));

    assert_succeeded(&result);
    assert_eq!(
        mutations(&result),
        [
            ("le.nif".to_owned(), MutationState::Committed),
            ("tamriel.4.0.0.btr".to_owned(), MutationState::Committed)
        ]
    );
    assert_eq!(
        read(&mod_root, "meshes/le.nif"),
        resaved(&fixture(LE_MESH), false)
    );
    assert_eq!(
        read(&mod_root, "meshes/terrain/tamriel.4.0.0.btr"),
        resaved(&fixture(SSE_MESH), true)
    );
}

/// Ported as-is: `scan()` asks whether the profile's *target* version is
/// Skyrim LE, not the Mesh's, so under TES5 every Mesh is a critical issue.
/// The necessary level then converts an SSE Mesh to LE and resaves an LE one,
/// for which `OptimizeFor` is a no-op.
#[test]
fn an_le_target_makes_every_mesh_a_critical_issue() {
    let _serial = serial();
    let (app, mod_root) = mod_with_meshes(
        "mesh-le-target",
        &[("meshes/le.nif", LE_MESH), ("meshes/sse.nif", SSE_MESH)],
    );

    let result = run(
        &app,
        "TES5",
        &apply_meshes(&app, "TES5", &mod_root, 1, false),
    );

    assert_succeeded(&result);
    assert_eq!(
        mutations(&result),
        [
            ("le.nif".to_owned(), MutationState::Committed),
            ("sse.nif".to_owned(), MutationState::Committed)
        ]
    );
    assert_eq!(stream_version(&read(&mod_root, "meshes/sse.nif")), 83);
    assert_eq!(
        read(&mod_root, "meshes/le.nif"),
        resaved(&fixture(LE_MESH), false)
    );
}

/// Ported as-is: nifly's `OptimizeFor` converts only between LE and SSE, so
/// with an FO4 target it changes nothing, and the version mismatch it reports
/// is not logged. The full level still saves the Mesh again.
#[test]
fn an_fo4_target_resaves_without_converting() {
    let _serial = serial();
    let (app, mod_root) = mod_with_meshes("mesh-fo4-target", &[("meshes/le.nif", LE_MESH)]);
    edit_profile(&app, "SSE", "meshesStream", "130");

    let result = run(&app, "SSE", &apply_meshes(&app, "SSE", &mod_root, 3, false));

    assert_succeeded(&result);
    assert_eq!(
        mutations(&result),
        [("le.nif".to_owned(), MutationState::Committed)]
    );
    assert_eq!(
        read(&mod_root, "meshes/le.nif"),
        resaved(&fixture(LE_MESH), false)
    );
}

/// Spec (#504), C++ headparts: in Apply, a Mesh on a facegen path is a Headpart Mesh from
/// the necessary level up, and is optimized as one even when its scan finds
/// no critical issue.
#[test]
fn a_facegen_mesh_is_optimized_as_a_headpart() {
    let _serial = serial();
    let facegen = "meshes/actors/character/facegendata/facegeom/skyrim.esm/00000001.nif";
    let (app, mod_root) = mod_with_meshes(
        "mesh-facegen",
        &[(facegen, SSE_MESH), ("meshes/sse.nif", SSE_MESH)],
    );

    let result = run(&app, "SSE", &apply_meshes(&app, "SSE", &mod_root, 1, false));

    assert_succeeded(&result);
    assert_eq!(
        mutations(&result),
        [
            ("00000001.nif".to_owned(), MutationState::Committed),
            ("sse.nif".to_owned(), MutationState::None)
        ]
    );
}

/// Spec (#504), Quarantine (Apply only): a Mesh nifly cannot load is renamed to `.caobad`,
/// a Committed Mutation, and the run carries on with the other Meshes.
#[test]
fn an_unloadable_mesh_is_quarantined() {
    let _serial = serial();
    let (app, mod_root) = mod_with_meshes("mesh-quarantine", &[("meshes/le.nif", LE_MESH)]);
    write(&mod_root, "meshes/garbage.nif", b"not a mesh");
    write(&mod_root, "meshes/empty.nif", b"");

    let result = run(&app, "SSE", &apply_meshes(&app, "SSE", &mod_root, 3, false));

    assert_eq!(result.outcome(), RunOutcome::CompletedWithFailures);
    for attempt in result.asset_attempts() {
        assert_eq!(attempt.result.mutation_state(), MutationState::Committed);
        assert!(attempt.result.safe_to_continue());
    }
    let failures: Vec<_> = result
        .asset_attempts()
        .iter()
        .filter_map(|attempt| attempt.result.failure())
        .collect();
    assert_eq!(failures, [AssetExecutionFailure::LoadFailed; 2]);
    assert_eq!(read(&mod_root, "meshes/garbage.nif.caobad"), b"not a mesh");
    assert!(mod_root.join("meshes/empty.nif.caobad").exists());
    assert!(!mod_root.join("meshes/garbage.nif").exists());
    assert_eq!(stream_version(&read(&mod_root, "meshes/le.nif")), 100);
}

/// Spec (#504): Dry Run evaluates every Mesh, quarantines nothing and writes nothing.
#[test]
fn a_dry_run_over_meshes_changes_nothing() {
    let _serial = serial();
    let (app, mod_root) = mod_with_meshes(
        "mesh-dry-run",
        &[("meshes/le.nif", LE_MESH), ("meshes/sse.nif", SSE_MESH)],
    );
    write(&mod_root, "meshes/garbage.nif", b"not a mesh");
    let before = snapshot_tree(&app);
    let mut options = apply_meshes(&app, "SSE", &mod_root, 3, true);
    options.dry_run = true;

    let result = run(&app, "SSE", &options);

    assert_eq!(result.outcome(), RunOutcome::CompletedWithFailures);
    let failures: Vec<_> = result
        .asset_attempts()
        .iter()
        .filter_map(|attempt| attempt.result.failure())
        .collect();
    assert_eq!(failures, [AssetExecutionFailure::LoadFailed]);
    assert_eq!(snapshot_tree(&app), before, "Dry Run never mutates");
}
