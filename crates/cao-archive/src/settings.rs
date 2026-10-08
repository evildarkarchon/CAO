//! bethutil's per-game archive tables (`btu::bsa::Settings`), hand-ported for the
//! three shipped games, plus file-type classification by first path component.
//!
//! Source: `include/btu/bsa/settings.hpp` at bethutil `81f882ed` (#461).

use std::path::{Component, Path};

use crate::name::split_file_name;

/// A game whose archive rules CAO applies, as a profile's `[BSA] bsaGame` names it.
///
/// **Deviation 11:** bethutil also has TES3, TES4 and FNV tables, and fell back to
/// SSE's for any other value. The port has only these three; `cao-profiles`
/// rejects any other `bsaGame`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Game {
    /// Skyrim Legendary Edition (`btu::Game::SLE`, `bsaGame=3`).
    Tes5,
    /// Skyrim Special Edition (`btu::Game::SSE`, `bsaGame=4`).
    Sse,
    /// Fallout 4 (`btu::Game::FO4`, `bsaGame=5`).
    Fo4,
}

/// The container an Archive is written as (`btu::bsa::ArchiveVersion`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArchiveVersion {
    /// BSA v104 with zlib, as Skyrim LE reads.
    Tes5,
    /// BSA v105 with LZ4 frames, as Skyrim SE reads.
    Sse,
    /// BTDX v1 `GNRL`: a general Fallout 4 BA2.
    Fo4,
    /// BTDX v1 `DX10`: a Fallout 4 texture BA2, always compressed and chunked.
    Fo4Dx,
}

/// A file kind `file_type` sorts a path into (`btu::bsa::FileTypes`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileType {
    /// Packed into a compressed Standard Archive.
    Standard,
    /// Packed into a Textures Archive.
    Texture,
    /// Packed into an Incompressible Archive, which is never compressed.
    Incompressible,
    /// Matches no rule, so it is never packed and stays loose (bethutil's
    /// `Blacklist`, renamed because the glossary avoids that word).
    Unpackable,
    /// A plugin of the game (`.esp` and friends); never packed.
    Plugin,
    /// An Archive of the game (`btu::bsa::FileTypes::BSA`); never packed.
    Archive,
}

/// One classification rule: an extension allowed under some top-level directories
/// (`btu::bsa::AllowedPath`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllowedPath {
    /// The extension, with its dot, in lowercase.
    pub extension: &'static str,
    /// The lowercase first path components, relative to the Mod Root, this rule
    /// accepts. `root` stands for the Mod Root itself; see [`AllowedPath::check`].
    pub directories: &'static [&'static str],
}

