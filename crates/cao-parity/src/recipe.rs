//! The declarative parts of `case.json` beside the `CaseSpec` (#473, #490): the
//! GUI-reachable profile overrides and the tree recipe.
//!
//! A recipe says what a case's tree holds, never how it was made. Content
//! entries are written once into `input/` and copied byte for byte to each
//! side; filesystem-shape operations are then applied to every copy, because a
//! plain copy breaks hard links, junctions, symlinks, attributes and
//! `\\?\`-only names. [`crate::materialise`] turns a recipe into files.
//!
//! Every path in a recipe is relative to a side's case root and
//! `/`-separated, as `CaseSpec`'s Mod Selection folder is.

use directxtex::DXGI_FORMAT;
use serde::{Deserialize, Serialize};

/// The profile values a case changes from the shipped profile, each one a value
/// the GUI's widgets can produce. `None` keeps the profile's own value.
///
/// They are written into each side's private `profiles/<P>/profile.ini` with the
/// QSettings-compatible writer, so both builds read the bytes the GUI's save
/// would produce. The archive game and the maximum archive size are not here:
/// the first is tied to the profile's identity, and C++ never lets the second
/// lower the limit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_format: Option<OutputFormat>,
    /// The unwanted formats, in the order the profile stores them. An empty list
    /// is "none"; whether a list is one the GUI's dialog can produce is the
    /// deviation guard's call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unwanted_formats: Option<Vec<TextureFormat>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compress_interface: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub convert_tga: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh_target: Option<MeshTarget>,
}

impl ProfileOverrides {
    /// Whether the case keeps every shipped profile value.
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// The Textures output formats the GUI's combo box offers (`src/MainWindow.cpp`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputFormat {
    #[serde(rename = "BC7_UNORM")]
    Bc7,
    #[serde(rename = "BC5_UNORM")]
    Bc5,
    #[serde(rename = "BC3_UNORM")]
    Bc3,
    #[serde(rename = "BC1_UNORM")]
    Bc1,
    #[serde(rename = "R8G8B8A8_UNORM")]
    R8G8B8A8,
}

impl OutputFormat {
    /// The `DXGI_FORMAT` written to `texturesFormat`.
    pub fn format(self) -> DXGI_FORMAT {
        match self {
            Self::Bc7 => DXGI_FORMAT::DXGI_FORMAT_BC7_UNORM,
            Self::Bc5 => DXGI_FORMAT::DXGI_FORMAT_BC5_UNORM,
            Self::Bc3 => DXGI_FORMAT::DXGI_FORMAT_BC3_UNORM,
            Self::Bc1 => DXGI_FORMAT::DXGI_FORMAT_BC1_UNORM,
            Self::R8G8B8A8 => DXGI_FORMAT::DXGI_FORMAT_R8G8B8A8_UNORM,
        }
    }
}

/// The three game presets of the mesh target. Arbitrary user, stream and
/// version combinations are not allowed: the GUI only ever saves these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeshTarget {
    /// Skyrim LE: `20.2.0.7`, user 12, stream 83.
    Le,
    /// Skyrim SE: `20.2.0.7`, user 12, stream 100.
    Sse,
    /// Fallout 4: `20.2.0.7`, user 12, stream 130.
    Fo4,
}

impl MeshTarget {
    /// `nifly::V20_2_0_7`, the file version every preset writes.
    pub const FILE_VERSION: u32 = 0x1402_0007;

    /// The `(user, stream)` pair the preset writes.
    pub fn user_and_stream(self) -> (u32, u32) {
        match self {
            Self::Le => (12, 83),
            Self::Sse => (12, 100),
            Self::Fo4 => (12, 130),
        }
    }
}

/// A `DXGI_FORMAT`, named in JSON without its `DXGI_FORMAT_` prefix, such as
/// `"BC7_UNORM"` or `"R8G8B8A8_TYPELESS"`. The vendor formats keep their whole
/// name, such as `"XBOX_DXGI_FORMAT_R4G4_UNORM"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TextureFormat(pub DXGI_FORMAT);

impl TryFrom<String> for TextureFormat {
    type Error = String;

    /// Finds the format whose DirectXTex name is `DXGI_FORMAT_<name>`, or
    /// exactly `<name>` for a vendor format. The crate's `Debug` impl is its
    /// name table, so no second table can drift.
    fn try_from(name: String) -> Result<Self, Self::Error> {
        let prefixed = format!("DXGI_FORMAT_{name}");
        (1..=u32::from(DXGI_FORMAT::WIN11_DXGI_FORMAT_A4B4G4R4_UNORM))
            .map(DXGI_FORMAT::from)
            .find(|format| {
                let full = format!("{format:?}");
                full == prefixed || (!full.starts_with("DXGI_FORMAT_") && full == name)
            })
            .map(Self)
            .ok_or_else(|| format!("`{name}` is not a DXGI_FORMAT name"))
    }
}

