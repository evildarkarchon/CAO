//! What the window's rules show, in one place: which controls they force, how
//! they force them, and which they disable.
//!
//! **The Admission Rules.** The window asks the `run_request` module the
//! questions a run asks. [`admission`] builds the window's [`Admission`]
//! through [`run_request::admission`], from the mode the mode combo shows and
//! the loaded Profile's Profile Capabilities, the same build a run's Run
//! Request uses, so the window and the run derive Several Mods in one place.
//! The window shows its answer:
//!
//! - **Several Mods** caps the Mesh level at Necessary, so the level radios
//!   show Necessary, with Medium and Full disabled. This is a Work Choice cap,
//!   not a work group: "Process meshes" stays the user's, so level 0 (no Mesh
//!   work) stays allowed.
//! - **A Profile Capability the Profile lacks** holds back the work that
//!   needs it. The window asks which [`WorkGroup`]s are held back (none of
//!   their work admitted) and maps only its own layout onto the answer:
//!   - A tab is disabled when every work group it shows is held back
//!     ([`Tab::enabled_under`]). The BSA tab shows both Archive groups, so
//!     either keeps it enabled.
//!   - Extract BSA, Create BSA and Delete backups (a choice of Archive
//!     extraction) show unchecked and disabled while their Archive
//!     extraction or Archive creation group is held back.
//!
//! **A Profile CAO cannot read.** An Admission built without Profile
//! Capabilities holds back nothing and leaves the Profile to Preparing. The
//! window keeps its own rule beside that: a Profile whose `profile.ini` could
//! not be read enables no asset tab ([`form::tab_enabled`]), and its asset
//! options are saved as loaded ([`form::read_options`]).
//!
//! The Dry Run box is no Admission Rule: a Dry Run requests the Archive work
//! the user chose and mutates no Archive, so it forces no control.
//!
//! **The Archive Rules display (#628).** The Archive Rules the run will use
//! for the game in the Game combo, read from `cao_archive::Settings`:
//!
//! - **A game that keeps Textures in their own Archive** (FO4) shows "Separate
//!   textures BSA" checked and disabled, since the packer never merges that
//!   game's Textures into the Standard Archive.
//! - **The Archive Limit** is what the Maximum size spin box shows: the
//!   Profile's size, or the game's default while the Profile gives none (saved
//!   still giving none until the user edits it). A game with an Archive Size
//!   Cap (TES5 and SSE BSAs, 2 GiB) stops the spin box there, and a larger
//!   loaded size is kept as written.
//!
//! Every rule forces only what a control shows. None writes the user's choice
//! in `BsaTab` or `MeshesTab`: the choice is kept, saved as it is, and shows
//! again once the rule lifts. The run applies the same Admission Rules to what
//! was saved, so the window shows what the run will do.
//!
//! [`control_state`] is the pure part, from the admission and the Archive
//! Rules to a plain [`ControlState`]. [`apply`] builds its inputs from the
//! window's current state and writes the answer into the `RuleDisplay` global and
//! the Maximum size spin box. It is idempotent, so the same call serves a
//! click on a triggering control and a profile load.
//!
//! [`Tab::enabled_under`]: crate::form::Tab::enabled_under

use cao_archive::Settings;
use cao_core::admission::Admission;
use cao_core::routing::ProfileCapabilities;
use cao_core::run::MeshLevel;
use cao_gui_ui::{BsaTab, MainWindow, RuleDisplay};
use cao_optimizers::run_request::{self, WorkGroup};
use cao_profiles::OptimizationMode;
use slint::ComponentHandle;

use crate::form;

/// What the window's rules force the controls to show. A forced control is
/// disabled and shows its forced state; a control no field forces shows the
/// user's choice. [`ControlState::default`] forces nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ControlState {
    /// Extract BSA shows unchecked: the loaded Profile's Profile Capabilities
    /// hold back Archive extraction.
    pub(crate) extract_forced_off: bool,
    /// Create BSA shows unchecked: the loaded Profile's Profile Capabilities
    /// hold back Archive creation.
    pub(crate) create_forced_off: bool,
    /// Delete backups shows unchecked: it is a choice of the Archive
    /// extraction the loaded Profile's Profile Capabilities hold back.
    pub(crate) delete_backups_forced_off: bool,
    /// The Mesh level radios show Necessary, with Medium and Full disabled:
    /// the run's Mesh level is capped at Necessary. The "Process meshes" group
    /// is never forced, so level 0 stays the user's to choose.
    pub(crate) necessary_meshes_forced: bool,
    /// "Separate textures BSA" shows checked: the game keeps Textures in their
    /// own Archive ([`Settings::keeps_textures_separate`], FO4's DX10 BA2), so
    /// the packer keeps them separate whatever the user chose.
    pub(crate) separate_textures_forced_on: bool,
    /// The game's default Archive Limit in bytes, which the Maximum size spin
    /// box shows while the Profile gives no size; `None` for no game.
    pub(crate) default_archive_limit: Option<u64>,
    /// The game's Archive Size Cap in bytes, past which the Maximum size spin
    /// box does not go; `None` for no game or no cap.
    pub(crate) archive_size_cap: Option<u64>,
}

