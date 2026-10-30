//! The options form into a Run Request: its option checks and their order,
//! the work it requests and the Work Choices it carries. Building a request
//! reads no file and only checks that the selected folder is absolute, so
//! these tests use a fixed absolute folder and need no app directory.
//! They also pin the form's work groups, which options choose each and when
//! a group is held back, and the Admission the module builds from a mode.

use std::path::{Path, PathBuf};

use cao_core::routing::{ExecutionMode, ProfileCapabilities, ProfileCapability, RequestedWork};
use cao_core::run::{
    ArchiveFinalizationChoices, MeshChoices, MeshLevel, ModSelection, ResizeRatio, ResizeSize,
    SourceCleanup, TextureChoices, TextureResizeChoice, WorkChoiceError, WorkChoices,
};
use cao_optimizers::run_request::{RunSetupError, WorkGroup, admission, run_request};
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

/// Every Requested Work, in [`work_position`] order.
const ALL_WORK: [RequestedWork; 6] = [
    RequestedWork::NativeTextureOptimization,
    RequestedWork::StandardMeshOptimization,
    RequestedWork::TerrainMeshOptimization,
    RequestedWork::AnimationOptimization,
    RequestedWork::ArchiveExtraction,
    RequestedWork::ArchiveCreation,
];

/// The index of `work` in [`ALL_WORK`]. The match has no wildcard arm, so a
/// new Requested Work does not compile here until it is listed in
/// [`ALL_WORK`] too, and the work-group tests then cover it.
fn work_position(work: RequestedWork) -> usize {
    match work {
        RequestedWork::NativeTextureOptimization => 0,
        RequestedWork::StandardMeshOptimization => 1,
        RequestedWork::TerrainMeshOptimization => 2,
        RequestedWork::AnimationOptimization => 3,
        RequestedWork::ArchiveExtraction => 4,
        RequestedWork::ArchiveCreation => 5,
    }
}

#[test]
fn every_requested_work_belongs_to_exactly_one_work_group() {
    for (position, work) in ALL_WORK.into_iter().enumerate() {
        assert_eq!(
            work_position(work),
            position,
            "{work:?} is listed out of place"
        );
        let groups: Vec<WorkGroup> = WorkGroup::ALL
            .into_iter()
            .filter(|group| group.work().contains(&work))
            .collect();
        assert_eq!(groups.len(), 1, "{work:?} belongs to {groups:?}");
    }
    // Mesh Reference Maintenance and Texture conversion are no Requested
    // Work, so no group can ask for them.
    let grouped: usize = WorkGroup::ALL.iter().map(|group| group.work().len()).sum();
    assert_eq!(grouped, ALL_WORK.len());
}

#[test]
fn each_work_group_asks_for_its_own_requested_work() {
    use RequestedWork::*;
    let expected: [(WorkGroup, &[RequestedWork]); 5] = [
        (WorkGroup::Textures, &[NativeTextureOptimization]),
        (
            WorkGroup::Meshes,
            &[StandardMeshOptimization, TerrainMeshOptimization],
        ),
        (WorkGroup::Animations, &[AnimationOptimization]),
        (WorkGroup::ArchiveExtraction, &[ArchiveExtraction]),
        (WorkGroup::ArchiveCreation, &[ArchiveCreation]),
    ];
    assert_eq!(WorkGroup::ALL, expected.map(|(group, _)| group));
    for (group, work) in expected {
        assert_eq!(group.work(), work, "{group:?}");
    }
}

/// One change to an options form.
type Change = fn(&mut Options);

/// The groups `options` choose, in [`WorkGroup::ALL`] order.
fn chosen(options: &Options) -> Vec<WorkGroup> {
    WorkGroup::ALL
        .into_iter()
        .filter(|group| group.chosen_by(options))
        .collect()
}

#[test]
fn every_texture_option_chooses_textures_alone() {
    let none = Options {
        textures_necessary: false,
        ..single_mod_options()
    };
    assert_eq!(chosen(&none), []);
    let changes: [Change; 5] = [
        |o| o.textures_necessary = true,
        |o| o.textures_compress = true,
        |o| o.textures_mipmaps = true,
        |o| o.textures_resize_size = true,
        |o| o.textures_resize_ratio = true,
    ];
    for change in changes {
        let mut options = none.clone();
        change(&mut options);
        assert_eq!(chosen(&options), [WorkGroup::Textures]);
    }
}

#[test]
fn a_mesh_level_above_none_or_resaving_chooses_meshes() {
    let base = Options {
        textures_necessary: false,
        ..single_mod_options()
    };
    for (level, resave, expected) in [
        // Head parts are on by default and ask for no Mesh work by themselves.
        (0, false, &[][..]),
        (1, false, &[WorkGroup::Meshes][..]),
        (2, false, &[WorkGroup::Meshes]),
        (3, false, &[WorkGroup::Meshes]),
        (0, true, &[WorkGroup::Meshes]),
    ] {
        let options = Options {
            meshes_optimization_level: level,
            meshes_resave: resave,
            ..base.clone()
        };
        assert_eq!(chosen(&options), expected, "level {level}, resave {resave}");
    }
}

