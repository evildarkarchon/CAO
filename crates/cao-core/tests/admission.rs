//! Admission Rules: which of the user's choices a run's Run Request may
//! carry, given its Mod Selection and, optionally, its Profile Capabilities.
//! The execution mode holds back nothing, so a Dry Run's request keeps its
//! Archive work.

use cao_core::admission::Admission;
use cao_core::routing::{ExecutionMode, ProfileCapabilities, ProfileCapability, RequestedWork};
use cao_core::run::{
    ArchiveFinalizationChoices, MeshChoices, MeshLevel, ModSelection, RunRequest, SourceCleanup,
    TextureChoices, TextureResizeChoice, WorkChoices,
};
use cao_core::test_support::sse_capabilities;

/// Every piece of requested work, in enumeration order.
const ALL_WORK: [RequestedWork; 6] = [
    RequestedWork::NativeTextureOptimization,
    RequestedWork::StandardMeshOptimization,
    RequestedWork::TerrainMeshOptimization,
    RequestedWork::AnimationOptimization,
    RequestedWork::ArchiveExtraction,
    RequestedWork::ArchiveCreation,
];

/// The work `admission` holds back, in enumeration order.
fn held_back(admission: &Admission) -> Vec<RequestedWork> {
    ALL_WORK
        .into_iter()
        .filter(|work| !admission.admits(*work))
        .collect()
}

/// With no Profile Capabilities known, no rule holds back requested work,
/// whichever the Mod Selection.
#[test]
fn without_profile_capabilities_every_piece_of_work_is_admitted() {
    for several_mods in [false, true] {
        let admission = Admission::new(several_mods, None);

        assert_eq!(held_back(&admission), [], "several mods: {several_mods}");
    }
}

#[test]
fn several_mods_admits_mesh_levels_up_to_necessary() {
    let admission = Admission::new(true, None);

    assert_eq!(admission.highest_mesh_level().get(), 1);
}

#[test]
fn one_mod_admits_every_mesh_level() {
    let capabilities = partial_capabilities();
    for capabilities in [None, Some(&capabilities)] {
        let admission = Admission::new(false, capabilities);

        assert_eq!(admission.highest_mesh_level().get(), 3);
    }
}

/// Work Choices away from every default, at Mesh level `level`.
fn choices_at(level: i32) -> WorkChoices {
    WorkChoices {
        textures: TextureChoices {
            necessary: false,
            compress: true,
            mipmaps: true,
            resize: TextureResizeChoice::None,
        },
        meshes: MeshChoices {
            level: MeshLevel::new(level).unwrap(),
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
}

#[test]
fn several_mods_caps_the_chosen_mesh_level_at_necessary_and_keeps_the_rest() {
    let admission = Admission::new(true, None);

    for (chosen, admitted) in [(0, 0), (1, 1), (2, 1), (3, 1)] {
        let mut expected = choices_at(chosen);
        expected.meshes.level = MeshLevel::new(admitted).unwrap();

        assert_eq!(
            admission.admitted_work_choices(choices_at(chosen)),
            expected,
            "level {chosen}"
        );
    }
}

#[test]
fn one_mod_keeps_the_chosen_mesh_level() {
    let admission = Admission::new(false, None);

    assert_eq!(
        admission.admitted_work_choices(choices_at(3)),
        choices_at(3)
    );
}

#[test]
fn the_admitted_work_is_the_chosen_work_less_what_is_held_back() {
    let capabilities = partial_capabilities();
    let admission = Admission::new(false, Some(&capabilities));

    assert_eq!(
        admission.admitted_work(&ALL_WORK),
        [
            RequestedWork::NativeTextureOptimization,
            RequestedWork::TerrainMeshOptimization,
            RequestedWork::ArchiveCreation,
        ]
    );
    assert_eq!(
        admission.admitted_work(&[RequestedWork::AnimationOptimization]),
        []
    );
}

/// A profile supporting native Textures, terrain Meshes and Archive creation
/// only. Conversion and Mesh Reference Maintenance are never requested work,
/// so supporting them admits nothing more.
fn partial_capabilities() -> ProfileCapabilities {
    ProfileCapabilities::define(
        ".bsa",
        &[
            ProfileCapability::NativeTextureOptimization,
            ProfileCapability::ConvertibleTextureConversion,
            ProfileCapability::TerrainMeshOptimization,
            ProfileCapability::MeshReferenceMaintenance,
            ProfileCapability::ArchiveCreation,
        ],
    )
}

#[test]
fn profile_capabilities_hold_back_exactly_the_work_they_do_not_support() {
    let capabilities = partial_capabilities();

    let admission = Admission::new(false, Some(&capabilities));

    assert_eq!(
        held_back(&admission),
        [
            RequestedWork::StandardMeshOptimization,
            RequestedWork::AnimationOptimization,
            RequestedWork::ArchiveExtraction,
        ]
    );
}

#[test]
fn a_profile_supporting_every_capability_holds_back_nothing() {
    let capabilities = sse_capabilities(".bsa");

    let admission = Admission::new(true, Some(&capabilities));

    assert_eq!(held_back(&admission), []);
}

/// A request asking for every piece of work over `selection`.
fn request(mode: ExecutionMode, selection: ModSelection) -> RunRequest {
    RunRequest::new("SSE", mode, selection, ALL_WORK.to_vec())
}

fn one_mod() -> ModSelection {
    ModSelection::SingleModRoot("C:\\mods\\Mod".into())
}

fn several_mods() -> ModSelection {
    ModSelection::ChildModRoots("C:\\mods".into())
}

/// A Dry Run requests the Archive work the user chose. No Admission Rule
/// holds it back: the run's work phases plan or skip it as Dry Run, so no
/// Archive is mutated, but the request keeps it.
#[test]
fn a_dry_run_request_built_with_archive_work_keeps_it() {
    let request = request(ExecutionMode::DryRun, one_mod());

    assert!(request.requests(RequestedWork::ArchiveExtraction));
    assert!(request.requests(RequestedWork::ArchiveCreation));
    assert_eq!(request.requested_work(), ALL_WORK);
}

/// A Dry Run keeps Delete backups and the Archive Finalization choices as
/// chosen, along with every other Work Choice.
#[test]
fn a_dry_run_request_keeps_its_work_choices_as_chosen() {
    let request = request(ExecutionMode::DryRun, one_mod()).with_work_choices(choices_at(3));

    assert_eq!(request.work_choices(), &choices_at(3));
}

#[test]
fn an_apply_request_keeps_its_archive_work() {
    let request = request(ExecutionMode::Apply, several_mods());

    assert_eq!(request.requested_work(), ALL_WORK);
}

#[test]
fn a_several_mods_request_built_with_level_3_carries_level_1() {
    let request = request(ExecutionMode::Apply, several_mods()).with_work_choices(choices_at(3));

    let mut expected = choices_at(3);
    expected.meshes.level = MeshLevel::new(1).unwrap();
    assert_eq!(request.work_choices(), &expected);
}

#[test]
fn a_one_mod_request_built_with_level_3_keeps_level_3() {
    let request = request(ExecutionMode::Apply, one_mod()).with_work_choices(choices_at(3));

    assert_eq!(request.work_choices().meshes.level.get(), 3);
}
