//! The options form into a Run Request: its option checks and their order,
//! the work it requests and the Work Choices it carries. Building a request
//! reads no file and only checks that the selected folder is absolute, so
//! these tests use a fixed absolute folder and need no app directory.

use std::path::{Path, PathBuf};

use cao_core::routing::{ExecutionMode, RequestedWork};
use cao_core::run::{
    ArchiveFinalizationChoices, MeshChoices, MeshLevel, ModSelection, ResizeRatio, ResizeSize,
    SourceCleanup, TextureChoices, TextureResizeChoice, WorkChoiceError, WorkChoices,
};
use cao_optimizers::run_request::{RunSetupError, run_request};
use cao_profiles::{OptimizationMode, Options};

/// The fixed absolute mod folder the single-mod requests select.
const MOD_ROOT: &str = "C:\\mods\\Mod";

/// The default options form over the single mod folder [`MOD_ROOT`].
fn single_mod_options() -> Options {
    Options {
        user_path: MOD_ROOT.into(),
        ..Options::default()
    }
}

/// Dry Run options over the single mod folder [`MOD_ROOT`], with every
/// Texture option on and every other kind of work off.
fn dry_run_textures() -> Options {
    Options {
        dry_run: true,
        textures_necessary: true,
        textures_compress: true,
        textures_mipmaps: true,
        ..single_mod_options()
    }
}

#[test]
fn invalid_option_values_are_rejected_before_any_run() {
    let base = single_mod_options;
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
    let invalid_choice = Options {
        meshes_optimization_level: 4,
        ..single_mod_options()
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
    // Only the resize mode the run uses is checked: a size the run never
    // resizes to cannot keep it from starting.
    let options = Options {
        textures_resize_size: false,
        textures_target_width: 513,
        ..single_mod_options()
    };
    assert!(run_request("SSE", &options, None).is_ok());
}

#[test]
fn the_request_owns_the_caller_intent() {
    let mods = PathBuf::from("C:\\mods");
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
    let options = single_mod_options();

    let request = run_request("SSE", &options, None).unwrap();

    assert_eq!(request.work_choices(), &WorkChoices::default());
}

#[test]
fn the_request_carries_the_forms_work_choices_with_ratio_winning() {
    let options = Options {
        textures_necessary: false,
        textures_compress: true,
        textures_mipmaps: true,
        // Both resize modes are on: ratio wins.
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
        ..single_mod_options()
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
    // Resizing by size is on, but ratio wins, so the size is never used and
    // is not checked.
    let options = Options {
        textures_resize_ratio: true,
        textures_target_width_ratio: 2,
        textures_target_height_ratio: 2,
        textures_resize_size: true,
        textures_target_width: 513,
        ..single_mod_options()
    };

    let request = run_request("SSE", &options, None).unwrap();

    assert_eq!(
        request.work_choices().textures.resize,
        TextureResizeChoice::Ratio(ResizeRatio::new(2, 2).unwrap())
    );
}

#[test]
fn resizing_by_size_alone_carries_the_target_size() {
    let options = Options {
        textures_resize_size: true,
        textures_target_width: 1024,
        textures_target_height: 512,
        ..single_mod_options()
    };

    let request = run_request("SSE", &options, None).unwrap();

    assert_eq!(
        request.work_choices().textures.resize,
        TextureResizeChoice::Size(ResizeSize::new(1024, 512).unwrap())
    );
}

#[test]
fn disabled_texture_work_does_not_request_profile_conversion() {
    let options = Options {
        textures_necessary: false,
        ..single_mod_options()
    };
    let request = run_request("SSE", &options, None).unwrap();
    assert!(!request.has_requested_work());
}

#[test]
fn every_kind_of_work_is_accepted() {
    // Meshes are ported (#504), at every level and when only resaving, in
    // both modes.
    for (level, resave) in [(1, false), (2, false), (3, false), (0, true)] {
        let mut options = dry_run_textures();
        options.meshes_optimization_level = level;
        options.meshes_resave = resave;
        assert!(run_request("SSE", &options, None).is_ok());
        options.dry_run = false;
        assert!(run_request("SSE", &options, None).is_ok());
    }

    // Animations are ported (#502), in both modes.
    let mut options = dry_run_textures();
    options.animations_optimization = true;
    assert!(run_request("SSE", &options, None).is_ok());
    options.dry_run = false;
    assert!(run_request("SSE", &options, None).is_ok());

    // Archive creation is ported (#498), in both modes.
    let mut options = dry_run_textures();
    options.bsa_create = true;
    assert!(
        run_request("SSE", &options, None).is_ok(),
        "Dry Run never packs"
    );
    options.dry_run = false;
    assert!(run_request("SSE", &options, None).is_ok());
}
