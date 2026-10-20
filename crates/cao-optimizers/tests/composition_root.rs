//! The composition root: options into a Run Request, the profile-backed Run
//! Configuration Provider, and a Dry Run over loose Textures through the
//! production wiring. Ports the intent of C++ `ApplicationRunSetupTests`.

mod common;

use std::path::Path;

use cao_core::execution::AssetExecutionFailure;
use cao_core::routing::{
    AssetOperation, ExecutionMode, PolicyValidationError, ProfileCapabilities, ProfileCapability,
    RequestedWork, SkipReason,
};
use cao_core::run::{
    ModSelection, OptimizationRunResult, RunConfigurationProvider, RunDiagnosticCode,
    RunFailureCode, RunOutcome, RunPhase,
};
use cao_optimizers::composition::{ApplicationRun, ProfileConfigurationProvider, run_request};
use cao_profiles::{OptimizationMode, Options};
use common::{
    app_dir, dry_run_textures, edit_profile, profile_options, serial, snapshot_tree, write,
    write_dds,
};
use directxtex::{
    CP_FLAGS_NONE, DXGI_FORMAT_B5G6R5_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM, ScratchImage,
    TGA_FLAGS_NONE,
};

#[test]
fn invalid_option_values_are_rejected_before_any_run() {
    let app = app_dir("invalid-options");
    let base = || Options {
        user_path: app.join("mods/Mod").to_string_lossy().into_owned(),
        ..Options::default()
    };
    let mut cases: Vec<(&str, Options)> = Vec::new();
    let mut add = |name, change: &dyn Fn(&mut Options)| {
        let mut options = base();
        change(&mut options);
        cases.push((name, options));
    };
    add("negative mesh level", &|o| o.meshes_optimization_level = -1);
    add("excessive mesh level", &|o| o.meshes_optimization_level = 4);
    add("zero width ratio", &|o| {
        o.textures_resize_ratio = true;
        o.textures_target_width_ratio = 0;
    });
    add("zero height ratio", &|o| {
        o.textures_resize_ratio = true;
        o.textures_target_height_ratio = 0;
    });
    add("zero target width", &|o| {
        o.textures_resize_size = true;
        o.textures_target_width = 0;
    });
    add("zero target height", &|o| {
        o.textures_resize_size = true;
        o.textures_target_height = 0;
    });
    add("odd target width", &|o| {
        o.textures_resize_size = true;
        o.textures_target_width = 513;
    });
    add("invalid mode", &|o| {
        o.mode = OptimizationMode::Unsupported(2)
    });
    add("relative folder", &|o| o.user_path = "mods/Mod".into());

    for (name, options) in cases {
        assert!(run_request("SSE", &options).is_err(), "{name}");
    }
}

#[test]
fn an_odd_target_size_is_accepted_while_resizing_by_size_is_off() {
    // Deviation 18: C++ rejected an odd width even when it was never used.
    let app = app_dir("odd-size-unused");
    let options = Options {
        user_path: app.join("mods/Mod").to_string_lossy().into_owned(),
        textures_resize_size: false,
        textures_target_width: 513,
        ..Options::default()
    };
    assert!(run_request("SSE", &options).is_ok());
}

#[test]
fn the_request_owns_the_caller_intent() {
    let app = app_dir("request-intent");
    let mods = app.join("mods");
    let options = Options {
        mode: OptimizationMode::SeveralMods,
        user_path: mods.to_string_lossy().into_owned(),
        dry_run: true,
        meshes_resave: true,
        ..Options::default()
    };

    let request = run_request("FO4", &options).unwrap();

    assert_eq!(request.profile_identity(), "FO4");
    assert_eq!(request.execution_mode(), ExecutionMode::DryRun);
    assert_eq!(request.mod_selection(), &ModSelection::ChildModRoots(mods));
    // The default options select necessary Texture work. Although FO4 prefers
    // conversion, the request never carries it: Preparing derives it from the
    // profile it reads.
    assert!(request.requests(RequestedWork::NativeTextureOptimization));
    assert!(request.requests(RequestedWork::StandardMeshOptimization));
    assert!(request.requests(RequestedWork::TerrainMeshOptimization));
    assert!(!request.requests(RequestedWork::ArchiveCreation));
    // The intent is exactly the work the options chose, so the request carries
    // no conversion of any kind.
    let mut work = request.requested_work().to_vec();
    work.sort();
    assert_eq!(
        work,
        [
            RequestedWork::NativeTextureOptimization,
            RequestedWork::StandardMeshOptimization,
            RequestedWork::TerrainMeshOptimization,
        ]
    );
}

