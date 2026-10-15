//! The window's UI rules (#509), in one place.
//!
//! Each rule forces some controls' checked state and disables them, so the
//! window never shows a combination the run would reject or silently change:
//!
//! - **Dry Run** clears and disables Extract BSA, Create BSA and Delete backups,
//!   and the Base group around them (C++ `dryRunCheckBox` `clicked`).
//! - **Several Mods** checks the "Necessary" mesh level and disables Medium and
//!   Full (C++ `modeChooserComboBox` `activated`).
//! - **FO4** checks and disables "Separate textures BSA". C++ has no such rule
//!   (deviation 21).
//!
//! [`apply`] enforces them all from the controls' current state. It is
//! idempotent, so the same call serves a click on the triggering control and a
//! profile load. C++ ran the rules on a click only, so a profile saved with,
//! say, Dry Run and Extract BSA both on loaded as is (deviation 3).
//!
//! What a rule disables is written to the `UiRules` global, which the `.slint`
//! controls AND into their `enabled`.

use cao_gui_ui::{BsaTab, MainWindow, MeshesTab, UiRules};
use cao_profiles::{BsaGame, OptimizationMode};
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
    fo4_textures(ui);
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
/// work) stays allowed, as the parity guard expects.
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

/// FO4 (deviation 21, new in the port): FO4 never merges textures into the
/// general archive, so "Separate textures BSA" (the inverse of
/// `bBsaMergeTexture`) is checked and disabled. Another game gives it back as
/// it is.
fn fo4_textures(ui: &MainWindow) {
    let bsa = ui.global::<BsaTab>();
    let fo4 = form::game_at(bsa.get_game_index()) == Some(BsaGame::Fo4);
    if fo4 {
        bsa.set_separate_textures(true);
    }
    ui.global::<UiRules>().set_separate_textures_enabled(!fo4);
}