/// The controls the window's rules force for a run under `admission`, with
/// the Game combo showing a game whose Archive Rules are `archive_rules`
/// (`None` for no selection).
pub(crate) fn control_state(
    admission: &Admission,
    archive_rules: Option<&Settings>,
) -> ControlState {
    let extraction_held_back = WorkGroup::ArchiveExtraction.held_back(admission);
    ControlState {
        extract_forced_off: extraction_held_back,
        create_forced_off: WorkGroup::ArchiveCreation.held_back(admission),
        delete_backups_forced_off: extraction_held_back,
        necessary_meshes_forced: admission.highest_mesh_level() == MeshLevel::NECESSARY,
        separate_textures_forced_on: archive_rules.is_some_and(Settings::keeps_textures_separate),
        default_archive_limit: archive_rules.map(Settings::archive_limit),
        archive_size_cap: archive_rules.and_then(Settings::size_cap),
    }
}

/// The mode the mode combo shows. No selection, which `form::show_options`
/// makes for an unsupported mode, reads as Single Mod, the mode
/// `form::read_options` saves for it.
pub(crate) fn chosen_mode(ui: &MainWindow) -> OptimizationMode {
    form::mode_at(ui.get_mode_index()).unwrap_or(OptimizationMode::SingleMod)
}

/// The window's Admission: the one a run's Run Request is built with, from
/// the mode the mode combo shows and `capabilities`, through
/// [`run_request::admission`], so the window and the run derive Several Mods
/// in one place. `capabilities` are the loaded Profile's Profile
/// Capabilities, or `None` when its `profile.ini` could not be read, which
/// holds back nothing.
pub(crate) fn admission(ui: &MainWindow, capabilities: Option<&ProfileCapabilities>) -> Admission {
    run_request::admission(chosen_mode(ui), capabilities)
}

/// Applies every rule to the window's current state under the loaded
/// Profile's `capabilities` (see [`admission`]).
pub(crate) fn apply(ui: &MainWindow, capabilities: Option<&ProfileCapabilities>) {
    let admission = admission(ui, capabilities);
    let archive_rules = game_default_rules(ui.global::<BsaTab>().get_game_index());
    write_control_state(ui, &control_state(&admission, archive_rules.as_ref()));
}

/// Copies `state` into the `RuleDisplay` global, which the `.slint` controls draw
/// their forced state and AND into their `enabled` from, and into the Maximum
/// size spin box.
///
/// Nothing here writes a user's choice. The `.slint` side draws a forced
/// control beside the user's (`BsaTab.extract`, `MeshesTab.full`, and so on),
/// so previewing FO4 or choosing Several Mods cannot change what is saved, and
/// the choice shows again once the rule lifts.
///
/// The Archive Limit is the one value written, and only while the Profile
/// gives no size (`BsaTab.archive-limit-given`): the spin box then shows the
/// game's default and follows the Game combo until the user edits it. A loaded
/// size above the Archive Size Cap is kept as it is, so the Profile's line is
/// saved as written until the user edits it, and another game shows it again.
fn write_control_state(ui: &MainWindow, state: &ControlState) {
    let display = ui.global::<RuleDisplay>();
    display.set_extract_forced_off(state.extract_forced_off);
    display.set_create_forced_off(state.create_forced_off);
    display.set_delete_backups_forced_off(state.delete_backups_forced_off);
    display.set_necessary_meshes_forced(state.necessary_meshes_forced);
    display.set_separate_textures_forced(state.separate_textures_forced_on);

    let bsa = ui.global::<BsaTab>();
    if !bsa.get_archive_limit_given()
        && let Some(limit) = state.default_archive_limit
    {
        bsa.set_archive_limit(form::gigabytes_of(limit));
    }
    // 0 is the `.slint` side's "no cap".
    bsa.set_archive_size_cap(state.archive_size_cap.map_or(0.0, form::gigabytes_of));
}