#[test]
fn disabled_texture_work_does_not_request_profile_conversion() {
    let app = app_dir("no-texture-work");
    let options = Options {
        user_path: app.join("mods/Mod").to_string_lossy().into_owned(),
        textures_necessary: false,
        ..Options::default()
    };
    let request = run_request("SSE", &options).unwrap();
    assert!(!request.has_requested_work());
}

#[test]
fn every_kind_of_work_is_accepted() {
    let app = app_dir("available-work");

    // Meshes are ported (#504), at every level and when only resaving, in
    // both modes.
    for (level, resave) in [(1, false), (2, false), (3, false), (0, true)] {
        let mut options = dry_run_textures(&app, &app.join("mods/Mod"));
        options.meshes_optimization_level = level;
        options.meshes_resave = resave;
        assert!(ApplicationRun::new(&app, "SSE", &options).is_ok());
        options.dry_run = false;
        assert!(ApplicationRun::new(&app, "SSE", &options).is_ok());
    }

    // Animations are ported (#502), in both modes.
    let mut options = dry_run_textures(&app, &app.join("mods/Mod"));
    options.animations_optimization = true;
    assert!(ApplicationRun::new(&app, "SSE", &options).is_ok());
    options.dry_run = false;
    assert!(ApplicationRun::new(&app, "SSE", &options).is_ok());

    // Archive creation is ported (#498), in both modes.
    let mut options = dry_run_textures(&app, &app.join("mods/Mod"));
    options.bsa_create = true;
    assert!(
        ApplicationRun::new(&app, "SSE", &options).is_ok(),
        "Dry Run never packs"
    );
    options.dry_run = false;
    assert!(ApplicationRun::new(&app, "SSE", &options).is_ok());
}

/// Origin: ApplicationRunSetupTests::archiveCreationRequiresProfileArchiveSupport
/// and the FO4 Profile Capabilities. A request the selected profile cannot
/// honour is not a Start Error: setup accepts it, and the run fails Preparing
/// with a Policy Conflict Run Failure.
#[test]
fn a_request_contradicting_the_profile_capabilities_fails_preparing() {
    let _serial = serial();
    let app = app_dir("capability-conflict");
    let mod_root = app.join("mods").join("Mod");
    std::fs::create_dir_all(&mod_root).unwrap();

    // FO4 has no Mesh support: any Mesh work contradicts it.
    let mut options = dry_run_textures(&app, &mod_root);
    options.meshes_resave = true;
    let result = ApplicationRun::new(&app, "FO4", &options)
        .unwrap()
        .start(None)
        .unwrap()
        .wait();
    assert_eq!(result.outcome(), RunOutcome::Failed);
    let failure = &result.failures()[0];
    assert_eq!(failure.code, RunFailureCode::PolicyConflict);
    assert_eq!(failure.phase, RunPhase::Preparing);
    let conflicts = &failure.policy_conflicts;
    assert!(!conflicts.is_empty());
    assert!(conflicts.iter().all(|conflict| matches!(
        conflict,
        PolicyValidationError::UnsupportedRequestedAssetKind {
            request: RequestedWork::StandardMeshOptimization
                | RequestedWork::TerrainMeshOptimization,
            ..
        } | PolicyValidationError::UnsupportedRequestedAssetVariant {
            request: RequestedWork::StandardMeshOptimization
                | RequestedWork::TerrainMeshOptimization,
            ..
        }
    )));

    // A profile with no Archive support cannot create Archives, even in a
    // Dry Run that would never pack them.
    edit_profile(&app, "SSE", "bsaEnabled", "false");
    let mut options = dry_run_textures(&app, &mod_root);
    options.bsa_create = true;
    let result = ApplicationRun::new(&app, "SSE", &options)
        .unwrap()
        .start(None)
        .unwrap()
        .wait();
    assert_eq!(result.outcome(), RunOutcome::Failed);
    let failure = &result.failures()[0];
    assert_eq!(failure.code, RunFailureCode::PolicyConflict);
    assert_eq!(failure.phase, RunPhase::Preparing);
    assert!(failure.policy_conflicts.iter().any(|conflict| matches!(
        conflict,
        PolicyValidationError::UnsupportedRequestedAssetKind {
            request: RequestedWork::ArchiveCreation,
            ..
        } | PolicyValidationError::UnsupportedRequestedAssetVariant {
            request: RequestedWork::ArchiveCreation,
            ..
        }
    )));
}