#[test]
fn the_animation_and_archive_options_each_choose_their_group() {
    let base = Options {
        textures_necessary: false,
        ..single_mod_options()
    };
    let cases: [(Change, WorkGroup); 3] = [
        (|o| o.animations_optimization = true, WorkGroup::Animations),
        (|o| o.bsa_extract = true, WorkGroup::ArchiveExtraction),
        (|o| o.bsa_create = true, WorkGroup::ArchiveCreation),
    ];
    for (change, group) in cases {
        let mut options = base.clone();
        change(&mut options);
        assert_eq!(chosen(&options), [group]);
    }
    // Delete backups is a choice of Archive extraction, not a group of its own.
    let delete_backup = Options {
        bsa_delete_backup: true,
        ..base
    };
    assert_eq!(chosen(&delete_backup), []);
}

#[test]
fn the_admission_of_several_mods_caps_the_mesh_level_at_necessary() {
    assert_eq!(
        admission(OptimizationMode::SeveralMods, None).highest_mesh_level(),
        MeshLevel::NECESSARY
    );
    assert_eq!(
        admission(OptimizationMode::SingleMod, None).highest_mesh_level(),
        MeshLevel::FULL
    );
}

#[test]
fn an_unsupported_mode_is_admitted_as_not_several_mods() {
    assert_eq!(
        admission(OptimizationMode::Unsupported(7), None),
        admission(OptimizationMode::SingleMod, None)
    );
    assert_eq!(
        admission(OptimizationMode::Unsupported(7), None).highest_mesh_level(),
        MeshLevel::FULL
    );
}

#[test]
fn the_admission_holds_back_the_work_the_capabilities_do_not_support() {
    let capabilities =
        ProfileCapabilities::define(".ba2", &[ProfileCapability::NativeTextureOptimization]);
    let known = admission(OptimizationMode::SingleMod, Some(&capabilities));
    let admitted: Vec<RequestedWork> = ALL_WORK
        .into_iter()
        .filter(|work| known.admits(*work))
        .collect();
    assert_eq!(admitted, [RequestedWork::NativeTextureOptimization]);
    // Unknown capabilities hold back nothing: Preparing judges the Profile.
    let unknown = admission(OptimizationMode::SingleMod, None);
    assert!(ALL_WORK.into_iter().all(|work| unknown.admits(work)));
}

/// Profile Capabilities supporting Standard but not Terrain Meshes, and no
/// other work.
fn standard_meshes_only() -> ProfileCapabilities {
    ProfileCapabilities::define(".bsa", &[ProfileCapability::StandardMeshOptimization])
}

/// The groups held back under `capabilities`, in [`WorkGroup::ALL`] order.
fn held_back(capabilities: Option<&ProfileCapabilities>) -> Vec<WorkGroup> {
    let rules = admission(OptimizationMode::SingleMod, capabilities);
    WorkGroup::ALL
        .into_iter()
        .filter(|group| group.held_back(&rules))
        .collect()
}

#[test]
fn a_partly_supported_group_is_not_held_back() {
    assert_eq!(
        held_back(Some(&standard_meshes_only())),
        [
            WorkGroup::Textures,
            WorkGroup::Animations,
            WorkGroup::ArchiveExtraction,
            WorkGroup::ArchiveCreation,
        ]
    );
    let terrain_only =
        ProfileCapabilities::define(".bsa", &[ProfileCapability::TerrainMeshOptimization]);
    assert!(!held_back(Some(&terrain_only)).contains(&WorkGroup::Meshes));
}

#[test]
fn a_group_is_held_back_when_none_of_its_work_is_supported() {
    let nothing = ProfileCapabilities::define(".bsa", &[]);
    assert_eq!(held_back(Some(&nothing)), WorkGroup::ALL);
    // Capabilities outside every group's work hold back the same.
    let outside = ProfileCapabilities::define(
        ".bsa",
        &[
            ProfileCapability::ConvertibleTextureConversion,
            ProfileCapability::MeshReferenceMaintenance,
        ],
    );
    assert_eq!(held_back(Some(&outside)), WorkGroup::ALL);
}

#[test]
fn no_group_is_held_back_when_every_work_is_supported_or_none_is_known() {
    let every = ProfileCapabilities::define(
        ".bsa",
        &[
            ProfileCapability::NativeTextureOptimization,
            ProfileCapability::StandardMeshOptimization,
            ProfileCapability::TerrainMeshOptimization,
            ProfileCapability::AnimationOptimization,
            ProfileCapability::ArchiveExtraction,
            ProfileCapability::ArchiveCreation,
        ],
    );
    assert_eq!(held_back(Some(&every)), []);
    assert_eq!(held_back(None), []);
}

#[test]
fn a_request_on_a_partly_supported_group_carries_only_its_supported_work() {
    let options = Options {
        textures_necessary: false,
        meshes_optimization_level: 1,
        ..single_mod_options()
    };
    let request = run_request("TES5", &options, Some(&standard_meshes_only())).unwrap();
    assert_eq!(
        request.requested_work(),
        [RequestedWork::StandardMeshOptimization]
    );
}

#[test]
fn a_form_choosing_every_group_requests_every_work() {
    let options = Options {
        meshes_resave: true,
        animations_optimization: true,
        bsa_extract: true,
        bsa_create: true,
        ..single_mod_options()
    };
    let request = run_request("SSE", &options, None).unwrap();
    assert_eq!(request.requested_work(), ALL_WORK);
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