impl AllowedPath {
    const fn new(extension: &'static str, directories: &'static [&'static str]) -> Self {
        Self {
            extension,
            directories,
        }
    }

    /// Whether `path`, inside the Mod Root `root`, matches this rule.
    ///
    /// The extension is compared without regard to ASCII case. The directory is the
    /// lowercased first component of `path` relative to `root`, or `root` when
    /// `path` *is* `root`. A root-level file's first component is its own name, so
    /// the `.jpg`/`root` rule only matches under a folder literally named `root`;
    /// that quirk is bethutil's and is kept.
    pub fn check(&self, path: &Path, root: &Path) -> bool {
        if !extension_of(path).eq_ignore_ascii_case(self.extension) {
            return false;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            // bethutil's `lexically_relative` yields a `..` first component here,
            // which no rule lists.
            return false;
        };
        match relative.components().next() {
            None => self.directories.contains(&"root"),
            Some(Component::Normal(first)) => first.to_str().is_some_and(|first| {
                self.directories
                    .iter()
                    .any(|directory| first.eq_ignore_ascii_case(directory))
            }),
            Some(_) => false,
        }
    }
}

/// Classifies `path`, inside the Mod Root `root`, under `settings`' rules
/// (bethutil's `get_filetype`).
///
/// Rules are tried in bethutil's order: Standard, Texture, Incompressible, then a
/// plugin extension, then the Archive extension; anything else is
/// [`FileType::Unpackable`] and stays loose. The plugin and Archive checks compare
/// the extension without regard to ASCII case, and ignore the directory.
pub fn file_type(path: &Path, root: &Path, settings: &Settings) -> FileType {
    let matches = |rules: &[AllowedPath]| rules.iter().any(|rule| rule.check(path, root));
    if matches(settings.standard_files) {
        return FileType::Standard;
    }
    if matches(settings.texture_files) {
        return FileType::Texture;
    }
    if matches(settings.incompressible_files) {
        return FileType::Incompressible;
    }
    let extension = extension_of(path);
    if settings
        .plugin_extensions
        .iter()
        .any(|plugin| extension.eq_ignore_ascii_case(plugin))
    {
        return FileType::Plugin;
    }
    if extension.eq_ignore_ascii_case(settings.extension) {
        return FileType::Archive;
    }
    FileType::Unpackable
}

/// The extension of `path`'s file name with its dot, as `std::filesystem::path::
/// extension` gives it; empty when the name has none or is not Unicode.
pub(crate) fn extension_of(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .map_or("", |name| split_file_name(name).1)
}

/// The 49-byte Dummy Plugins: a `TES4` record holding a `HEDR` and an empty `CNAM`
/// (`btu::bsa::dummy`). SSE's and FO4's carry the ESL flag (`0x200`).
mod dummy {
    pub const TES5: [u8; 49] = [
        0x54, 0x45, 0x53, 0x34, 0x19, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x2B, 0x00, 0x00, 0x00, 0x48, 0x45, 0x44, 0x52, 0x0C, 0x00,
        0x9A, 0x99, 0xD9, 0x3F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x43, 0x4E, 0x41,
        0x4D, 0x01, 0x00, 0x00,
    ];

    pub const SSE: [u8; 49] = [
        0x54, 0x45, 0x53, 0x34, 0x19, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x2C, 0x00, 0x00, 0x00, 0x48, 0x45, 0x44, 0x52, 0x0C, 0x00,
        0x9A, 0x99, 0xD9, 0x3F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x43, 0x4E, 0x41,
        0x4D, 0x01, 0x00, 0x00,
    ];

