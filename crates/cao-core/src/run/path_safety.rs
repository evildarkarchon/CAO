//! The path-safety rules every step that touches a Mod Root shares: which
//! Windows names CAO refuses, and how the directories between a Mod Root
//! and a path beneath it are held in place while that path is used.
//!
//! Staging, publication, Archive extraction, source cleanup and staging
//! recovery all go through these two rules, so none of them can drift from
//! the others (audit FS-12).

use std::collections::BTreeMap;
use std::fs::File;
use std::io;
use std::path::{Component, Path, PathBuf};

use cao_winfs::{Access, FileFacts, Open, Share};

/// Directory pins keyed by path. One map can hold the chains of several paths
/// beneath one Mod Root, which then share every common ancestor's pin.
pub(crate) type DirectoryPins = BTreeMap<PathBuf, File>;

/// Whether `name`, one path component, is a name Win32 would read as
/// something other than the entry it spells, so CAO never creates, stages,
/// publishes or deletes through it.
///
/// Refused: an empty name; a trailing dot or space, which Win32 strips, so
/// `a.dds.` opens `a.dds`; a separator, a reserved character such as `:`,
/// which names an alternate data stream, or a control character; and a DOS
/// device name, which Win32 resolves to the device in any directory and with
/// any extension (see [`reserved_device_name`]).
pub(crate) fn unsafe_windows_name(name: &str) -> bool {
    name.is_empty()
        || name.ends_with(['.', ' '])
        || name.contains(['/', '\\', '<', '>', ':', '"', '|', '?', '*'])
        || name.chars().any(|character| u32::from(character) < 32)
        || reserved_device_name(name)
}

/// Whether Win32 resolves `name` to a DOS device, which it does in any
/// directory and with any extension.
///
/// The stem is compared with trailing spaces and dots trimmed, so `NUL .pex`
/// is refused as well: Win32's device-name match skips them, and refusing a
/// name that may alias a device costs a mod nothing it could rely on.
pub(crate) fn reserved_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or_default();
    let stem = stem.trim_end_matches([' ', '.']).to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return true;
    }
    let mut characters = stem.chars();
    let prefix: String = characters.by_ref().take(3).collect();
    let number = characters.next();
    (prefix == "COM" || prefix == "LPT")
        && characters.next().is_none()
        && number.is_some_and(|digit| matches!(digit, '1'..='9' | '¹' | '²' | '³'))
}

/// Why a directory chain could not be pinned. Nothing on disk is changed by
/// a failed pin; the pins taken before it stay in the caller's map.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ChainError {
    /// The directory is not the Mod Root or a path beneath it made only of
    /// ordinary components.
    #[error("`{}` is not inside its Mod Root", .0.display())]
    Outside(PathBuf),
    /// A directory on the chain has a name Win32 would alias
    /// ([`unsafe_windows_name`]).
    #[error("`{}` has a name Windows would alias", .0.display())]
    UnsafeName(PathBuf),
    /// A directory on the chain does not exist.
    #[error("Pin `{}`: {source}", .path.display())]
    Missing {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// A directory on the chain is a link, a junction or another reparse
    /// point, or not a directory at all.
    #[error("`{}` is not an ordinary directory", .0.display())]
    NotDirectory(PathBuf),
    /// Opening or inspecting a directory on the chain failed.
    #[error("Pin `{}`: {source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Opens the ordinary directory `path` so that, while the handle lives, no
/// other handle can rename or delete it. `share` is what other handles may
/// still do; it never includes delete.
///
/// The handle has list access, because a handle with no data access takes no
/// part in sharing checks, and the withheld delete sharing would then stop
/// nothing. The open never follows a link, so a link or junction at `path`
/// opens as itself and is refused.
///
/// # Errors
/// [`ChainError::Missing`] for an absent directory,
/// [`ChainError::NotDirectory`] for a link, a reparse point or a file, and
/// [`ChainError::Io`] for any other open or query failure.
pub(crate) fn pin_ordinary_directory(path: &Path, share: Share) -> Result<File, ChainError> {
    let pin = Open::new(Access::LIST_DIRECTORY | Access::READ_ATTRIBUTES, share)
        .directory()
        .open(path)
        .map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                ChainError::Missing {
                    path: path.to_path_buf(),
                    source,
                }
            } else {
                ChainError::Io {
                    path: path.to_path_buf(),
                    source,
                }
            }
        })?;
    let facts = FileFacts::of(&pin).map_err(|source| ChainError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !facts.is_directory() || facts.is_reparse_point() {
        return Err(ChainError::NotDirectory(path.to_path_buf()));
    }
    Ok(pin)
}

