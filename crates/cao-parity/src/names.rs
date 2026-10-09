//! The fixed name vocabulary generated cases draw their names from (#473).
//!
//! Generated names come only from these lists, so no generated case can hold a
//! deviation trigger by accident:
//!
//! - no `separator` in any case (deviation 20's `_separator` children are
//!   written by hand, in seeds);
//! - no `facegen` or `meshes` above a Mod Root (deviation 17);
//! - no `FilesToNotPack.txt` line (deviation 16);
//! - nothing in the `.cao-staging` namespace (deviation 19);
//! - no all-digit stems (deviation 9) and no device stems, trailing spaces or
//!   trailing dots (deviation 15).
//!
//! Game paths stay ASCII. One Several Mods parent and one Mod Root are not, so
//! every generated case set exercises non-ASCII Mod Roots and the Archive and
//! plugin names derived from them. Every name is short, so a generated path
//! stays far below [`crate::materialise::PATH_CAP_UTF16`].

/// Several Mods parent folders, and the folders a One Mod Root sits in.
pub const PARENTS: &[&str] = &["mods", "Mod Library", "Modsammlung Ü"];

/// Mod Root folder names. Archive and Loading Plugin names derive from them.
pub const MOD_ROOTS: &[&str] = &[
    "Alpha",
    "Bravo",
    "Charlie",
    "Delta",
    "Echo",
    "Foxtrot",
    "Mod Ünïcode",
];

/// Asset subfolders below an Asset Kind's top folder (`textures/<folder>`).
pub const FOLDERS: &[&str] = &["architecture", "armor", "clutter", "weapons", "landscape"];

/// Asset file stems.
pub const STEMS: &[&str] = &["barrel", "crate", "lantern", "shield", "sword", "banner"];

#[cfg(test)]
mod tests {
    use super::*;

    /// The `FilesToNotPack.txt` lines CAO ships, lowercased.
    fn packing_exclusions() -> Vec<String> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../profiles/SSE/FilesToNotPack.txt"
        );
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| line.trim().to_lowercase())
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .collect()
    }

    fn all_names() -> impl Iterator<Item = &'static str> {
        [PARENTS, MOD_ROOTS, FOLDERS, STEMS]
            .into_iter()
            .flatten()
            .copied()
    }

    #[test]
    fn no_name_holds_a_deviation_trigger() {
        let exclusions = packing_exclusions();
        for name in all_names() {
            let lower = name.to_lowercase();
            assert!(!lower.contains("separator"), "{name}");
            assert!(!lower.starts_with(".cao-staging"), "{name}");
            assert!(!name.chars().all(|c| c.is_ascii_digit()), "{name}");
            assert!(!name.ends_with([' ', '.']), "{name}");
            assert!(!crate::materialise::is_device_name(name), "{name}");
            assert!(
                !exclusions
                    .iter()
                    .any(|line| line.split('/').any(|part| part == lower)
                        || line.starts_with(&format!("{lower}."))),
                "{name} is part of a FilesToNotPack.txt line"
            );
        }
        for name in [PARENTS, MOD_ROOTS].into_iter().flatten() {
            let lower = name.to_lowercase();
            assert!(
                !lower.contains("facegen") && !lower.contains("meshes"),
                "{name}"
            );
        }
    }

    #[test]
    fn game_path_names_are_ascii_and_mod_roots_include_non_ascii() {
        for name in [FOLDERS, STEMS].into_iter().flatten() {
            assert!(name.is_ascii(), "{name}");
        }
        assert!(MOD_ROOTS.iter().any(|name| !name.is_ascii()));
        assert!(PARENTS.iter().any(|name| !name.is_ascii()));
    }
}
