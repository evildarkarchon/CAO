//! Headpart Meshes through the production wiring (#505): recognised from the
//! profile's `customHeadparts.txt`, from HDPT records in any plugin across the
//! Mod Selection, and from facegen paths, with deviation 17's fixes.
//!
//! Every Mesh here is nifly's SSE fixture, which scans as good under the SSE
//! profile. At the necessary level such a Mesh is left alone unless it is a
//! Headpart Mesh, which is optimized as one and saved: a Committed Mutation
//! marks exactly the Meshes recognised as Headpart Meshes.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cao_core::execution::MutationState;
use cao_core::routing::{ExecutionMode, MeshVariant};
use cao_core::run::{OptimizationRunResult, RunDiagnosticCode, RunOutcome, RunPhase};
use cao_optimizers::composition::ApplicationRun;
use cao_optimizers::meshes::{MeshOptimizer, MeshSettings};
use cao_profiles::{OptimizationMode, Options};
use common::plugin::headpart_plugin;
use common::{app_dir, profile_options, serial, write};
use nifly_sys::NifVersion;

/// A Skyrim SE Mesh (stream 100), SSE-compatible.
const SSE_MESH: &str = "TestNifFile_Static_SE.nif";

/// The bytes of nifly's SSE fixture.
fn sse_mesh() -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../nifly-sys/vendor/nifly/tests")
        .join(SSE_MESH);
    std::fs::read(path).unwrap()
}

/// Apply options over `selection` with only Mesh work at the necessary level
/// and head parts processed.
fn apply_meshes(app: &Path, selection: &Path, mode: OptimizationMode) -> Options {
    let mut options = profile_options(app, "SSE");
    options.dry_run = false;
    options.mode = mode;
    options.user_path = selection.to_string_lossy().into_owned();
    options.textures_necessary = false;
    options.textures_compress = false;
    options.textures_mipmaps = false;
    options.textures_resize_ratio = false;
    options.textures_resize_size = false;
    options.meshes_optimization_level = 1;
    options.meshes_headparts = true;
    options.meshes_resave = false;
    options.animations_optimization = false;
    options.bsa_extract = false;
    options.bsa_create = false;
    options
}

/// Writes the SSE fixture at each of `meshes` under `mod_root`.
fn write_meshes(mod_root: &Path, meshes: &[&str]) {
    for mesh in meshes {
        write(mod_root, mesh, &sse_mesh());
    }
}

/// Runs `options` under the SSE profile to completion.
fn run(app: &Path, options: &Options) -> Arc<OptimizationRunResult> {
    let run = ApplicationRun::new(app, "SSE", options).unwrap();
    run.start(None).unwrap().wait()
}

