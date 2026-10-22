//! The window's UI rules (#509), in one place.
//!
//! Each rule forces some controls' checked state and disables them, so the
//! window never shows a combination the run would reject or silently change:
//!
//! - **Dry Run** clears and disables Extract BSA, Create BSA and Delete backups,
//!   and the Base group around them (C++ `dryRunCheckBox` `clicked`).
//! - **Several Mods** checks the "Necessary" mesh level and disables Medium and
//!   Full (C++ `modeChooserComboBox` `activated`).
//! - **A game that keeps Textures in their own Archive** (FO4) checks and
//!   disables "Separate textures BSA", since the packer never merges that
//!   game's Textures into the Standard Archive anyway.
//! - **A game whose Archives have a hard size cap** (TES5 and SSE BSAs, 2 GiB)
//!   stops the Maximum size spin box there, since the packer never packs past
//!   it anyway. Unlike the rules above it limits a value rather than forcing
//!   a checkbox, and it keeps a larger loaded size as written.
//!
//! [`apply`] enforces them all from the controls' current state. It is
//! idempotent, so the same call serves a click on the triggering control and a
//! profile load. C++ ran the rules on a click only, so a profile saved with,
//! say, Dry Run and Extract BSA both on loaded as is (deviation 3).
//!
//! What a rule disables is written to the `UiRules` global, which the `.slint`
//! controls AND into their `enabled`.

use cao_gui_ui::{BsaTab, MainWindow, MeshesTab, UiRules};
use cao_optimizers::profile::{archive_size_cap, keeps_textures_separate};
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
    size_cap(ui);
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

/// Separate textures: while the Game combo shows a game that keeps Textures in
/// their own Archive ([`keeps_textures_separate`], FO4's DX10 BA2), "Separate
/// textures BSA" (the inverse of `bBsaMergeTexture`) is checked and disabled.
/// The packer keeps such Textures separate whatever the checkbox says, so the
/// window shows what the run will do. Another game gives the checkbox back as
/// it is.
fn separate_textures(ui: &MainWindow) {
    let bsa = ui.global::<BsaTab>();
    let forced = form::game_at(bsa.get_game_index()).is_some_and(keeps_textures_separate);
    if forced {
        bsa.set_separate_textures(true);
    }
    ui.global::<UiRules>()
        .set_separate_textures_enabled(!forced);
}

/// Archive size cap: while the Game combo shows a game whose Archives have a
/// hard size cap ([`archive_size_cap`], 2 GiB for the BSA games), the Maximum
/// size spin box stops there. The packer never packs past the cap whatever the
/// Profile says, so the window shows the size the run will use. The spin box
/// keeps a larger loaded size as it is, so the Profile's line is saved as
/// written until the user edits it, and another game shows it again.
fn size_cap(ui: &MainWindow) {
    let bsa = ui.global::<BsaTab>();
    let cap = form::game_at(bsa.get_game_index()).and_then(archive_size_cap);
    // 0 is the `.slint` side's "no cap".
    bsa.set_max_size_cap(cap.map_or(0.0, form::gigabytes_of));
}