    pub const FO4: [u8; 49] = [
        0x54, 0x45, 0x53, 0x34, 0x19, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x83, 0x00, 0x00, 0x00, 0x48, 0x45, 0x44, 0x52, 0x0C, 0x00,
        0x33, 0x33, 0x73, 0x3F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x43, 0x4E, 0x41,
        0x4D, 0x01, 0x00, 0x00,
    ];
}

/// SSE's Standard files; every game starts from these.
const STANDARD_SSE: &[AllowedPath] = &[
    AllowedPath::new(".bgem", &["materials"]),
    AllowedPath::new(".bgsm", &["materials"]),
    AllowedPath::new(".bto", &["meshes"]),
    AllowedPath::new(".btr", &["meshes"]),
    AllowedPath::new(".btt", &["meshes"]),
    AllowedPath::new(".cgid", &["grass"]),
    AllowedPath::new(".dlodsettings", &["lodsettings"]),
    AllowedPath::new(".dtl", &["meshes"]),
    AllowedPath::new(".egm", &["meshes"]),
    AllowedPath::new(".jpg", &["root"]),
    // bethutil lists `.hkb` twice; the duplicate is harmless and kept.
    AllowedPath::new(".hkb", &["meshes"]),
    AllowedPath::new(".hkb", &["meshes"]),
    AllowedPath::new(".hkx", &["meshes"]),
    AllowedPath::new(".lst", &["meshes"]),
    AllowedPath::new(".nif", &["meshes"]),
    AllowedPath::new(".psc", &["scripts", "source"]),
    AllowedPath::new(".tga", &["textures"]),
    AllowedPath::new(".tri", &["meshes"]),
];

/// FO4's Standard files: SSE's, plus `.png` textures (FO4 has no PNG texture rule)
/// and `.uvd` visibility data, appended in bethutil's order. Spelled out in full
/// because `const` slices cannot be concatenated; keep it in step with
/// [`STANDARD_SSE`].
const STANDARD_FO4: &[AllowedPath] = &[
    AllowedPath::new(".bgem", &["materials"]),
    AllowedPath::new(".bgsm", &["materials"]),
    AllowedPath::new(".bto", &["meshes"]),
    AllowedPath::new(".btr", &["meshes"]),
    AllowedPath::new(".btt", &["meshes"]),
    AllowedPath::new(".cgid", &["grass"]),
    AllowedPath::new(".dlodsettings", &["lodsettings"]),
    AllowedPath::new(".dtl", &["meshes"]),
    AllowedPath::new(".egm", &["meshes"]),
    AllowedPath::new(".jpg", &["root"]),
    // bethutil lists `.hkb` twice; the duplicate is harmless and kept.
    AllowedPath::new(".hkb", &["meshes"]),
    AllowedPath::new(".hkb", &["meshes"]),
    AllowedPath::new(".hkx", &["meshes"]),
    AllowedPath::new(".lst", &["meshes"]),
    AllowedPath::new(".nif", &["meshes"]),
    AllowedPath::new(".psc", &["scripts", "source"]),
    AllowedPath::new(".tga", &["textures"]),
    AllowedPath::new(".tri", &["meshes"]),
    AllowedPath::new(".png", &["textures"]),
    AllowedPath::new(".uvd", &["vis"]),
];

/// SSE's (and TES5's) Texture files.
const TEXTURE_SSE: &[AllowedPath] = &[
    AllowedPath::new(".dds", &["textures", "interface"]),
    AllowedPath::new(".png", &["textures"]),
];

/// FO4's Texture files: DDS only, since a DX10 BA2 can hold nothing else.
const TEXTURE_FO4: &[AllowedPath] = &[AllowedPath::new(".dds", &["textures", "interface"])];

/// The Incompressible files, the same for every game.
const INCOMPRESSIBLE: &[AllowedPath] = &[
    AllowedPath::new(".dlstrings", &["strings"]),
    AllowedPath::new(".fuz", &["sound"]),
    AllowedPath::new(".fxp", &["shadersfx"]),
    AllowedPath::new(".gid", &["grass"]),
    AllowedPath::new(".gfx", &["interface"]),
    AllowedPath::new(".hkc", &["meshes"]),
    AllowedPath::new(".hkt", &["meshes"]),
    AllowedPath::new(".hkp", &["meshes"]),
    AllowedPath::new(".ilstrings", &["strings"]),
    AllowedPath::new(".ini", &["meshes"]),
    AllowedPath::new(".lip", &["sound"]),
    AllowedPath::new(".lnk", &["grass"]),
    AllowedPath::new(".lod", &["lodsettings"]),
    AllowedPath::new(".ogg", &["sound"]),
    AllowedPath::new(".pex", &["scripts"]),
    AllowedPath::new(".seq", &["seq"]),
    AllowedPath::new(".strings", &["strings"]),
    AllowedPath::new(".swf", &["interface"]),
    AllowedPath::new(".txt", &["interface", "meshes", "scripts"]),
    AllowedPath::new(".wav", &["sound"]),
    AllowedPath::new(".xml", &["dialogueviews"]),
    AllowedPath::new(".xwm", &["music", "sound"]),
];

/// The 2000 MiB btu maximum for TES5 and SSE.
const BSA_MAX_SIZE: u64 = 2000 * 1024 * 1024;
/// The 4000 MiB btu maximum for FO4.
const BA2_MAX_SIZE: u64 = 4000 * 1024 * 1024;

/// One game's archive rules (`btu::bsa::Settings`).
///
/// Every field is public so callers can adjust [`Settings::max_size`], as C++
/// CAO's `archiveSettings` did; see [`Settings::with_profile_max_size`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// The game these rules belong to.
    pub game: Game,
    /// The most source bytes one Archive may hold; split and merge compare against it.
    pub max_size: u64,
    /// The container for Standard and Incompressible Archives, and for merged ones.
    pub format: ArchiveVersion,
    /// The container for Textures Archives; `None` means [`Settings::format`].
    pub texture_format: Option<ArchiveVersion>,
    /// The name suffix for Standard and Incompressible Archives (`Name - Main.ba2`).
    pub suffix: Option<&'static str>,
    /// The name suffix for Textures Archives (`Name - Textures.bsa`).
    pub texture_suffix: Option<&'static str>,
    /// The Archive extension with its dot: `.bsa` or `.ba2`.
    pub extension: &'static str,
    /// The plugin extensions, in the order Loading Plugin names are tried.
    pub plugin_extensions: &'static [&'static str],
    /// The canonical Dummy Plugin bytes. bethutil holds an `optional`, but every
    /// ported game has one.
    pub dummy_plugin: &'static [u8; 49],
    /// The Standard classification rules.
    pub standard_files: &'static [AllowedPath],
    /// The Texture classification rules.
    pub texture_files: &'static [AllowedPath],
    /// The Incompressible classification rules.
    pub incompressible_files: &'static [AllowedPath],
}