/// The Archive Rules of a Profile that gives no maximum Archive size, for the
/// game the Game combo shows at `game_index`; `None` for no selection. The
/// game's facts (its Archive Size Cap, its default Archive Limit, whether it
/// keeps Textures separate) are the same whatever size a Profile asks for.
fn game_default_rules(game_index: i32) -> Option<Settings> {
    form::game_at(game_index).map(|game| Settings::for_profile(game, None))
}

/// [`control_state`] without a window: one case per rule.
#[cfg(test)]
mod tests {
    use cao_archive::{Game, Settings};
    use cao_core::admission::Admission;
    use cao_core::routing::{ProfileCapabilities, ProfileCapability};

    use super::{ControlState, control_state};

    /// 2000 MiB, the default Archive Limit of the BSA games.
    const BSA_DEFAULT_LIMIT: u64 = 2000 * 1024 * 1024;
    /// 2 GiB, the Archive Size Cap of the BSA games.
    const BSA_SIZE_CAP: u64 = 2 * 1024 * 1024 * 1024;
    /// 4000 MiB, FO4's default Archive Limit.
    const BA2_DEFAULT_LIMIT: u64 = 4000 * 1024 * 1024;

    /// The control state for a run of Several Mods when `several_mods`, with
    /// no capabilities known and no game selected.
    fn state(several_mods: bool) -> ControlState {
        control_state(&Admission::new(several_mods, None), None)
    }

    /// A profile supporting Textures and Meshes but no Archive work.
    fn without_archives() -> ProfileCapabilities {
        ProfileCapabilities::define(
            ".bsa",
            &[
                ProfileCapability::NativeTextureOptimization,
                ProfileCapability::StandardMeshOptimization,
                ProfileCapability::TerrainMeshOptimization,
            ],
        )
    }

    /// The execution mode is not an input: a Dry Run forces nothing either,
    /// which the window tests in `tests/rule_display.rs` pin.
    #[test]
    fn one_mod_without_capabilities_known_forces_nothing() {
        assert_eq!(state(false), ControlState::default());
    }

    #[test]
    fn several_mods_shows_necessary_meshes() {
        assert_eq!(
            state(true),
            ControlState {
                necessary_meshes_forced: true,
                ..ControlState::default()
            }
        );
    }

    /// Several Mods under a profile without Archive support forces the union
    /// of the two rules: the held-back Archive controls and the Several Mods
    /// level radios, nothing more.
    ///
    /// No [`ControlState`] field forces the "Process meshes" group, so Mesh
    /// level 0 (Mesh work off) cannot be expressed here; the window test
    /// `several_mods_loaded_without_mesh_work_keeps_it_off`
    /// (`tests/rule_display.rs`) pins that it stays the user's choice.
    #[test]
    fn several_mods_without_archive_support_forces_both_rules_controls() {
        let capabilities = without_archives();
        let admission = Admission::new(true, Some(&capabilities));
        assert_eq!(
            control_state(&admission, None),
            ControlState {
                extract_forced_off: true,
                create_forced_off: true,
                delete_backups_forced_off: true,
                necessary_meshes_forced: true,
                ..ControlState::default()
            }
        );
    }

    #[test]
    fn unsupported_archive_work_shows_its_controls_off() {
        let capabilities = without_archives();
        let admission = Admission::new(false, Some(&capabilities));
        assert_eq!(
            control_state(&admission, None),
            ControlState {
                extract_forced_off: true,
                create_forced_off: true,
                delete_backups_forced_off: true,
                ..ControlState::default()
            }
        );
    }

    #[test]
    fn a_bsa_game_shows_its_default_limit_and_its_size_cap() {
        let admission = Admission::new(false, None);
        for game in [Game::Tes5, Game::Sse] {
            let rules = Settings::for_profile(game, None);
            assert_eq!(
                control_state(&admission, Some(&rules)),
                ControlState {
                    default_archive_limit: Some(BSA_DEFAULT_LIMIT),
                    archive_size_cap: Some(BSA_SIZE_CAP),
                    ..ControlState::default()
                },
                "{game:?}"
            );
        }
    }

    #[test]
    fn fo4_shows_separate_textures_on_and_no_size_cap() {
        let admission = Admission::new(false, None);
        let rules = Settings::for_profile(Game::Fo4, None);
        assert_eq!(
            control_state(&admission, Some(&rules)),
            ControlState {
                separate_textures_forced_on: true,
                default_archive_limit: Some(BA2_DEFAULT_LIMIT),
                ..ControlState::default()
            }
        );
    }
}
