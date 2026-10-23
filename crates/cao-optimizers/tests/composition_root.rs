//! The composition root: options into a Run Request, the profile-backed Run
//! Configuration Provider, and a Dry Run over loose Textures through the
//! production wiring. Ports the intent of C++ `ApplicationRunSetupTests`.

mod common;

use std::path::Path;

use cao_archive::ReadArchive;
use cao_core::execution::AssetExecutionFailure;
use cao_core::routing::{
    AssetOperation, ExecutionMode, PolicyValidationError, ProfileCapabilities, ProfileCapability,
    RequestedWork, SkipReason,
};
use cao_core::run::{
    ArchiveFinalizationChoices, MeshChoices, MeshLevel, ModSelection, OptimizationRunResult,
    ResizeRatio, ResizeSize, RunConfigurationProvider, RunDiagnosticCode, RunFailureCode,
    RunOutcome, RunPhase, RunRequest, SourceCleanup, TextureChoices, TextureResizeChoice,
    WorkChoiceError, WorkChoices,
};
use cao_optimizers::composition::{
    ProfileConfigurationProvider, RunSetupError, run_request, run_service,
};
use cao_optimizers::profile::profile_capabilities;
use cao_profiles::{OptimizationMode, Options, Profiles};
use common::{
    app_dir, canonical, dry_run_textures, edit_profile, profile_options, serial, snapshot_tree,
    start_and_wait, write, write_dds,
};
use directxtex::{
    CP_FLAGS_NONE, DDS_FLAGS_NONE, DXGI_FORMAT_B5G6R5_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM,
    ScratchImage, TGA_FLAGS_NONE, TexMetadata,
};

#[test]
fn invalid_option_values_are_rejected_before_any_run() {
    let app = app_dir("invalid-options");
    let base = || single_mod_options(&app);
    // Each case's option change, the error it must give, and that error's
    // message, which the GUI shows as it is.
    type Case = (
        &'static str,
        Options,
        fn(&RunSetupError) -> bool,
        &'static str,
    );
    let mut cases: Vec<Case> = Vec::new();
    let mut add = |name, change: &dyn Fn(&mut Options), expected, message| {
        let mut options = base();
        change(&mut options);
        cases.push((name, options, expected, message));
    };
    let mesh_level = "Mesh optimization level must be between 0 and 3.";
    add(
        "negative mesh level",
        &|o| o.meshes_optimization_level = -1,
        |e| matches!(e, RunSetupError::WorkChoice(WorkChoiceError::MeshLevel(-1))),
        mesh_level,
    );
    add(
        "excessive mesh level",
        &|o| o.meshes_optimization_level = 4,
        |e| matches!(e, RunSetupError::WorkChoice(WorkChoiceError::MeshLevel(4))),
        mesh_level,
    );
    let zero_ratio = "Texture resizing requires non-zero width and height ratios.";
    add(
        "zero width ratio",
        &|o| {
            o.textures_resize_ratio = true;
            o.textures_target_width_ratio = 0;
        },
        |e| {
            matches!(
                e,
                RunSetupError::WorkChoice(WorkChoiceError::ZeroResizeRatio)
            )
        },
        zero_ratio,
    );
    add(
        "zero height ratio",
        &|o| {
            o.textures_resize_ratio = true;
            o.textures_target_height_ratio = 0;
        },
        |e| {
            matches!(
                e,
                RunSetupError::WorkChoice(WorkChoiceError::ZeroResizeRatio)
            )
        },
        zero_ratio,
    );
    let zero_size = "Texture resizing requires non-zero width and height.";
    add(
        "zero target width",
        &|o| {
            o.textures_resize_size = true;
            o.textures_target_width = 0;
        },
        |e| {
            matches!(
                e,
                RunSetupError::WorkChoice(WorkChoiceError::ZeroResizeSize)
            )
        },
        zero_size,
    );
    add(
        "zero target height",
        &|o| {
            o.textures_resize_size = true;
            o.textures_target_height = 0;
        },
        |e| {
            matches!(
                e,
                RunSetupError::WorkChoice(WorkChoiceError::ZeroResizeSize)
            )
        },
        zero_size,
    );
    add(
        "odd target width",
        &|o| {
            o.textures_resize_size = true;
            o.textures_target_width = 513;
        },
        |e| matches!(e, RunSetupError::WorkChoice(WorkChoiceError::OddResizeSize)),
        "Texture target width and height must be even.",
    );
    add(
        "invalid mode",
        &|o| o.mode = OptimizationMode::Unsupported(2),
        |e| matches!(e, RunSetupError::UnsupportedMode(2)),
        "This mode does not exist.",
    );
    add(
        "relative folder",
        &|o| o.user_path = "mods/Mod".into(),
        |e| matches!(e, RunSetupError::RelativeSelection(path) if path == Path::new("mods/Mod")),
        "The selected folder `mods/Mod` is not an absolute path.",
    );

    for (name, options, expected, message) in cases {
        let error = run_request("SSE", &options, None).unwrap_err();
        assert!(expected(&error), "{name}: {error:?}");
        assert_eq!(error.to_string(), message, "{name}");
    }
}