/// Origin: ApplicationRunSetupTests::fo4ConversionCompilesWithoutMeshOptimization.
/// TGA conversion derives Mesh Reference Maintenance, which FO4 supports
/// through its Texture capability even though it has no Mesh optimization, so
/// a run under FO4's Conversion Preference prepares and converts.
#[test]
fn fo4_texture_conversion_needs_no_mesh_optimization_capability() {
    let _serial = serial();
    let app = app_dir("fo4-conversion");
    let mod_root = app.join("mods").join("Mod");
    write(&mod_root, "textures/armor.tga", &tga_bytes());
    let options = dry_run_textures(&app, &mod_root);
    let run = ApplicationRun::new(&app, "FO4", &options).unwrap();
    assert!(
        !run.request()
            .requests(RequestedWork::StandardMeshOptimization)
    );

    let result = run.start(None).unwrap().wait();

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    let policy = result.preparation().unwrap().policy();
    assert!(policy.converts_textures());
    assert!(policy.maintains_mesh_references());
    assert!(attempt_carries(
        &result,
        "armor.tga",
        AssetOperation::Conversion
    ));
}

/// Several Mods through the production wiring: the shipped SSE profile's
/// `ignoredMods.txt` and MO2's separator suffix exclude children with a Run
/// Diagnostic each, and the run's outcome is unaffected.
#[test]
fn several_mods_excludes_separators_and_ignored_mods_from_the_shipped_profile() {
    let _serial = serial();
    let app = app_dir("several-mods");
    let mods = app.join("mods");
    for name in [
        "Alpha",
        "Beta separator pack",
        "Group_separator",
        "Nemesis",
        ".cao-staging-old",
    ] {
        write_dds(
            &mods.join(name).join("textures/plain.dds"),
            DXGI_FORMAT_R8G8B8A8_UNORM,
            16,
        );
    }
    let mut options = dry_run_textures(&app, &mods);
    options.mode = OptimizationMode::SeveralMods;
    let before = snapshot_tree(&app);

    let result = ApplicationRun::new(&app, "SSE", &options)
        .unwrap()
        .start(None)
        .unwrap()
        .wait();

    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    let root = cao_winfs::msvc_canonical(&mods).unwrap();
    // Deviation 20: "separator" inside a name is not a separator. Deviation
    // 19: the staging-named child is neither a Mod Root nor diagnosed.
    assert_eq!(
        result.mod_roots(),
        [root.join("Alpha"), root.join("Beta separator pack")]
    );
    let diagnostics: Vec<_> = result
        .diagnostics()
        .iter()
        .map(|diagnostic| (diagnostic.code, diagnostic.path.clone()))
        .collect();
    assert_eq!(
        diagnostics,
        [
            (
                RunDiagnosticCode::SeparatorModExcluded,
                root.join("Group_separator")
            ),
            (RunDiagnosticCode::IgnoredModExcluded, root.join("Nemesis")),
        ]
    );
    assert_eq!(result.asset_attempts().len(), 2);
    assert_eq!(snapshot_tree(&app), before, "Dry Run never mutates");
}