impl From<TextureFormat> for String {
    fn from(format: TextureFormat) -> Self {
        let name = format!("{:?}", format.0);
        match name.strip_prefix("DXGI_FORMAT_") {
            Some(short) => short.to_owned(),
            None => name,
        }
    }
}

/// A case's tree: content written once, then shape operations per copy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeRecipe {
    #[serde(default)]
    pub content: Vec<ContentEntry>,
    /// Applied in order, to `input/` and to each side.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fs_shape: Vec<FsShape>,
}

/// One file or directory of the case tree. In JSON: `{"kind": "texture", ...}`.
///
/// Every entry may carry a `note` saying why the case needs it; the
/// materialiser ignores it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContentEntry {
    /// A synthetic Texture, built with DirectXTex from a seeded pattern.
    Texture(TextureEntry),
    /// A file holding exactly this UTF-8 text, such as a readme, or a file
    /// that is not the Asset its extension claims.
    Text {
        path: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// The escape for bytes the vocabulary cannot express: exactly one of
    /// inline `base64` or a `fixture` file under `crates/cao-parity/fixtures/`.
    Raw {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base64: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fixture: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// A directory, which may stay empty.
    Directory {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
}

impl ContentEntry {
    /// The entry's path, relative to the case root.
    pub fn path(&self) -> &str {
        match self {
            Self::Texture(texture) => &texture.path,
            Self::Text { path, .. } | Self::Raw { path, .. } | Self::Directory { path, .. } => path,
        }
    }
}

/// A synthetic Texture. The container follows the extension: `.dds` or `.tga`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextureEntry {
    pub path: String,
    /// The stored format. Block-compressed formats are encoded on the CPU from
    /// the pattern; other formats are converted from it.
    pub format: TextureFormat,
    pub width: u32,
    pub height: u32,
    /// The mip count, as DirectXTex counts it: 1 is the top level only and 0 is
    /// the full chain.
    #[serde(default = "one")]
    pub mip_levels: u32,
    /// The array size, or the number of cubes for a cubemap.
    #[serde(default = "one")]
    pub array_size: u32,
    #[serde(default, skip_serializing_if = "is_false")]
    pub cubemap: bool,
    #[serde(default)]
    pub header: DdsHeader,
    #[serde(default)]
    pub pattern: Pattern,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fault: Option<Fault>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

fn one() -> u32 {
    1
}

fn is_false(value: &bool) -> bool {
    !value
}

/// Which DDS header a Texture is written with. `legacy` is an error for a
/// format only the DX10 header can describe, so a recipe never silently gets
/// the other header.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DdsHeader {
    #[default]
    Legacy,
    Dx10,
}

/// The seeded content of a Texture's pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pattern {
    /// Red across, green down, a seeded blue; opaque.
    #[default]
    Gradient,
    /// Seeded colour noise; opaque.
    Noise,
    /// A checkerboard of two seeded colours with hard edges; opaque.
    Edges,
    /// A seeded colour whose alpha ramps from 0 to 255 across.
    AlphaRamp,
    /// A gradient whose alpha is a binary checkerboard mask.
    Mask,
    /// Normal-map-like content: smooth bumps in red and green, blue near 255.
    Normal,
    /// Seeded bytes written straight into every level in the stored format,
    /// with no conversion. The only pattern for typeless formats.
    Bytes,
}

/// A decorator that damages a generated file, for Quarantine and load-failure
/// cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fault {
    /// Keeps only the first `n` bytes: `{"truncate": n}`.
    Truncate(u64),
    /// An empty file.
    Zero,
    /// Seeded garbage of the generated file's length.
    Garbage,
}

/// A filesystem-shape operation. In JSON: `{"op": "hardlink", ...}`.
///
/// Targets are paths in the same copy, so each side's links point into its own
/// tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum FsShape {
    /// `path` becomes a second hard link to the file `target`.
    Hardlink {
        path: String,
        target: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// `path` becomes a directory junction to `target`.
    Junction {
        path: String,
        target: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// `path` becomes a file symlink to `target`. A case with one is not run
    /// where the process cannot create symlinks.
    FileSymlink {
        path: String,
        target: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// Sets the read-only attribute on `path`.
    Readonly {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// Renames the content entry `from` to `path`, whose name is a reserved
    /// device name such as `NUL.dds`, through a `\\?\` path, the only way to
    /// create one.
    ReservedName {
        path: String,
        from: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
}

impl FsShape {
    /// The path the operation creates or changes.
    pub fn path(&self) -> &str {
        match self {
            Self::Hardlink { path, .. }
            | Self::Junction { path, .. }
            | Self::FileSymlink { path, .. }
            | Self::Readonly { path, .. }
            | Self::ReservedName { path, .. } => path,
        }
    }

    /// The existing path the operation reads, if any.
    pub fn source(&self) -> Option<&str> {
        match self {
            Self::Hardlink { target, .. }
            | Self::Junction { target, .. }
            | Self::FileSymlink { target, .. } => Some(target),
            Self::ReservedName { from, .. } => Some(from),
            Self::Readonly { .. } => None,
        }
    }
}