/// The folder and mode are checked before any Work Choice, so a form wrong in
/// both ways reports its folder or mode.
#[test]
fn an_invalid_mode_or_folder_is_reported_before_an_invalid_work_choice() {
    let app = app_dir("structural-before-choices");
    let invalid_choice = Options {
        meshes_optimization_level: 4,
        ..single_mod_options(&app)
    };

    let mode = Options {
        mode: OptimizationMode::Unsupported(2),
        ..invalid_choice.clone()
    };
    let error = run_request("SSE", &mode, None).unwrap_err();
    assert!(
        matches!(error, RunSetupError::UnsupportedMode(2)),
        "{error:?}"
    );

    let folder = Options {
        user_path: "mods/Mod".into(),
        ..invalid_choice
    };
    let error = run_request("SSE", &folder, None).unwrap_err();
    assert!(
        matches!(error, RunSetupError::RelativeSelection(_)),
        "{error:?}"
    );
}

#[test]
fn an_odd_target_size_is_accepted_while_resizing_by_size_is_off() {
    // Deviation 18: C++ rejected an odd width even when it was never used.
    let app = app_dir("odd-size-unused");
    let options = Options {
        textures_resize_size: false,
        textures_target_width: 513,
        ..single_mod_options(&app)
    };
    assert!(run_request("SSE", &options, None).is_ok());
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

    let request = run_request("FO4", &options, None).unwrap();

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

/// Core cannot see the options model, so this pins core's default Work
/// Choices to an untouched options form.
#[test]
fn a_default_options_form_carries_the_default_work_choices() {
    let app = app_dir("default-work-choices");
    let options = single_mod_options(&app);

    let request = run_request("SSE", &options, None).unwrap();

    assert_eq!(request.work_choices(), &WorkChoices::default());
}

#[test]
fn the_request_carries_the_forms_work_choices_with_ratio_winning() {
    let app = app_dir("chosen-work-choices");
    let options = Options {
        textures_necessary: false,
        textures_compress: true,
        textures_mipmaps: true,
        // Both resize modes are on: ratio wins, as in C++.
        textures_resize_ratio: true,
        textures_target_width_ratio: 2,
        textures_target_height_ratio: 4,
        textures_resize_size: true,
        textures_target_width: 1024,
        textures_target_height: 512,
        meshes_optimization_level: 2,
        meshes_headparts: false,
        meshes_resave: true,
        bsa_delete_backup: true,
        bsa_compress: false,
        bsa_delete_source: false,
        bsa_create_dummies: false,
        bsa_merge_incompressible: false,
        bsa_merge_textures: true,
        ..single_mod_options(&app)
    };

    let request = run_request("SSE", &options, None).unwrap();

    assert_eq!(
        request.work_choices(),
        &WorkChoices {
            textures: TextureChoices {
                necessary: false,
                compress: true,
                mipmaps: true,
                resize: TextureResizeChoice::Ratio(ResizeRatio::new(2, 4).unwrap()),
            },
            meshes: MeshChoices {
                level: MeshLevel::new(2).unwrap(),
                headparts: false,
                resave: true,
            },
            archive_extraction: SourceCleanup::Remove,
            archive_finalization: ArchiveFinalizationChoices {
                compress: false,
                delete_sources: false,
                create_dummy_plugins: false,
                merge_incompressible: false,
                merge_textures: true,
            },
        }
    );
}

#[test]
fn an_odd_target_size_is_accepted_when_resizing_by_ratio_wins() {
    // Deviation 18: C++ checked the size whenever resizing by size was on,
    // although ratio wins and the size is never used.
    let app = app_dir("odd-size-ratio-wins");
    let options = Options {
        textures_resize_ratio: true,
        textures_target_width_ratio: 2,
        textures_target_height_ratio: 2,
        textures_resize_size: true,
        textures_target_width: 513,
        ..single_mod_options(&app)
    };

    let request = run_request("SSE", &options, None).unwrap();

    assert_eq!(
        request.work_choices().textures.resize,
        TextureResizeChoice::Ratio(ResizeRatio::new(2, 2).unwrap())
    );
}

#[test]
fn resizing_by_size_alone_carries_the_target_size() {
    let app = app_dir("size-work-choice");
    let options = Options {
        textures_resize_size: true,
        textures_target_width: 1024,
        textures_target_height: 512,
        ..single_mod_options(&app)
    };

    let request = run_request("SSE", &options, None).unwrap();

    assert_eq!(
        request.work_choices().textures.resize,
        TextureResizeChoice::Size(ResizeSize::new(1024, 512).unwrap())
    );
}

#[test]
fn disabled_texture_work_does_not_request_profile_conversion() {
    let app = app_dir("no-texture-work");
    let options = Options {
        textures_necessary: false,
        ..single_mod_options(&app)
    };
    let request = run_request("SSE", &options, None).unwrap();
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
        assert!(run_request("SSE", &options, None).is_ok());
        options.dry_run = false;
        assert!(run_request("SSE", &options, None).is_ok());
    }

    // Animations are ported (#502), in both modes.
    let mut options = dry_run_textures(&app, &app.join("mods/Mod"));
    options.animations_optimization = true;
    assert!(run_request("SSE", &options, None).is_ok());
    options.dry_run = false;
    assert!(run_request("SSE", &options, None).is_ok());

    // Archive creation is ported (#498), in both modes.
    let mut options = dry_run_textures(&app, &app.join("mods/Mod"));
    options.bsa_create = true;
    assert!(
        run_request("SSE", &options, None).is_ok(),
        "Dry Run never packs"
    );
    options.dry_run = false;
    assert!(run_request("SSE", &options, None).is_ok());
}