/// The production provider over `app`'s profiles, set up for runs of the
/// options `profile` saves, wired from the Run Request those options build as
/// `ApplicationRun::new` wires it.
fn provider(app: &Path, profile: &str) -> ProfileConfigurationProvider {
    let options = profile_options(app, profile);
    let request = run_request(profile, &options).unwrap();
    ProfileConfigurationProvider::new(app, &request, &options)
}

/// The Selected Profile the production provider loads from the shipped FO4
/// profile: its Profile Capabilities and Archive extension, its Conversion
/// Preference, and the ignored-mods list and Packing Exclusions read with
/// it, together with the Run Toolset built from the same read.
#[test]
fn the_provider_loads_the_selected_profile_from_the_profile_files() {
    let app = app_dir("provider-selected-profile");
    write(
        &app.join("profiles/SSE"),
        "ignoredMods.txt",
        b"# comment\n  Tool   Mod  \n\n",
    );
    write(
        &app.join("profiles/FO4"),
        "FilesToNotPack.txt",
        b"# comment\nreadme.txt\n  meshes/kept  \n",
    );

    // FO4 has no ignoredMods.txt of its own, so it falls back to SSE's.
    let (profile, toolset) = provider(&app, "FO4").load("FO4").unwrap();

    assert!(toolset.is_some(), "the provider always builds the Toolset");
    // FO4 enables Textures and Archives, but neither Meshes nor Animations.
    // Mesh Reference Maintenance belongs to Texture conversion.
    assert_eq!(
        profile.capabilities(),
        &ProfileCapabilities::define(
            ".ba2",
            &[
                ProfileCapability::NativeTextureOptimization,
                ProfileCapability::ConvertibleTextureConversion,
                ProfileCapability::MeshReferenceMaintenance,
                ProfileCapability::ArchiveExtraction,
                ProfileCapability::ArchiveCreation,
            ]
        )
    );
    assert!(profile.prefers_conversion());
    assert_eq!(profile.ignored_mods(), ["Tool Mod"]);
    assert_eq!(profile.packing_exclusions(), ["readme.txt", "meshes/kept"]);

    assert!(provider(&app, "FO4").load("MissingProfile").is_err());
}

/// Each load reads the profile afresh, as each run's Preparing does, so an
/// edit between two loads is in the second Selected Profile.
#[test]
fn the_provider_loads_an_updated_selected_profile() {
    let app = app_dir("provider-updated");
    let provider = provider(&app, "FO4");
    let (first, _) = provider.load("FO4").unwrap();
    assert!(first.prefers_conversion());

    // Edited between two loads, as between two runs' Preparing.
    edit_profile(&app, "FO4", "bsaGame", "4");
    edit_profile(&app, "FO4", "bsaEnabled", "false");
    edit_profile(&app, "FO4", "texturesConvertTga", "false");
    edit_profile(&app, "FO4", "meshesEnabled", "true");
    let (second, _) = provider.load("FO4").unwrap();

    assert_eq!(
        second.capabilities(),
        &ProfileCapabilities::define(
            ".bsa",
            &[
                ProfileCapability::NativeTextureOptimization,
                ProfileCapability::ConvertibleTextureConversion,
                ProfileCapability::StandardMeshOptimization,
                ProfileCapability::TerrainMeshOptimization,
                ProfileCapability::MeshReferenceMaintenance,
            ]
        )
    );
    assert!(!second.prefers_conversion());
}

#[test]
fn the_provider_rejects_an_unreadable_ignored_mod_list() {
    let app = app_dir("provider-unreadable-ignored");
    std::fs::create_dir(app.join("profiles/FO4/ignoredMods.txt")).unwrap();
    assert!(provider(&app, "FO4").load("FO4").is_err());
}