/// The `/`-separated paths, under `selection`, of the Meshes the run saved,
/// sorted. Every other attempt must have left its Mesh alone.
fn saved(result: &OptimizationRunResult, selection: &Path) -> Vec<String> {
    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    let selection = cao_winfs::msvc_canonical(selection).unwrap();
    let mut saved: Vec<String> = result
        .asset_attempts()
        .iter()
        .filter(|attempt| {
            let state = attempt.result.mutation_state();
            assert!(matches!(
                state,
                MutationState::None | MutationState::Committed
            ));
            state == MutationState::Committed
        })
        .map(|attempt| {
            let path: PathBuf = attempt.asset.execution_path().into();
            path.strip_prefix(&selection)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    saved.sort();
    saved
}

/// Spec (#505): a plugin anywhere in the Mod Selection names Headpart Meshes
/// for every Mod Root, so a patch can name meshes another mod holds. The
/// plugin of a mod with a Mod Exclusion still counts. Matching ignores case,
/// and a MODL path with or without its `meshes\` prefix names the same Mesh.
#[test]
fn plugins_across_the_mod_selection_name_headpart_meshes() {
    let _serial = serial();
    let app = app_dir("headparts-plugins");
    let mods = app.join("mods");
    write(
        &mods.join("Patch"),
        "Patch.esp",
        &headpart_plugin(&["Hair\\PatchHair.nif"]),
    );
    write(
        &mods.join("Hair_separator"),
        "Separator.esm",
        &headpart_plugin(&["meshes\\HAIR\\SeparatorHair.nif"]),
    );
    write_meshes(
        &mods.join("HairMod"),
        &[
            "meshes/hair/patchhair.nif",
            "meshes/hair/separatorhair.nif",
            "meshes/hair/plain.nif",
        ],
    );

    let result = run(
        &app,
        &apply_meshes(&app, &mods, OptimizationMode::SeveralMods),
    );

    assert_eq!(
        saved(&result, &mods),
        [
            "HairMod/meshes/hair/patchhair.nif",
            "HairMod/meshes/hair/separatorhair.nif",
        ]
    );
}

/// Spec (#505): the Meshes the profile's `customHeadparts.txt` lists are
/// Headpart Meshes; the SSE profile ships the vanilla beards.
#[test]
fn the_profile_list_names_headpart_meshes() {
    let _serial = serial();
    let app = app_dir("headparts-profile");
    let mod_root = app.join("mods").join("BeardMod");
    let beard = "meshes/actors/character/character assets/beards/humanbeardlong01.nif";
    write_meshes(&mod_root, &[beard, "meshes/actors/character/plain.nif"]);

    let result = run(
        &app,
        &apply_meshes(&app, &mod_root, OptimizationMode::SingleMod),
    );

    assert_eq!(saved(&result, &mod_root), [beard]);
}

/// Deviation 17: a Mesh is matched by its game path within its Mod Root. C++
/// cut the absolute path at its first `/meshes/`, so a `meshes` folder above
/// the Mod Root hid the listed beard, and a `facegen` folder between that
/// folder and the Mod Root made every Mesh beneath it a Headpart Mesh.
#[test]
fn headparts_are_matched_within_the_mod_root() {
    let _serial = serial();
    let app = app_dir("headparts-within-root");
    let mod_root = app.join("meshes").join("facegen").join("BeardMod");
    let beard = "meshes/actors/character/character assets/beards/humanbeardlong01.nif";
    let facegen = "meshes/actors/character/facegendata/facegeom/beardmod.esp/00000801.nif";
    write_meshes(
        &mod_root,
        &[beard, facegen, "meshes/actors/character/plain.nif"],
    );

    let result = run(
        &app,
        &apply_meshes(&app, &mod_root, OptimizationMode::SingleMod),
    );

    assert_eq!(saved(&result, &mod_root), [beard, facegen]);
}

/// Deviation 17: the plugin scan skips the `.cao-staging` namespace, whether a
/// Several Mods child or a folder inside a Mod Root. C++ read the plugins in
/// both. The nested folder sits below the Mod Root's top level, where
/// Preparing would refuse an unknown staging-like name before any scan.
#[test]
fn the_plugin_scan_skips_cao_staging() {
    let _serial = serial();
    let app = app_dir("headparts-staging");
    let mods = app.join("mods");
    write(
        &mods.join(".cao-staging-old"),
        "Stale.esp",
        &headpart_plugin(&["Hair\\StaleChild.nif"]),
    );
    write(
        &mods
            .join("HairMod")
            .join("plugins")
            .join(".cao-staging-old"),
        "Stale.esp",
        &headpart_plugin(&["Hair\\StaleNested.nif"]),
    );
    write(
        &mods.join("HairMod"),
        "HairMod.esp",
        &headpart_plugin(&["Hair\\Live.nif"]),
    );
    write_meshes(
        &mods.join("HairMod"),
        &[
            "meshes/hair/stalechild.nif",
            "meshes/hair/stalenested.nif",
            "meshes/hair/live.nif",
        ],
    );

    let result = run(
        &app,
        &apply_meshes(&app, &mods, OptimizationMode::SeveralMods),
    );

    assert_eq!(saved(&result, &mods), ["HairMod/meshes/hair/live.nif"]);
}

/// Deviation 17: Dry Run applies the facegen rule, as Apply does, so it
/// reports an SSE-compatible Mesh on a facegen path at the necessary level as
/// one that would change. C++'s Dry Run applied only the listed Headpart
/// Meshes. A Dry Run's verdict on a Mesh is not in Run Evidence, so it is read
/// from the optimizer the backend calls.
#[test]
fn dry_run_applies_the_facegen_rule() {
    let dir = common::scratch_dir("headparts-dry-run-facegen");
    let mod_root = dir.join("FaceMod");
    let facegen = "meshes/actors/character/facegendata/facegeom/facemod.esp/00000801.nif";
    write_meshes(&mod_root, &[facegen, "meshes/actors/character/plain.nif"]);
    let settings = MeshSettings {
        level: 1,
        headparts: true,
        resave: false,
    };
    let optimizer = MeshOptimizer::new(settings, NifVersion::SSE);
    let would_change = |relative: &str| {
        let path = mod_root.join(relative);
        let mut nif = optimizer.load(&path, MeshVariant::Standard).unwrap();
        optimizer
            .optimize(&mut nif, &path, &mod_root, ExecutionMode::DryRun)
            .unwrap()
    };

    assert!(would_change(facegen));
    assert!(!would_change("meshes/actors/character/plain.nif"));
}

/// Deviation 17: a plugin whose HDPT group or record is truncated is
/// unreadable. The run reports it as a Run Diagnostic, which leaves the Run
/// Outcome alone, and carries on with every other plugin. C++ hung.
#[test]
fn an_unreadable_plugin_is_reported_and_the_run_carries_on() {
    let _serial = serial();
    let app = app_dir("headparts-unreadable");
    let mod_root = app.join("mods").join("HairMod");
    let mut truncated_group = headpart_plugin(&["Hair\\Lost.nif"]);
    truncated_group.truncate(truncated_group.len() - 10);
    let mut long_record = headpart_plugin(&["Hair\\AlsoLost.nif"]);
    // The HDPT record's size, after the TES4 record (24 + 18 bytes) and the
    // group header (24), now runs past the end of its group.
    long_record[66 + 4..66 + 8].copy_from_slice(&4000_u32.to_le_bytes());
    write(&mod_root, "Broken.esp", &truncated_group);
    write(&mod_root, "AlsoBroken.esm", &long_record);
    write(
        &mod_root,
        "HairMod.esp",
        &headpart_plugin(&["Hair\\Live.nif"]),
    );
    write_meshes(
        &mod_root,
        &[
            "meshes/hair/lost.nif",
            "meshes/hair/alsolost.nif",
            "meshes/hair/live.nif",
        ],
    );

    let result = run(
        &app,
        &apply_meshes(&app, &mod_root, OptimizationMode::SingleMod),
    );

    assert_eq!(saved(&result, &mod_root), ["meshes/hair/live.nif"]);
    let root = cao_winfs::msvc_canonical(&mod_root).unwrap();
    let mut reported: Vec<_> = result
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code == RunDiagnosticCode::PluginUnreadable)
        .map(|diagnostic| (diagnostic.phase, diagnostic.path.clone()))
        .collect();
    reported.sort();
    assert_eq!(
        reported,
        [
            (RunPhase::ProcessingAssets, root.join("AlsoBroken.esm")),
            (RunPhase::ProcessingAssets, root.join("Broken.esp")),
        ]
    );
}