/// Origin: ApplicationRunSetupTests::archiveCreationRequiresProfileArchiveSupport
/// and the FO4 Profile Capabilities. A request the selected profile cannot
/// honour is not a Start Error: building it reads no profile, so setup
/// accepts it, and the run fails Preparing with a Policy Conflict Run
/// Failure.
///
/// A choice an Admission Rule held back never reaches that judgement (spec
/// #634): a Dry Run's request carries no Archive creation, so a Dry Run with
/// Create BSA under a profile without Archive support prepares and succeeds.
#[test]
fn a_request_contradicting_the_profile_capabilities_fails_preparing() {
    let _serial = serial();
    let app = app_dir("capability-conflict");
    let mod_root = app.join("mods").join("Mod");

    // FO4 has no Mesh support: any Mesh work contradicts it. Built without
    // Profile Capabilities, the request holds none of it back.
    let options = fo4_mesh_options(&app);
    let request = run_request("FO4", &options, None).unwrap();
    assert!(request.requests(RequestedWork::StandardMeshOptimization));
    let result = run_service(&app).start(request, None).unwrap().wait();
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

    // A Dry Run never packs, so its request holds Archive creation back and
    // a profile with no Archive support has nothing to contradict.
    edit_profile(&app, "SSE", "bsaEnabled", "false");
    let mut options = dry_run_textures(&app, &mod_root);
    options.bsa_create = true;
    assert!(
        !run_request("SSE", &options, None)
            .unwrap()
            .requests(RequestedWork::ArchiveCreation)
    );
    let result = start_and_wait(&run_service(&app), "SSE", &options);
    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    let policy = result.preparation().expect("the run prepares").policy();
    assert!(!policy.requests(RequestedWork::ArchiveCreation));
}