/// Pins `root` and every directory from it down to `directory`, which must
/// be `root` itself or lie beneath it, into `pins`. A directory already in
/// `pins` is reused, not opened again.
///
/// The chain starts at the Mod Root, never above it. Everything CAO creates,
/// replaces or deletes lies beneath the canonical Mod Root the run resolved
/// at Preparing, and an Apply run's staging scope keeps that root pinned from
/// then on. The folders above it are the user's own layout: pinning them
/// protected nothing CAO acts on, and needed list access to each of them,
/// which a network share or a locked-down parent folder does not always
/// grant.
///
/// Each directory is opened only while its parent is pinned, from the root
/// down, so no component can be renamed or replaced with a junction between
/// the check of one name and the open of the next: checking only the last
/// directory could still follow an earlier junction. Every name below the
/// root must pass [`unsafe_windows_name`]. `share` is what the pins let other
/// handles do (see [`pin_ordinary_directory`]).
///
/// # Errors
/// [`ChainError::Outside`] when `directory` is not `root` or beneath it,
/// [`ChainError::UnsafeName`] for an aliasing name, and otherwise as
/// [`pin_ordinary_directory`] for the first directory that fails. The pins
/// taken before a failure stay in `pins`.
pub(crate) fn pin_directory_chain(
    root: &Path,
    directory: &Path,
    share: Share,
    pins: &mut DirectoryPins,
) -> Result<(), ChainError> {
    let outside = || ChainError::Outside(directory.to_path_buf());
    let relative = directory.strip_prefix(root).map_err(|_| outside())?;
    if !root.is_absolute() {
        return Err(outside());
    }
    let mut names = Vec::new();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(outside());
        };
        names.push(name);
    }
    let mut current = root.to_path_buf();
    if !pins.contains_key(&current) {
        let pin = pin_ordinary_directory(&current, share)?;
        pins.insert(current.clone(), pin);
    }
    for name in names {
        current.push(name);
        if pins.contains_key(&current) {
            continue;
        }
        // A name that is not Unicode cannot be one Win32 aliases; the
        // lossy form only replaces what cannot be a reserved character.
        if unsafe_windows_name(&name.to_string_lossy()) {
            return Err(ChainError::UnsafeName(current));
        }
        let pin = pin_ordinary_directory(&current, share)?;
        pins.insert(current.clone(), pin);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, empty directory under the system temporary directory, made
    /// canonical so a chain can start at it. `CARGO_TARGET_TMPDIR` exists only
    /// for integration tests.
    fn scratch_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("cao-core-unit")
            .join(format!("{name}-{}", std::process::id()));
        // A missing directory is the expected case; anything else surfaces in create_dir_all.
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        cao_winfs::canonical(&dir).unwrap()
    }

    #[test]
    fn device_names_are_recognized_with_any_extension_and_trailing_spaces() {
        for name in [
            "NUL",
            "nul.pex",
            "Con.txt",
            "COM1.esp",
            "lpt9",
            "COM¹.x",
            "NUL .pex",
            "conin$",
            "CONOUT$.log",
            "aux.tar.gz",
        ] {
            assert!(reserved_device_name(name), "{name}");
            assert!(unsafe_windows_name(name), "{name}");
        }
        for name in [
            "NULL.pex",
            "COM0",
            "COM10",
            "console",
            "LPT",
            "aux_file.dds",
            "connect.esp",
        ] {
            assert!(!reserved_device_name(name), "{name}");
        }
    }

    /// Every way a component can alias another entry, a stream or a device
    /// is refused, and an ordinary game-path name is not.
    #[test]
    fn the_name_predicate_refuses_every_aliasing_name() {
        for name in [
            "",
            ".",
            "..",
            "trailing.",
            "trailing ",
            "stream.dds:hidden",
            "a<b",
            "a>b",
            "a\"b",
            "a|b",
            "a?b",
            "a*b",
            "a/b",
            "a\\b",
            "bell\u{7}.dds",
            "tab\t.dds",
            "PRN",
            "lpt1.dds",
        ] {
            assert!(unsafe_windows_name(name), "{name:?}");
        }
        for name in [
            "textures",
            "Shared.dds",
            ".hidden",
            "name with spaces.nif",
            "Ünïcode.esp",
            "a.b.c",
            "COM10.dds",
        ] {
            assert!(!unsafe_windows_name(name), "{name:?}");
        }
    }

    /// The chain pins the Mod Root and each directory down to the one asked
    /// for, and while it holds them none of them can be renamed.
    #[test]
    fn a_pinned_chain_holds_the_root_and_every_directory_below_it() {
        let root = scratch_root("chain-holds");
        let deep = root.join("textures").join("armor");
        std::fs::create_dir_all(&deep).unwrap();
        let mut pins = DirectoryPins::new();

        pin_directory_chain(&root, &deep, Share::READ | Share::WRITE, &mut pins).unwrap();

        let pinned: Vec<_> = pins.keys().cloned().collect();
        assert_eq!(pinned, [root.clone(), root.join("textures"), deep.clone()]);
        assert!(
            std::fs::rename(root.join("textures"), root.join("moved")).is_err(),
            "a pinned directory cannot be renamed"
        );
        assert!(std::fs::rename(&deep, root.join("textures").join("moved")).is_err());
        drop(pins);
        std::fs::rename(root.join("textures"), root.join("moved")).unwrap();
    }

    /// A second chain through the same Mod Root reuses the pins the first
    /// took instead of opening them again.
    #[test]
    fn a_shared_map_reuses_the_pins_it_already_holds() {
        let root = scratch_root("chain-shared");
        std::fs::create_dir_all(root.join("a").join("b")).unwrap();
        std::fs::create_dir_all(root.join("a").join("c")).unwrap();
        let mut pins = DirectoryPins::new();
        pin_directory_chain(&root, &root.join("a").join("b"), Share::READ, &mut pins).unwrap();
        let shared = cao_winfs::FileIdentity::of(&pins[&root.join("a")]).unwrap();

        pin_directory_chain(&root, &root.join("a").join("c"), Share::READ, &mut pins).unwrap();

        assert_eq!(pins.len(), 4);
        assert_eq!(
            cao_winfs::FileIdentity::of(&pins[&root.join("a")]).unwrap(),
            shared
        );
    }

    /// A junction on the chain is refused as itself, never followed, and a
    /// directory outside the Mod Root or reached by traversal is refused
    /// before anything is opened.
    #[test]
    fn a_chain_refuses_a_junction_and_anything_outside_its_root() {
        let root = scratch_root("chain-refusals");
        let elsewhere = scratch_root("chain-refusals-target");
        std::fs::create_dir(elsewhere.join("inner")).unwrap();
        let link = root.join("link");
        let made = std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(&link)
            .arg(&elsewhere)
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");
        let share = Share::READ | Share::WRITE;

        let mut pins = DirectoryPins::new();
        let error = pin_directory_chain(&root, &link.join("inner"), share, &mut pins).unwrap_err();
        assert!(
            matches!(&error, ChainError::NotDirectory(path) if *path == link),
            "{error:?}"
        );
        assert!(!pins.contains_key(&link.join("inner")));

        for directory in [
            elsewhere.join("inner"),
            root.join("..").join("escape"),
            PathBuf::from("relative"),
        ] {
            let error = pin_directory_chain(&root, &directory, share, &mut pins).unwrap_err();
            assert!(
                matches!(error, ChainError::Outside(_)),
                "{directory:?}: {error:?}"
            );
        }
    }

    /// A directory whose name Win32 would alias to a device is refused by
    /// name, before the chain opens it; an absent directory is reported as
    /// missing, so a caller can tell it from a refusal.
    #[test]
    fn a_chain_refuses_a_device_name_and_reports_an_absent_directory() {
        let root = scratch_root("chain-names");
        let share = Share::READ | Share::WRITE;
        let mut pins = DirectoryPins::new();

        for name in ["CON", "nul.dir", "trailing."] {
            let error = pin_directory_chain(&root, &root.join(name).join("x"), share, &mut pins)
                .unwrap_err();
            assert!(
                matches!(error, ChainError::UnsafeName(_)),
                "{name}: {error:?}"
            );
        }
        let error = pin_directory_chain(&root, &root.join("absent").join("x"), share, &mut pins)
            .unwrap_err();
        assert!(
            matches!(&error, ChainError::Missing { path, .. } if *path == root.join("absent")),
            "{error:?}"
        );
    }
}