impl Settings {
    /// The rules bethutil's `Settings::get` returns for `game`, before any profile
    /// adjustment.
    pub fn get(game: Game) -> Self {
        let sse = Self {
            game: Game::Sse,
            max_size: BSA_MAX_SIZE,
            format: ArchiveVersion::Sse,
            texture_format: Some(ArchiveVersion::Sse),
            suffix: None,
            texture_suffix: Some("Textures"),
            extension: ".bsa",
            plugin_extensions: &[".esl", ".esm", ".esp"],
            dummy_plugin: &dummy::SSE,
            standard_files: STANDARD_SSE,
            texture_files: TEXTURE_SSE,
            incompressible_files: INCOMPRESSIBLE,
        };
        match game {
            Game::Sse => sse,
            Game::Tes5 => Self {
                game: Game::Tes5,
                format: ArchiveVersion::Tes5,
                texture_format: None,
                suffix: None,
                texture_suffix: None,
                plugin_extensions: &[".esm", ".esp"],
                dummy_plugin: &dummy::TES5,
                ..sse
            },
            Game::Fo4 => Self {
                game: Game::Fo4,
                format: ArchiveVersion::Fo4,
                texture_format: Some(ArchiveVersion::Fo4Dx),
                max_size: BA2_MAX_SIZE,
                extension: ".ba2",
                suffix: Some("Main"),
                texture_files: TEXTURE_FO4,
                standard_files: STANDARD_FO4,
                dummy_plugin: &dummy::FO4,
                ..sse
            },
        }
    }

    /// Raises [`Settings::max_size`] to a profile's `maxBsaUncompressedSize` when
    /// that is larger, truncating it to whole bytes, as C++ CAO's
    /// `archiveSettings` did. A smaller, negative or NaN limit changes nothing.
    #[must_use]
    pub fn with_profile_max_size(mut self, profile_max_size: f64) -> Self {
        // C++ compares the double against the table value, then assigns it to an
        // integer, truncating; `as` truncates the same way (and saturates).
        #[expect(
            clippy::cast_precision_loss,
            reason = "C++ converts the table value to double for this comparison"
        )]
        let table = self.max_size as f64;
        if profile_max_size > table {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "truncating the profile's double is the C++ behaviour"
            )]
            let max_size = profile_max_size as u64;
            self.max_size = max_size;
        }
        self
    }

    /// The container an Archive of `archive_type` is written as: bethutil's
    /// `texture_format.value_or(format)` for Textures, otherwise `format`.
    pub fn version_for(&self, archive_type: crate::ArchiveType) -> ArchiveVersion {
        match archive_type {
            crate::ArchiveType::Textures => self.texture_format.unwrap_or(self.format),
            crate::ArchiveType::Standard | crate::ArchiveType::Incompressible => self.format,
        }
    }
}
