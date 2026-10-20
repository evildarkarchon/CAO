//! The composition root: options into a Run Request, the profile-backed Run
//! Configuration Provider, and a Dry Run over loose Textures through the
//! production wiring. Ports the intent of C++ `ApplicationRunSetupTests`.

mod common;

use cao_core::execution::AssetExecutionFailure;
use cao_core::routing::{ExecutionMode, PolicyValidationError, RequestedWork, SkipReason};
use cao_core::run::{ModSelection, RunDiagnosticCode, RunFailureCode, RunOutcome, RunPhase};
use cao_optimizers::composition::{ApplicationRun, RunSetupError, run_request};
use cao_profiles::{OptimizationMode, Options};
use common::{app_dir, dry_run_textures, edit_profile, serial, snapshot_tree, write, write_dds};
use directxtex::{DXGI_FORMAT_B5G6R5_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM};

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
    // The default options select necessary Texture work. FO4 converts TGAs,
    // but that is its Conversion Preference, which Preparing adds; the
    // request carries only the caller's intent.
    assert!(request.requests(RequestedWork::NativeTextureOptimization));
    assert!(!request.requests(RequestedWork::ConvertibleTextureConversion));
    assert!(request.requests(RequestedWork::StandardMeshOptimization));
    assert!(request.requests(RequestedWork::TerrainMeshOptimization));
    assert!(!request.requests(RequestedWork::ArchiveCreation));
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
/// honour is refused before any service exists, so no run starts and nothing
/// holds the one active-run slot.
#[test]
fn a_request_contradicting_the_profile_capabilities_never_starts_a_run() {
    let _serial = serial();
    let app = app_dir("capability-conflict");
    let mod_root = app.join("mods").join("Mod");
    std::fs::create_dir_all(&mod_root).unwrap();

    // FO4 has no Mesh support: any Mesh work contradicts it.
    let mut options = dry_run_textures(&app, &mod_root);
    options.meshes_resave = true;
    let Err(RunSetupError::PolicyConflict(conflicts)) = ApplicationRun::new(&app, "FO4", &options)
    else {
        panic!("Mesh work under FO4 must be a policy conflict");
    };
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
    let error = ApplicationRun::new(&app, "SSE", &options).err().unwrap();
    assert!(
        matches!(error, RunSetupError::PolicyConflict(_)),
        "{error:?}"
    );
    assert!(!error.to_string().is_empty());

    // Neither refusal left a run behind: the next valid request starts.
    let run = ApplicationRun::new(&app, "FO4", &dry_run_textures(&app, &mod_root)).unwrap();
    assert_eq!(
        run.start(None).unwrap().wait().outcome(),
        RunOutcome::Succeeded
    );
}

/// Origin: ApplicationRunSetupTests::fo4ConversionCompilesWithoutMeshOptimization.
/// TGA conversion derives Mesh Reference Maintenance, which FO4 supports
/// through its Texture capability even though it has no Mesh optimization.
/// FO4's Conversion Preference is on, so Preparing adds the conversion.
#[test]
fn fo4_texture_conversion_needs_no_mesh_optimization_capability() {
    let _serial = serial();
    let app = app_dir("fo4-conversion");
    let mod_root = app.join("mods").join("Mod");
    std::fs::create_dir_all(&mod_root).unwrap();
    let options = dry_run_textures(&app, &mod_root);
    let run = ApplicationRun::new(&app, "FO4", &options).unwrap();

    let result = run.start(None).unwrap().wait();

    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    let policy = result.preparation().unwrap().policy();
    assert!(policy.requests(RequestedWork::ConvertibleTextureConversion));
    assert!(policy.maintains_mesh_references());
    assert!(!policy.requests(RequestedWork::StandardMeshOptimization));
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

/// The Conversion Preference is read with the rest of the Selected Profile
/// during Preparing, so an edit made after setup but before start is honoured
/// in either direction rather than frozen at setup.
#[test]
fn a_conversion_preference_edit_between_setup_and_start_is_honoured() {
    let _serial = serial();
    let app = app_dir("conversion-preference-reload");
    let mod_root = app.join("mods").join("Mod");
    write_dds(
        &mod_root.join("textures/plain.dds"),
        DXGI_FORMAT_R8G8B8A8_UNORM,
        16,
    );
    edit_profile(&app, "SSE", "texturesConvertTga", "false");
    let run = ApplicationRun::new(&app, "SSE", &dry_run_textures(&app, &mod_root)).unwrap();
    let converts = |run: &ApplicationRun| {
        let result = run.start(None).unwrap().wait();
        assert_eq!(result.outcome(), RunOutcome::Succeeded);
        let policy = result.preparation().unwrap().policy();
        assert_eq!(
            policy.requests(RequestedWork::ConvertibleTextureConversion),
            policy.maintains_mesh_references(),
            "Mesh Reference Maintenance follows conversion"
        );
        policy.requests(RequestedWork::ConvertibleTextureConversion)
    };

    edit_profile(&app, "SSE", "texturesConvertTga", "true");
    assert!(converts(&run), "turned on after setup");

    edit_profile(&app, "SSE", "texturesConvertTga", "false");
    assert!(!converts(&run), "turned off again before the next start");
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

    // Unreadable before setup too: the capability check leaves it to Preparing.
    let mut options = Options {
        user_path: mod_root.to_string_lossy().into_owned(),
        textures_necessary: false,
        ..Options::default()
    };
    options.dry_run = true;
    let run = ApplicationRun::new(&app, "MissingProfile", &options).unwrap();
    let result = run.start(None).unwrap().wait();
    assert_eq!(
        result.failures()[0].code,
        RunFailureCode::ConfigurationLoadingFailed
    );
}