#[test]
fn a_dry_run_over_loose_textures_evaluates_them_and_changes_nothing() {
    let _serial = serial();
    let app = app_dir("dry-run-textures");
    let mod_root = app.join("mods").join("Mod");
    write_dds(
        &mod_root.join("textures/plain.dds"),
        DXGI_FORMAT_R8G8B8A8_UNORM,
        64,
    );
    write_dds(
        &mod_root.join("textures/unwanted.dds"),
        DXGI_FORMAT_B5G6R5_UNORM,
        16,
    );
    write(&mod_root, "textures/broken.dds", b"not a texture");
    // SSE converts TGAs, so every Mesh is routed for Mesh Reference
    // Maintenance; this one fails to load, as it does in C++.
    write(&mod_root, "meshes/thing.nif", b"not a mesh");
    write(&mod_root, "meshes/idle.hkx", b"not requested");
    let before = snapshot_tree(&app);

    let run = ApplicationRun::new(&app, "SSE", &dry_run_textures(&app, &mod_root)).unwrap();
    let result = run.start(None).unwrap().wait();

    assert_eq!(result.outcome(), RunOutcome::CompletedWithFailures);
    assert_eq!(result.final_phase(), RunPhase::ArchiveFinalization);
    let failures: Vec<_> = result
        .asset_attempts()
        .iter()
        .filter(|attempt| !attempt.result.succeeded())
        .map(|attempt| {
            let name = attempt.asset.execution_path().file_name().unwrap();
            (
                name.to_string_lossy().into_owned(),
                attempt.result.failure(),
            )
        })
        .collect();
    let load_failed = Some(AssetExecutionFailure::LoadFailed);
    assert_eq!(
        failures,
        [
            ("broken.dds".to_owned(), load_failed),
            ("thing.nif".to_owned(), load_failed)
        ]
    );
    assert_eq!(result.asset_attempts().len(), 4);
    assert_eq!(result.skipped_asset_count(SkipReason::DisabledAssetKind), 1);
    assert_eq!(snapshot_tree(&app), before, "Dry Run never mutates");
}

#[test]
fn a_change_to_profile_ini_between_two_runs_is_picked_up_at_preparing() {
    let _serial = serial();
    let app = app_dir("profile-reload");
    let mod_root = app.join("mods").join("Mod");
    write_dds(
        &mod_root.join("textures/plain.dds"),
        DXGI_FORMAT_R8G8B8A8_UNORM,
        16,
    );
    let run = ApplicationRun::new(&app, "SSE", &dry_run_textures(&app, &mod_root)).unwrap();

    let first = run.start(None).unwrap().wait();
    assert_eq!(first.outcome(), RunOutcome::Succeeded);

    // The request was built before the edit; only Preparing can see it.
    edit_profile(&app, "SSE", "texturesEnabled", "false");
    let second = run.start(None).unwrap().wait();
    assert_eq!(second.outcome(), RunOutcome::Failed);
    let failure = &second.failures()[0];
    assert_eq!(failure.code, RunFailureCode::PolicyConflict);
    assert_eq!(failure.phase, RunPhase::Preparing);
}

/// An 8×8 TGA, small enough to convert quickly.
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

/// Apply options over `mod_root` under SSE with only necessary Texture work,
/// so a TGA is converted exactly when the Conversion Preference says so.
fn apply_necessary_textures(app: &Path, mod_root: &Path) -> Options {
    let mut options = dry_run_textures(app, mod_root);
    options.dry_run = false;
    options.textures_compress = false;
    options.textures_mipmaps = false;
    options
}

/// Whether any attempt of `result` on a file named `name` carries `operation`.
fn attempt_carries(result: &OptimizationRunResult, name: &str, operation: AssetOperation) -> bool {
    result.asset_attempts().iter().any(|attempt| {
        attempt.asset.execution_path().file_name() == Some(name.as_ref())
            && attempt.asset.operations().contains(operation)
    })
}

