//! Plugin and Archive names (`btu::bsa::FilePath`): parsing a name into its stem,
//! counter and suffix, and rendering it back.
//!
//! Source: `src/bsa/plugin.cpp` and `include/btu/bsa/plugin.hpp` at bethutil
//! `81f882ed` (#461).

use std::path::{Path, PathBuf};

use crate::error::ArchiveError;
use crate::settings::Settings;

/// What separates a name from its suffix: `Name - Textures`.
const SUFFIX_SEPARATOR: &str = " - ";

/// Which names a [`FilePath`] parses: bethutil's `FileTypes::Plugin` or
/// `FileTypes::BSA`. The variant order is bethutil's, which [`FilePath`]'s
/// ordering compares last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NameKind {
    /// A plugin, matched against the game's plugin extensions.
    Plugin,
    /// An Archive, matched against the game's Archive extension.
    Archive,
}

/// A plugin or Archive name split into its parts (`btu::bsa::FilePath`).
///
/// `Foo2 - Textures.bsa` is `dir`, name `Foo`, counter 2, suffix `Textures`, and
/// extension `.bsa`. The derived ordering compares the fields in declaration
/// order, which is bethutil's defaulted `operator<=>`; C++ CAO sorts plugins with it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FilePath {
    /// The directory holding the file.
    pub dir: PathBuf,
    /// The stem without its counter and suffix. An all-digit stem is all name.
    pub name: String,
    /// The game's suffix the stem ended with, or empty.
    pub suffix: String,
    /// The extension, with its dot.
    pub ext: String,
    /// The trailing number of the stem, if any.
    pub counter: Option<u32>,
    /// Whether this names a plugin or an Archive.
    pub kind: NameKind,
}

impl FilePath {
    /// Parses `path` as a name of `kind` under `settings`' rules, or `None` when
    /// its extension is not one of the game's (case-sensitive: `Foo.ESP` is not a
    /// plugin), or its file name is missing or not Unicode.
    ///
    /// The stem is parsed as bethutil does: trailing digits become the counter,
    /// then the text after the last ` - ` becomes the suffix if it is exactly one
    /// of the game's suffixes, then, if no counter was found, trailing digits are
    /// tried again. Unlike bethutil's `make`, this never touches the filesystem;
    /// [`list_plugins`] and [`list_archives`] skip directories.
    pub fn make(path: &Path, settings: &Settings, kind: NameKind) -> Option<Self> {
        let file_name = path.file_name()?.to_str()?;
        let (stem, ext) = split_file_name(file_name);
        let known = match kind {
            NameKind::Plugin => settings.plugin_extensions.contains(&ext),
            NameKind::Archive => ext == settings.extension,
        };
        if !known {
            return None;
        }
        let mut name = stem.to_owned();
        let mut counter = eat_digits(&mut name);
        let suffix = eat_suffix(&mut name, settings);
        if counter.is_none() {
            counter = eat_digits(&mut name);
        }
        Some(Self {
            dir: path.parent().map(Path::to_path_buf).unwrap_or_default(),
            name,
            suffix,
            ext: ext.to_owned(),
            counter,
            kind,
        })
    }

    /// The stem this name renders as: name, then counter, then ` - ` and the
    /// suffix when there is one. `Foo - Textures2` therefore renders as
    /// `Foo2 - Textures`, and a counter loses any leading zeros.
    pub fn full_name(&self) -> String {
        let mut full = self.name.clone();
        if let Some(counter) = self.counter {
            full.push_str(&counter.to_string());
        }
        if !self.suffix.is_empty() {
            full.push_str(SUFFIX_SEPARATOR);
            full.push_str(&self.suffix);
        }
        full
    }

    /// The path this name renders as: [`FilePath::full_name`] plus the extension,
    /// in [`FilePath::dir`].
    pub fn full_path(&self) -> PathBuf {
        self.dir.join(self.full_name() + &self.ext)
    }
}

/// Lists the plugins directly in `dir` (not recursively) under `settings`' rules
/// (bethutil's `list_plugins` over a `directory_iterator`), in directory order.
///
/// # Errors
///
/// [`ArchiveError::Io`] when `dir` cannot be listed, and
/// [`ArchiveError::NonUnicodeName`] for any entry whose name is not Unicode, where
/// C++ threw converting it.
pub fn list_plugins(dir: &Path, settings: &Settings) -> Result<Vec<FilePath>, ArchiveError> {
    list(dir, settings, NameKind::Plugin)
}

/// Lists the Archives directly in `dir` (not recursively) under `settings`' rules
/// (bethutil's `list_archive`), in directory order.
///
/// # Errors
///
/// As [`list_plugins`].
pub fn list_archives(dir: &Path, settings: &Settings) -> Result<Vec<FilePath>, ArchiveError> {
    list(dir, settings, NameKind::Archive)
}

/// Lists the non-directory entries of `dir` that parse as names of `kind`.
fn list(dir: &Path, settings: &Settings, kind: NameKind) -> Result<Vec<FilePath>, ArchiveError> {
    let io_error = |source| ArchiveError::Io {
        path: dir.to_path_buf(),
        source,
    };
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(io_error)? {
        let path = entry.map_err(io_error)?.path();
        // `is_dir` follows links, as bethutil's `fs::is_directory` does; a broken
        // link is not a directory and is parsed like a file. bethutil skipped
        // directories before converting any name, so a non-Unicode directory is
        // skipped too.
        if path.is_dir() {
            continue;
        }
        if path.file_name().and_then(|name| name.to_str()).is_none() {
            return Err(ArchiveError::NonUnicodeName { path });
        }
        if let Some(name) = FilePath::make(&path, settings, kind) {
            names.push(name);
        }
    }
    Ok(names)
}

/// Moves the trailing ASCII digits of `name` into the returned counter.
///
/// **Deviation 9:** bethutil's `eat_digits` walks off the front of an all-digit
/// (or empty) string, which is undefined behaviour. Here an all-digit name is left
/// alone and has no counter, so it renders back exactly, leading zeros included:
/// taking `01` as counter 1 would name `01.esp`'s Archive `1.bsa`, which that
/// plugin does not load. Digits too large for `u32` also stay in the name, as C++
/// `stoul` throwing `out_of_range` left them.
fn eat_digits(name: &mut String) -> Option<u32> {
    let digits = name.bytes().rev().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits == name.len() {
        return None;
    }
    let start = name.len() - digits;
    let counter = name[start..].parse().ok()?;
    name.truncate(start);
    Some(counter)
}

/// Removes and returns the text after the last ` - ` in `name` when it is exactly
/// the game's suffix or texture suffix; otherwise leaves `name` alone and returns
/// an empty suffix.
fn eat_suffix(name: &mut String, settings: &Settings) -> String {
    let Some(position) = name.rfind(SUFFIX_SEPARATOR) else {
        return String::new();
    };
    let suffix = &name[position + SUFFIX_SEPARATOR.len()..];
    if Some(suffix) != settings.suffix && Some(suffix) != settings.texture_suffix {
        return String::new();
    }
    let suffix = suffix.to_owned();
    name.truncate(position);
    suffix
}

/// Splits a file name into its stem and its extension (with the dot), as
/// `std::filesystem::path::stem` and `extension` do: the extension starts at the
/// last dot, except that a leading dot (`.esp`) and the names `.` and `..` have
/// none. Unlike [`std::path::Path::extension`], a trailing dot is an extension
/// (`foo.` gives `.`).
pub(crate) fn split_file_name(name: &str) -> (&str, &str) {
    if name == "." || name == ".." {
        return (name, "");
    }
    match name.rfind('.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name, ""),
    }
}