/// Spec #634: a request built with the Selected Profile's capabilities, as
/// the GUI builds it from the `profile.ini` it loaded, holds back the work
/// they do not support. FO4 has no Mesh support, so the same Mesh options
/// that fail Preparing without capabilities request no Mesh work, and the
/// run succeeds.
#[test]
fn a_request_built_with_the_profile_capabilities_holds_back_unsupported_work() {
    let _serial = serial();
    let app = app_dir("capability-admission");
    let options = fo4_mesh_options(&app);
    let settings = Profiles::new(&app).open("FO4").load_settings().unwrap();
    let capabilities = profile_capabilities(&settings);

    let request = run_request("FO4", &options, Some(&capabilities)).unwrap();

    for mesh_work in [
        RequestedWork::StandardMeshOptimization,
        RequestedWork::TerrainMeshOptimization,
    ] {
        assert!(!request.requests(mesh_work), "{mesh_work:?}");
    }
    assert!(request.requests(RequestedWork::NativeTextureOptimization));
    let result = run_service(&app).start(request, None).unwrap().wait();
    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    let policy = result.preparation().expect("the run prepares").policy();
    assert!(!policy.requests(RequestedWork::StandardMeshOptimization));
    assert_eq!(result.asset_attempts().len(), 1);
}

/// The Mesh options FO4 cannot honour, shared by
/// [`a_request_contradicting_the_profile_capabilities_fails_preparing`] and
/// [`a_request_built_with_the_profile_capabilities_holds_back_unsupported_work`]
/// so the pair differ only in the capabilities `run_request` is given (#636):
/// a Dry Run over `app/mods/Mod`, which holds one plain DDS, with Full Mesh
/// optimization and resaving on.
fn fo4_mesh_options(app: &Path) -> Options {
    let mod_root = app.join("mods").join("Mod");
    write_dds(
        &mod_root.join("textures/plain.dds"),
        DXGI_FORMAT_R8G8B8A8_UNORM,
        16,
    );
    let mut options = dry_run_textures(app, &mod_root);
    options.meshes_optimization_level = 3;
    options.meshes_resave = true;
    options
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
    assert!(
        !run_request("FO4", &options, None)
            .unwrap()
            .requests(RequestedWork::StandardMeshOptimization)
    );

    let result = start_and_wait(&run_service(&app), "FO4", &options);

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

    let result = start_and_wait(&run_service(&app), "SSE", &options);

    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    let root = cao_winfs::canonical(&mods).unwrap();
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

/// The Run Request `run_request` builds under the profile identity `profile`
/// from the options saved in the `settings.ini` of the profile
/// `saved_options_profile`, which need not be `profile`.
fn saved_request(app: &Path, saved_options_profile: &str, profile: &str) -> RunRequest {
    run_request(profile, &profile_options(app, saved_options_profile), None).unwrap()
}

/// The default options form over the single mod folder `app/mods/Mod`.
fn single_mod_options(app: &Path) -> Options {
    Options {
        user_path: app.join("mods/Mod").to_string_lossy().into_owned(),
        ..Options::default()
    }
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

    // Built from the app directory alone: the run's intent comes from the
    // request each load is given.
    let provider = ProfileConfigurationProvider::new(&app);

    // FO4 has no ignoredMods.txt of its own, so it falls back to SSE's.
    // The contract's return type guarantees the Toolset is built.
    let (profile, _toolset) = provider.load(&saved_request(&app, "FO4", "FO4")).unwrap();

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

    // The profile read is the one the request names.
    assert!(
        provider
            .load(&saved_request(&app, "FO4", "MissingProfile"))
            .is_err()
    );
}

/// Each load reads the profile afresh, as each run's Preparing does, so an
/// edit between two loads is in the second Selected Profile.
#[test]
fn the_provider_loads_an_updated_selected_profile() {
    let app = app_dir("provider-updated");
    let provider = ProfileConfigurationProvider::new(&app);
    let request = saved_request(&app, "FO4", "FO4");
    let (first, _) = provider.load(&request).unwrap();
    assert!(first.prefers_conversion());

    // Edited between two loads, as between two runs' Preparing.
    edit_profile(&app, "FO4", "bsaGame", "4");
    edit_profile(&app, "FO4", "bsaEnabled", "false");
    edit_profile(&app, "FO4", "texturesConvertTga", "false");
    edit_profile(&app, "FO4", "meshesEnabled", "true");
    let (second, _) = provider.load(&request).unwrap();

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
    let provider = ProfileConfigurationProvider::new(&app);
    assert!(provider.load(&saved_request(&app, "FO4", "FO4")).is_err());
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

    let result = start_and_wait(
        &run_service(&app),
        "SSE",
        &dry_run_textures(&app, &mod_root),
    );

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
    let service = run_service(&app);
    let request = run_request("SSE", &dry_run_textures(&app, &mod_root), None).unwrap();

    let first = service.start(request.clone(), None).unwrap().wait();
    assert_eq!(first.outcome(), RunOutcome::Succeeded);

    // The request and the service were built before the edit; only Preparing
    // can see it.
    edit_profile(&app, "SSE", "texturesEnabled", "false");
    let second = service.start(request, None).unwrap().wait();
    assert_eq!(second.outcome(), RunOutcome::Failed);
    let failure = &second.failures()[0];
    assert_eq!(failure.code, RunFailureCode::PolicyConflict);
    assert_eq!(failure.phase, RunPhase::Preparing);
}

/// Spec (#618): one production service serves a session. Runs started on it
/// in turn, each with different Work Choices, each do the work their own
/// request chose, not the choices of the run before.
#[test]
fn one_service_runs_each_request_with_its_own_work_choices() {
    let _serial = serial();
    let app = app_dir("one-service-work-choices");
    let service = run_service(&app);
    let first_root = app.join("mods").join("First");
    let second_root = app.join("mods").join("Second");
    for mod_root in [&first_root, &second_root] {
        write_dds(
            &mod_root.join("textures/plain.dds"),
            DXGI_FORMAT_R8G8B8A8_UNORM,
            64,
        );
    }

    // TES5's BC3 is encoded on the CPU on every host.
    let mut options = profile_options(&app, "TES5");
    options.dry_run = false;
    options.textures_necessary = true;
    options.textures_compress = true;
    options.textures_resize_size = true;
    options.textures_resize_ratio = false;
    options.meshes_optimization_level = 0;
    options.meshes_resave = false;
    options.animations_optimization = false;
    options.bsa_extract = false;
    options.bsa_create = false;

    options.user_path = first_root.to_string_lossy().into_owned();
    options.textures_mipmaps = false;
    options.textures_target_width = 32;
    options.textures_target_height = 32;
    let first = start_and_wait(&service, "TES5", &options);

    options.user_path = second_root.to_string_lossy().into_owned();
    options.textures_mipmaps = true;
    options.textures_target_width = 16;
    options.textures_target_height = 16;
    let second = start_and_wait(&service, "TES5", &options);

    for result in [&first, &second] {
        assert_eq!(
            result.outcome(),
            RunOutcome::Succeeded,
            "{:?}",
            result.asset_attempts()
        );
    }
    let first_info = dds_metadata(&first_root.join("textures/plain.dds"));
    assert_eq!(
        (first_info.width, first_info.height, first_info.mip_levels),
        (32, 32, 1)
    );
    // 16, 8, 4, 2, 1.
    let second_info = dds_metadata(&second_root.join("textures/plain.dds"));
    assert_eq!(
        (
            second_info.width,
            second_info.height,
            second_info.mip_levels
        ),
        (16, 16, 5)
    );
}

/// Spec (#618): runs under two profiles in turn on one production service
/// each prepare with their own profile's Selected Profile.
#[test]
fn one_service_prepares_each_run_with_its_own_profile() {
    let _serial = serial();
    let app = app_dir("one-service-two-profiles");
    let mod_root = app.join("mods").join("Mod");
    std::fs::create_dir_all(&mod_root).unwrap();
    let service = run_service(&app);
    let options = dry_run_textures(&app, &mod_root);

    let sse = start_and_wait(&service, "SSE", &options);
    let fo4 = start_and_wait(&service, "FO4", &options);

    for result in [&sse, &fo4] {
        assert_eq!(
            result.outcome(),
            RunOutcome::Succeeded,
            "{:?}",
            result.failures()
        );
    }
    // The shipped SSE profile enables every kind of work.
    assert_eq!(
        sse.preparation().unwrap().selected_profile().capabilities(),
        &ProfileCapabilities::define(
            ".bsa",
            &[
                ProfileCapability::NativeTextureOptimization,
                ProfileCapability::ConvertibleTextureConversion,
                ProfileCapability::StandardMeshOptimization,
                ProfileCapability::TerrainMeshOptimization,
                ProfileCapability::AnimationOptimization,
                ProfileCapability::ArchiveExtraction,
                ProfileCapability::MeshReferenceMaintenance,
                ProfileCapability::ArchiveCreation,
            ]
        )
    );
    // FO4 packs `.ba2` Archives and has neither Meshes nor Animations.
    assert_eq!(
        fo4.preparation().unwrap().selected_profile().capabilities(),
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
}

/// Spec (#628): a run packs at its Profile's Archive Limit. An SSE Profile
/// asking for 1000-byte Archives splits three 400-byte Meshes into one
/// Archive of two and a counter-named Archive of the third, where the shipped
/// ~1.96 GiB limit would pack all three together.
#[test]
fn a_run_under_a_small_archive_limit_splits_its_archives_at_that_limit() {
    let _serial = serial();
    let app = app_dir("small-archive-limit");
    edit_profile(&app, "SSE", "maxBsaUncompressedSize", "1000");
    let root = canonical(&{
        let root = app.join("mods").join("Mod");
        std::fs::create_dir_all(&root).unwrap();
        root
    });
    for name in ["a", "b", "c"] {
        write(&root, &format!("meshes/{name}.nif"), &[b'm'; 400]);
    }

    let mut options = profile_options(&app, "SSE");
    options.dry_run = false;
    options.user_path = root.to_string_lossy().into_owned();
    options.textures_necessary = false;
    options.textures_compress = false;
    options.textures_mipmaps = false;
    options.textures_resize_ratio = false;
    options.textures_resize_size = false;
    options.meshes_optimization_level = 0;
    options.meshes_resave = false;
    options.animations_optimization = false;
    options.bsa_extract = false;
    options.bsa_create = true;
    let result = start_and_wait(&run_service(&app), "SSE", &options);

    assert_eq!(result.outcome(), RunOutcome::Succeeded, "{result:?}");
    let finalization = result.archive_finalization().unwrap();
    let archives: Vec<_> = finalization
        .attempts
        .iter()
        .map(|attempt| attempt.archive_path.clone())
        .collect();
    assert_eq!(archives, [root.join("Mod.bsa"), root.join("Mod0.bsa")]);
    let packed: Vec<usize> = archives
        .iter()
        .map(|archive| {
            ReadArchive::open(archive)
                .unwrap()
                .expect("a known Archive format")
                .archived_assets()
                .unwrap()
                .len()
        })
        .collect();
    // 400 + 400 fills the first Archive to 800 bytes; a third would take it
    // past 1000.
    assert_eq!(packed, [2, 1]);
}

/// The metadata of the DDS file at `path`.
fn dds_metadata(path: &Path) -> TexMetadata {
    let bytes = std::fs::read(path).unwrap();
    let mut info = TexMetadata::default();
    ScratchImage::load_dds(&bytes, DDS_FLAGS_NONE, Some(&mut info), None).unwrap();
    info
}

/// An 8×8 TGA, small enough to convert quickly.
///
/// Passing the metadata makes DirectXTex write a TGA 2.0 extension area
/// stamped with the current time to the second, so two calls can differ.
/// A test that checks a file is unchanged must compare against the bytes it
/// wrote, not against a second call.
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
    let tga = tga_bytes();
    write(&mod_root, "textures/armor.tga", &tga);
    write(&mod_root, "meshes/armor.nif", b"not a mesh");
    // The shipped SSE profile prefers conversion when the run is set up.
    let service = run_service(&app);
    let options = apply_necessary_textures(&app, &mod_root);

    edit_profile(&app, "SSE", "texturesConvertTga", "false");
    let result = start_and_wait(&service, "SSE", &options);

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    assert_eq!(
        std::fs::read(mod_root.join("textures/armor.tga")).unwrap(),
        tga
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
    let service = run_service(&app);
    let options = apply_necessary_textures(&app, &mod_root);

    edit_profile(&app, "SSE", "texturesConvertTga", "true");
    let result = start_and_wait(&service, "SSE", &options);

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
    let service = run_service(&app);
    // Deviation 11: an unsupported game makes the profile unreadable.
    edit_profile(&app, "SSE", "bsaGame", "7");

    let result = start_and_wait(&service, "SSE", &options);

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert_eq!(
        result.failures()[0].code,
        RunFailureCode::ConfigurationLoadingFailed
    );

    // Unreadable before setup too: setup never reads the profile, even for
    // Texture work, so Preparing reports it.
    let result = start_and_wait(&service, "MissingProfile", &options);
    assert_eq!(
        result.failures()[0].code,
        RunFailureCode::ConfigurationLoadingFailed
    );
}

#[test]
fn each_shipped_profiles_mesh_target_is_its_games_nifly_version() {
    use cao_optimizers::profile::mesh_target;
    use cao_profiles::{
        MeshFileVersion, MeshStreamVersion, MeshUserVersion, ProfileSettings, Profiles,
    };
    use nifly_sys::NifVersion;

    let app = app_dir("profile-mesh-targets");
    let profiles = Profiles::new(&app);
    for (name, expected) in [
        ("TES5", NifVersion::SK),
        ("SSE", NifVersion::SSE),
        ("FO4", NifVersion::FO4),
    ] {
        let settings = profiles.open(name).load_settings().unwrap();
        assert_eq!(mesh_target(&settings), expected, "{name}");
    }

    // Oblivion's numbers, which no shipped Profile uses.
    let oblivion = ProfileSettings {
        meshes_file_version: MeshFileVersion::V20_0_0_5,
        meshes_user: MeshUserVersion::V11,
        meshes_stream: MeshStreamVersion::V82,
        ..profiles.open("SSE").load_settings().unwrap()
    };
    assert_eq!(
        mesh_target(&oblivion),
        NifVersion {
            file: 0x1400_0005,
            user: 11,
            stream: 82,
        }
    );
}

#[test]
fn a_profile_with_an_out_of_set_mesh_target_fails_preparing_with_the_message_the_gui_shows() {
    let _serial = serial();
    let app = app_dir("profile-mesh-user-out-of-set");
    let mod_root = app.join("mods").join("Mod");
    std::fs::create_dir_all(&mod_root).unwrap();
    let options = dry_run_textures(&app, &mod_root);
    let service = run_service(&app);
    edit_profile(&app, "SSE", "meshesUser", "13");

    let result = start_and_wait(&service, "SSE", &options);

    assert_eq!(result.outcome(), RunOutcome::Failed);
    let failure = &result.failures()[0];
    assert_eq!(failure.code, RunFailureCode::ConfigurationLoadingFailed);
    // The GUI shows the same `ProfileError` after "could not be read: ".
    assert_eq!(
        failure.detail,
        "`profile.ini`: `Meshes/meshesUser` is `13`; expected one of 11, 12"
    );
}

#[test]
fn a_profile_with_an_out_of_set_texture_target_fails_preparing_with_the_message_the_gui_shows() {
    let _serial = serial();
    let app = app_dir("profile-texture-format-out-of-set");
    let mod_root = app.join("mods").join("Mod");
    std::fs::create_dir_all(&mod_root).unwrap();
    let options = dry_run_textures(&app, &mod_root);
    let service = run_service(&app);
    edit_profile(&app, "SSE", "texturesFormat", "99");

    let result = start_and_wait(&service, "SSE", &options);

    assert_eq!(result.outcome(), RunOutcome::Failed);
    let failure = &result.failures()[0];
    assert_eq!(failure.code, RunFailureCode::ConfigurationLoadingFailed);
    // The GUI shows the same `ProfileError` after "could not be read: ".
    assert_eq!(
        failure.detail,
        "`profile.ini`: `Textures/texturesFormat` is `99`; expected one of 98 (BC7_UNORM), \
         83 (BC5_UNORM), 77 (BC3_UNORM), 71 (BC1_UNORM), 28 (R8G8B8A8_UNORM)"
    );
}