/// A Conversion Preference turned off after setup but before the run starts
/// is honoured: Preparing's read decides, so no TGA is converted.
#[test]
fn a_conversion_preference_turned_off_before_the_start_converts_nothing() {
    let _serial = serial();
    let app = app_dir("conversion-preference-off");
    let mod_root = app.join("mods").join("Mod");
    write(&mod_root, "textures/armor.tga", &tga_bytes());
    write(&mod_root, "meshes/armor.nif", b"not a mesh");
    // The shipped SSE profile prefers conversion when the run is set up.
    let run = ApplicationRun::new(&app, "SSE", &apply_necessary_textures(&app, &mod_root)).unwrap();

    edit_profile(&app, "SSE", "texturesConvertTga", "false");
    let result = run.start(None).unwrap().wait();

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    assert_eq!(
        std::fs::read(mod_root.join("textures/armor.tga")).unwrap(),
        tga_bytes()
    );
    assert!(!mod_root.join("textures/armor.dds").exists());
    assert!(result.asset_attempts().is_empty());
    assert_eq!(
        result.skipped_asset_count(SkipReason::ExcludedAssetVariant),
        1,
        "the TGA is a Texture Variant this run excludes"
    );
    assert_eq!(
        result.skipped_asset_count(SkipReason::DisabledAssetKind),
        1,
        "without conversion no Mesh needs its references maintained"
    );
}

/// A Conversion Preference turned on after setup but before the run starts
/// is honoured: the TGA is converted, and Meshes are routed for Mesh
/// Reference Maintenance.
#[test]
fn a_conversion_preference_turned_on_before_the_start_converts_and_maintains_mesh_references() {
    let _serial = serial();
    let app = app_dir("conversion-preference-on");
    let mod_root = app.join("mods").join("Mod");
    write(&mod_root, "textures/armor.tga", &tga_bytes());
    // Fails to load, but its routed operations are still visible.
    write(&mod_root, "meshes/armor.nif", b"not a mesh");
    edit_profile(&app, "SSE", "texturesConvertTga", "false");
    let run = ApplicationRun::new(&app, "SSE", &apply_necessary_textures(&app, &mod_root)).unwrap();

    edit_profile(&app, "SSE", "texturesConvertTga", "true");
    let result = run.start(None).unwrap().wait();

    assert!(result.preparation().unwrap().policy().converts_textures());
    assert!(attempt_carries(
        &result,
        "armor.tga",
        AssetOperation::Conversion
    ));
    assert!(mod_root.join("textures/armor.dds").is_file());
    assert!(!mod_root.join("textures/armor.tga").exists());
    assert!(attempt_carries(
        &result,
        "armor.nif",
        AssetOperation::MeshReferenceMaintenance
    ));
}

#[test]
fn an_unreadable_profile_fails_preparing_rather_than_the_start() {
    let _serial = serial();
    let app = app_dir("profile-unreadable-at-preparing");
    let mod_root = app.join("mods").join("Mod");
    std::fs::create_dir_all(&mod_root).unwrap();
    let options = dry_run_textures(&app, &mod_root);
    let run = ApplicationRun::new(&app, "SSE", &options).unwrap();
    // Deviation 11: an unsupported game makes the profile unreadable.
    edit_profile(&app, "SSE", "bsaGame", "7");

    let result = run.start(None).unwrap().wait();

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert_eq!(
        result.failures()[0].code,
        RunFailureCode::ConfigurationLoadingFailed
    );

    // Unreadable before setup too: setup never reads the profile, even for
    // Texture work, so Preparing reports it.
    let run = ApplicationRun::new(&app, "MissingProfile", &options).unwrap();
    let result = run.start(None).unwrap().wait();
    assert_eq!(
        result.failures()[0].code,
        RunFailureCode::ConfigurationLoadingFailed
    );
}
