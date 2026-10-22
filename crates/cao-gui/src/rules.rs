//! The window's UI rules (#509), in one place.
//!
//! The window never shows a combination the run would reject or silently
//! change. Two rules force some controls' checked state and disable them:
//!
//! - **Dry Run** clears and disables Extract BSA, Create BSA and Delete backups,
//!   and the Base group around them (C++ `dryRunCheckBox` `clicked`).
//! - **Several Mods** checks the "Necessary" mesh level and disables Medium and
//!   Full (C++ `modeChooserComboBox` `activated`).
//!
//! Two rules show the Archive Rules the run will use for the game in the Game
//! combo, read from `cao_archive::Settings`, and never write over what the
//! user chose:
//!
//! - **A game that keeps Textures in their own Archive** (FO4) shows "Separate
//!   textures BSA" checked and disabled, since the packer never merges that
//!   game's Textures into the Standard Archive. The user's own choice is kept
//!   and saved as it is, and shows again under another game.
//! - **The Archive Limit** is what the Maximum size spin box shows: the
//!   Profile's size, or the game's default while the Profile gives none (saved
//!   still giving none until the user edits it). A game with an Archive Size
//!   Cap (TES5 and SSE BSAs, 2 GiB) stops the spin box there, and a larger
//!   loaded size is kept as written.
//!
//! [`apply`] enforces them all from the controls' current state. It is
//! idempotent, so the same call serves a click on the triggering control and a
//! profile load. C++ ran the rules on a click only, so a profile saved with,
//! say, Dry Run and Extract BSA both on loaded as is (deviation 3).
//!
//! What a rule disables is written to the `UiRules` global, which the `.slint`
//! controls AND into their `enabled`; so is the game forcing "Separate textures
//! BSA", which the `.slint` side shows without touching the user's choice.

use cao_archive::Settings;
use cao_gui_ui::{BsaTab, MainWindow, MeshesTab, UiRules};
use cao_profiles::OptimizationMode;
use slint::ComponentHandle;

use crate::form;

/// Whether the mode combo shows "Several mods".
pub(crate) fn several_mods_chosen(ui: &MainWindow) -> bool {
    form::mode_at(ui.get_mode_index()) == Some(OptimizationMode::SeveralMods)
}

/// Applies every UI rule to the window's current state.
pub(crate) fn apply(ui: &MainWindow) {
    dry_run(ui);
    several_mods(ui);
    separate_textures(ui);
    archive_limit(ui);
}

/// Dry Run: the base archive options are cleared and disabled while it is on.
/// C++ also cleared them when Dry Run was turned off, but they cannot be
/// checked while it is on, so they are already clear then.
fn dry_run(ui: &MainWindow) {
    let dry_run = ui.get_dry_run();
    if dry_run {
        let bsa = ui.global::<BsaTab>();
        bsa.set_extract(false);
        bsa.set_create(false);
        bsa.set_delete_backups(false);
    }
    ui.global::<UiRules>().set_bsa_base_enabled(!dry_run);
}

/// Several Mods: "Necessary" is the mesh level, and Medium and Full are
/// disabled. The "Process meshes" group is left as it is, so level 0 (no mesh
/// work) stays allowed.
///
/// C++ also called `setChecked(false)` on "Necessary" for One Mod, which Qt
/// ignores on the checked button of an exclusive group; so One Mod only gives
/// the heavier levels back.
///
/// While the shown Meshes page has its group unchecked, its disabled RadioGroup
/// reverts what this writes to the radios, so the window re-applies the rules
/// when the group is checked again.
fn several_mods(ui: &MainWindow) {
    let several_mods = several_mods_chosen(ui);
    if several_mods {
        // All three, so the global stays exclusive while the Meshes page (and
        // the RadioGroup that keeps its radios exclusive) is not shown.
        let meshes = ui.global::<MeshesTab>();
        meshes.set_necessary(true);
        meshes.set_medium(false);
        meshes.set_full(false);
    }
    ui.global::<UiRules>()
        .set_heavy_mesh_levels_enabled(!several_mods);
}

/// Separate textures: while the Game combo shows a game whose Archive Rules
/// keep Textures in their own Archive ([`Settings::keeps_textures_separate`],
/// FO4's DX10 BA2), "Separate textures BSA" shows checked and disabled. The
/// packer keeps such Textures separate whatever the user chose, so the window
/// shows what the run will do.
///
/// The rule forces only what the checkbox shows. The user's Work Choice
/// (`BsaTab.separate-textures`, the inverse of `bBsaMergeTexture`) is never
/// written here, so previewing FO4 cannot change what another game saves, and
/// another game shows the choice again.
fn separate_textures(ui: &MainWindow) {
    let bsa = ui.global::<BsaTab>();
    let forced = game_default_rules(bsa.get_game_index())
        .is_some_and(|rules| rules.keeps_textures_separate());
    ui.global::<UiRules>().set_separate_textures_forced(forced);
}

/// Archive Limit: the Maximum size spin box shows the Archive Limit
/// [`Settings::for_profile`] gives the game in the Game combo, which is the
/// size the run packs at.
///
/// - A Profile that gives no size shows the game's default, and follows the
///   Game combo until the user edits the spin box (`BsaTab.archive-limit-given`).
/// - While the game has an Archive Size Cap ([`Settings::size_cap`], 2 GiB for
///   the BSA games), the spin box stops there. It keeps a larger loaded size
///   as it is, so the Profile's line is saved as written until the user edits
///   it, and another game shows it again.
fn archive_limit(ui: &MainWindow) {
    let bsa = ui.global::<BsaTab>();
    let rules = game_default_rules(bsa.get_game_index());
    if !bsa.get_archive_limit_given()
        && let Some(rules) = &rules
    {
        bsa.set_archive_limit(form::gigabytes_of(rules.archive_limit()));
    }
    let cap = rules.and_then(|rules| rules.size_cap());
    // 0 is the `.slint` side's "no cap".
    bsa.set_archive_size_cap(cap.map_or(0.0, form::gigabytes_of));
}

/// The Archive Rules of a Profile that gives no maximum Archive size, for the
/// game the Game combo shows at `game_index`; `None` for no selection. The
/// game's facts (its Archive Size Cap, its default Archive Limit, whether it
/// keeps Textures separate) are the same whatever size a Profile asks for.
fn game_default_rules(game_index: i32) -> Option<Settings> {
    form::game_at(game_index).map(|game| Settings::for_profile(game, None))
}
