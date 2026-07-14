#![forbid(unsafe_code)]
//! Backend-neutral domain values and behavior for Tracetide.

use std::fmt::{self, Write as _};
use std::path::Path;

/// Reserved identity of an immutable built-in profile.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ActiveProfileId {
    /// Fallout 4 built-in profile.
    Fo4,
    /// Skyrim Special Edition built-in profile and fresh-install default.
    #[default]
    Sse,
    /// Classic Skyrim built-in profile.
    Tes5,
}

impl ActiveProfileId {
    /// Every built-in profile in deterministic identity order.
    pub const ALL: [Self; 3] = [Self::Fo4, Self::Sse, Self::Tes5];

    /// Returns the immutable definition owned by this stable identity.
    #[must_use]
    pub const fn definition(self) -> &'static BuiltInProfileDefinition {
        match self {
            Self::Fo4 => &FO4_DEFINITION,
            Self::Sse => &SSE_DEFINITION,
            Self::Tes5 => &TES5_DEFINITION,
        }
    }

    /// Returns the canonical persisted spelling of this reserved identity.
    #[must_use]
    pub const fn stable_id(self) -> &'static str {
        match self {
            Self::Fo4 => "FO4",
            Self::Sse => "SSE",
            Self::Tes5 => "TES5",
        }
    }

    /// Parses only a canonical reserved identity, independent of names and directories.
    #[must_use]
    pub fn from_stable_id(value: &str) -> Option<Self> {
        match value {
            "FO4" => Some(Self::Fo4),
            "SSE" => Some(Self::Sse),
            "TES5" => Some(Self::Tes5),
            _ => None,
        }
    }

    /// Returns the fixed array position used only for typed built-in collections.
    #[must_use]
    pub const fn storage_index(self) -> usize {
        match self {
            Self::Fo4 => 0,
            Self::Sse => 1,
            Self::Tes5 => 2,
        }
    }
}

/// A remembered absolute Windows asset path that may currently be unavailable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetPath(String);

impl AssetPath {
    /// Validates and owns one non-empty absolute path without requiring it to exist.
    ///
    /// # Errors
    ///
    /// Returns [`AssetPathError::Empty`] for an empty value,
    /// [`AssetPathError::ForbiddenCharacter`] for control characters, or
    /// [`AssetPathError::NotAbsolute`] for a relative or drive-relative value.
    pub fn new(path: impl Into<String>) -> Result<Self, AssetPathError> {
        let path = path.into();
        if path.is_empty() {
            return Err(AssetPathError::Empty);
        }
        if path.chars().any(|character| character <= '\u{1f}') {
            return Err(AssetPathError::ForbiddenCharacter);
        }
        if !Path::new(&path).is_absolute() {
            return Err(AssetPathError::NotAbsolute);
        }
        Ok(Self(path))
    }

    /// Returns the validated path text exactly as supplied by the user.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Reason a remembered asset path could not enter the domain model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssetPathError {
    /// An empty path must be represented by the overlay's unset state.
    Empty,
    /// Windows control characters cannot be represented safely in portable state.
    ForbiddenCharacter,
    /// The path is relative or drive-relative rather than absolute.
    NotAbsolute,
}

impl fmt::Display for AssetPathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("asset path is empty"),
            Self::ForbiddenCharacter => {
                formatter.write_str("asset path contains a forbidden control character")
            }
            Self::NotAbsolute => formatter.write_str("asset path is not absolute"),
        }
    }
}

impl std::error::Error for AssetPathError {}

/// Categories of processing supported by one immutable profile definition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProfileCapabilities {
    archives: bool,
    meshes: bool,
    textures: bool,
    animations: bool,
}

impl ProfileCapabilities {
    /// Returns whether archive choices participate in the effective configuration.
    #[must_use]
    pub const fn archives(self) -> bool {
        self.archives
    }

    /// Returns whether mesh choices participate in the effective configuration.
    #[must_use]
    pub const fn meshes(self) -> bool {
        self.meshes
    }

    /// Returns whether texture choices participate in the effective configuration.
    #[must_use]
    pub const fn textures(self) -> bool {
        self.textures
    }

    /// Returns whether animation choices participate in the effective configuration.
    #[must_use]
    pub const fn animations(self) -> bool {
        self.animations
    }
}

/// Game-specific archive container selected by a built-in definition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArchiveFormatTarget {
    /// Fallout 4 BA2 archives.
    Fo4Ba2,
    /// Skyrim Special Edition version-105 BSA archives.
    SseBsa,
    /// Classic Skyrim version-104 BSA archives.
    Tes5Bsa,
}

/// Typed NIF version tuple selected by a built-in definition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MeshFormatTarget {
    file_version: u32,
    user_version: u32,
    stream_version: u32,
}

impl MeshFormatTarget {
    /// Returns the NIF file-version word.
    #[must_use]
    pub const fn file_version(self) -> u32 {
        self.file_version
    }

    /// Returns the NIF user version.
    #[must_use]
    pub const fn user_version(self) -> u32 {
        self.user_version
    }

    /// Returns the NIF stream version.
    #[must_use]
    pub const fn stream_version(self) -> u32 {
        self.stream_version
    }
}

/// Default compressed texture format selected by a built-in definition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextureFormatTarget {
    /// BC3/DXT5 used by classic Skyrim.
    Bc3Unorm,
    /// BC7 used by Fallout 4 and Skyrim Special Edition.
    Bc7Unorm,
}

/// One exact behavioral-oracle artifact identity backing a built-in definition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuthenticatedProfileAsset {
    contract_key: &'static str,
    oracle_path: &'static str,
    size_bytes: u64,
    sha256: &'static str,
}

impl AuthenticatedProfileAsset {
    /// Returns the stable key used by the fork-owned authenticated inventory.
    #[must_use]
    pub const fn contract_key(self) -> &'static str {
        self.contract_key
    }

    /// Returns the stable oracle evidence key; archive paths retain their distribution spelling.
    #[must_use]
    pub const fn oracle_path(self) -> &'static str {
        self.oracle_path
    }

    /// Returns the exact artifact size established by oracle characterization.
    #[must_use]
    pub const fn size_bytes(self) -> u64 {
        self.size_bytes
    }

    /// Returns the uppercase SHA-256 established by oracle characterization.
    #[must_use]
    pub const fn sha256(self) -> &'static str {
        self.sha256
    }
}

/// Fork-owned immutable definition for one built-in game profile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BuiltInProfileDefinition {
    id: ActiveProfileId,
    display_name: &'static str,
    resource_directory: &'static str,
    state_directory: &'static str,
    capabilities: ProfileCapabilities,
    archive_format: ArchiveFormatTarget,
    max_archive_uncompressed_size: f64,
    mesh_format: Option<MeshFormatTarget>,
    texture_format: TextureFormatTarget,
    texture_convert_tga: bool,
    texture_unwanted_formats: &'static [u32],
    texture_compress_interface: bool,
    authenticated_assets: &'static [AuthenticatedProfileAsset],
}

impl BuiltInProfileDefinition {
    /// Returns the stable reserved identity, independent of all names and paths.
    #[must_use]
    pub const fn id(self) -> ActiveProfileId {
        self.id
    }

    /// Returns the presentation name, which is not used as identity or storage.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        self.display_name
    }

    /// Returns the explicit bundled-resource directory, independent of identity spelling.
    #[must_use]
    pub const fn resource_directory(self) -> &'static str {
        self.resource_directory
    }

    /// Returns the explicit mutable-state directory, independent of display/resource names.
    #[must_use]
    pub const fn state_directory(self) -> &'static str {
        self.state_directory
    }

    /// Returns the processing categories supported by this definition.
    #[must_use]
    pub const fn capabilities(self) -> ProfileCapabilities {
        self.capabilities
    }

    /// Returns the selected archive container family.
    #[must_use]
    pub const fn archive_format(self) -> ArchiveFormatTarget {
        self.archive_format
    }

    /// Returns the exact legacy byte threshold authenticated for archive creation.
    #[must_use]
    pub const fn max_archive_uncompressed_size(self) -> f64 {
        self.max_archive_uncompressed_size
    }

    /// Returns the authenticated mesh target, if the source profile defines one.
    #[must_use]
    pub const fn mesh_format(self) -> Option<MeshFormatTarget> {
        self.mesh_format
    }

    /// Returns the profile's default compressed texture target.
    #[must_use]
    pub const fn texture_format(self) -> TextureFormatTarget {
        self.texture_format
    }

    /// Returns whether TGA conversion is enabled by the profile definition.
    #[must_use]
    pub const fn texture_convert_tga(self) -> bool {
        self.texture_convert_tga
    }

    /// Returns the authenticated DXGI codes treated as unwanted source formats.
    #[must_use]
    pub const fn texture_unwanted_formats(self) -> &'static [u32] {
        self.texture_unwanted_formats
    }

    /// Returns whether interface textures participate in compression.
    #[must_use]
    pub const fn texture_compress_interface(self) -> bool {
        self.texture_compress_interface
    }

    /// Returns the exact oracle artifacts from which this definition was authenticated.
    #[must_use]
    pub const fn authenticated_assets(self) -> &'static [AuthenticatedProfileAsset] {
        self.authenticated_assets
    }
}

const FO4_ASSETS: [AuthenticatedProfileAsset; 3] = [
    AuthenticatedProfileAsset {
        contract_key: "profile.ini",
        oracle_path: "profiles/FO4/profile.ini",
        size_bytes: 376,
        sha256: "72CFCE00CBFD878A7F3CE0495C1ED26D25524ABC6F28126B13CC338471EC961D",
    },
    AuthenticatedProfileAsset {
        contract_key: "isBase",
        oracle_path: "profiles/FO4/isBase",
        size_bytes: 2,
        sha256: "B3D510EF04275CA8E698E5B3CBB0ECE3949EF9252F0CDC839E9EE347409A2209",
    },
    AuthenticatedProfileAsset {
        contract_key: "shipped_dummy",
        oracle_path: "profiles/FO4/DummyPlugin.esp",
        size_bytes: 49,
        sha256: "AFFCBDEA9D14FE2199440912B326B9F5B704C34F436B51CEF03730319F404CA1",
    },
];

const SSE_ASSETS: [AuthenticatedProfileAsset; 9] = [
    AuthenticatedProfileAsset {
        contract_key: "profile.ini",
        oracle_path: "profiles/SSE/profile.ini",
        size_bytes: 375,
        sha256: "E569440F1AC9E7DF0F9B1A6485476F186A6BB0AC0A36F3F7BAACECE1649388C1",
    },
    AuthenticatedProfileAsset {
        contract_key: "settings.ini",
        oracle_path: "profiles/SSE/settings.ini",
        size_bytes: 658,
        sha256: "46B31F2F151DF9516E4232D6E497975117211B1FA3D962E445A3F8A9C2C7E91E",
    },
    AuthenticatedProfileAsset {
        contract_key: "customHeadparts.txt",
        oracle_path: "profiles/SSE/customHeadparts.txt",
        size_bytes: 33_801,
        sha256: "A2E6162A2B420B1CA5396A2743B09102A986BE201366CE5729694641DE66104C",
    },
    AuthenticatedProfileAsset {
        contract_key: "customLandscape.txt",
        oracle_path: "profiles/SSE/customLandscape.txt",
        size_bytes: 6_202,
        sha256: "10DA47F392E79741C872C8538CE1F432AD23C74A8C20EB08A9681EFF8A44A9C4",
    },
    AuthenticatedProfileAsset {
        contract_key: "FilesToNotPack.txt",
        oracle_path: "profiles/SSE/FilesToNotPack.txt",
        size_bytes: 382,
        sha256: "34A6565AF7C092AF04B80E3A89F25A6AE93F7D52CB39E49FF983A76091F1418D",
    },
    AuthenticatedProfileAsset {
        contract_key: "ignoredMods.txt",
        oracle_path: "profiles/SSE/ignoredMods.txt",
        size_bytes: 24,
        sha256: "F085A159BD0D05D087BEE4EEE9AD232713E440588B1A63CFD24E8667261312C3",
    },
    AuthenticatedProfileAsset {
        contract_key: "isBase",
        oracle_path: "profiles/SSE/isBase",
        size_bytes: 2,
        sha256: "B3D510EF04275CA8E698E5B3CBB0ECE3949EF9252F0CDC839E9EE347409A2209",
    },
    AuthenticatedProfileAsset {
        contract_key: "shipped_dummy",
        oracle_path: "profiles/SSE/DummyPlugin.esp",
        size_bytes: 128,
        sha256: "ED29D3A93C5802E61EB78227629DB289BC14D71C6AC96D19B7E1A3BA5D13BD51",
    },
    AuthenticatedProfileAsset {
        contract_key: "native_dummy",
        oracle_path: "native-dummy/SSE/DummyPlugin.esp",
        size_bytes: 49,
        sha256: "08F228B84E6798D468472D30E74D550F786226A87F4F69F2DBFDDEC576E5799A",
    },
];

const TES5_ASSETS: [AuthenticatedProfileAsset; 3] = [
    AuthenticatedProfileAsset {
        contract_key: "profile.ini",
        oracle_path: "profiles/TES5/profile.ini",
        size_bytes: 368,
        sha256: "DCFD90F4687680DC3891BDE676003E7F09E1A1AE19F979A2D62844E9B0762111",
    },
    AuthenticatedProfileAsset {
        contract_key: "isBase",
        oracle_path: "profiles/TES5/isBase",
        size_bytes: 2,
        sha256: "B3D510EF04275CA8E698E5B3CBB0ECE3949EF9252F0CDC839E9EE347409A2209",
    },
    AuthenticatedProfileAsset {
        contract_key: "shipped_dummy",
        oracle_path: "profiles/TES5/DummyPlugin.esp",
        size_bytes: 49,
        sha256: "852F2DB6923C2203D60DAA176D5FA27D61A0D0E717B6819386BBFCBFF7FFFEFD",
    },
];

const FO4_DEFINITION: BuiltInProfileDefinition = BuiltInProfileDefinition {
    id: ActiveProfileId::Fo4,
    display_name: "Fallout 4",
    resource_directory: "fallout-4",
    state_directory: "builtin-fo4",
    capabilities: ProfileCapabilities {
        archives: true,
        meshes: false,
        textures: true,
        animations: false,
    },
    archive_format: ArchiveFormatTarget::Fo4Ba2,
    max_archive_uncompressed_size: 4_187_593_113.6,
    mesh_format: Some(MeshFormatTarget {
        file_version: 335_675_399,
        user_version: 12,
        stream_version: 130,
    }),
    texture_format: TextureFormatTarget::Bc7Unorm,
    texture_convert_tga: true,
    texture_unwanted_formats: &[86, 85, 115],
    texture_compress_interface: true,
    authenticated_assets: &FO4_ASSETS,
};

const SSE_DEFINITION: BuiltInProfileDefinition = BuiltInProfileDefinition {
    id: ActiveProfileId::Sse,
    display_name: "Skyrim Special Edition",
    resource_directory: "skyrim-special-edition",
    state_directory: "builtin-sse",
    capabilities: ProfileCapabilities {
        archives: true,
        meshes: true,
        textures: true,
        animations: true,
    },
    archive_format: ArchiveFormatTarget::SseBsa,
    max_archive_uncompressed_size: 2_115_271_393.28,
    mesh_format: Some(MeshFormatTarget {
        file_version: 335_675_399,
        user_version: 12,
        stream_version: 100,
    }),
    texture_format: TextureFormatTarget::Bc7Unorm,
    texture_convert_tga: true,
    texture_unwanted_formats: &[85, 86, 115],
    texture_compress_interface: true,
    authenticated_assets: &SSE_ASSETS,
};

const TES5_DEFINITION: BuiltInProfileDefinition = BuiltInProfileDefinition {
    id: ActiveProfileId::Tes5,
    display_name: "Skyrim",
    resource_directory: "skyrim-classic",
    state_directory: "builtin-tes5",
    capabilities: ProfileCapabilities {
        archives: true,
        meshes: true,
        textures: true,
        animations: false,
    },
    archive_format: ArchiveFormatTarget::Tes5Bsa,
    max_archive_uncompressed_size: 2_118_123_520.0,
    mesh_format: Some(MeshFormatTarget {
        file_version: 335_675_399,
        user_version: 12,
        stream_version: 83,
    }),
    texture_format: TextureFormatTarget::Bc3Unorm,
    texture_convert_tga: false,
    texture_unwanted_formats: &[98, 99],
    texture_compress_interface: true,
    authenticated_assets: &TES5_ASSETS,
};

/// Materializes the authenticated inventory from the immutable domain definitions.
///
/// Runtime startup compares this deterministic representation with the packaged inventory so
/// either side changing independently becomes an installation-integrity failure.
#[must_use]
pub fn authenticated_built_in_profile_contract() -> String {
    let mut contract = String::from(
        "schema_version=1\n\
oracle_file=Cathedral Assets Optimizer 64-23316-5-3-15-1687526925.7z\n\
oracle_size=10410192\n\
oracle_sha256=B25CF0C0C97160B602DD47C252AF2EDE735C27ABE565C9AB84D272306308ABB6\n",
    );
    for profile in ActiveProfileId::ALL {
        let definition = profile.definition();
        let identity = definition.id().stable_id();
        writeln!(contract, "{identity}.identity={identity}")
            .expect("writing an authenticated contract to a String cannot fail");
        writeln!(
            contract,
            "{identity}.display_name={}",
            definition.display_name()
        )
        .expect("writing an authenticated contract to a String cannot fail");
        writeln!(
            contract,
            "{identity}.resource_directory={}",
            definition.resource_directory()
        )
        .expect("writing an authenticated contract to a String cannot fail");
        writeln!(
            contract,
            "{identity}.state_directory={}",
            definition.state_directory()
        )
        .expect("writing an authenticated contract to a String cannot fail");
        let capabilities = [
            ("archives", definition.capabilities().archives()),
            ("meshes", definition.capabilities().meshes()),
            ("textures", definition.capabilities().textures()),
            ("animations", definition.capabilities().animations()),
        ]
        .into_iter()
        .filter_map(|(name, supported)| supported.then_some(name))
        .collect::<Vec<_>>()
        .join(",");
        writeln!(contract, "{identity}.capabilities={capabilities}")
            .expect("writing an authenticated contract to a String cannot fail");
        let archive = match definition.archive_format() {
            ArchiveFormatTarget::Fo4Ba2 => "fo4-ba2",
            ArchiveFormatTarget::SseBsa => "sse-bsa",
            ArchiveFormatTarget::Tes5Bsa => "tes5-bsa",
        };
        writeln!(
            contract,
            "{identity}.archive={archive}:{}",
            definition.max_archive_uncompressed_size()
        )
        .expect("writing an authenticated contract to a String cannot fail");
        let mesh = definition
            .mesh_format()
            .expect("every authenticated built-in records a mesh target");
        let mesh_state = if definition.capabilities().meshes() {
            "enabled"
        } else {
            "disabled"
        };
        writeln!(
            contract,
            "{identity}.mesh={mesh_state}:{}:{}:{}",
            mesh.file_version(),
            mesh.user_version(),
            mesh.stream_version()
        )
        .expect("writing an authenticated contract to a String cannot fail");
        let texture = match definition.texture_format() {
            TextureFormatTarget::Bc3Unorm => "bc3-unorm",
            TextureFormatTarget::Bc7Unorm => "bc7-unorm",
        };
        let unwanted_formats = definition
            .texture_unwanted_formats()
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        writeln!(
            contract,
            "{identity}.texture={texture}:{}:{unwanted_formats}:{}",
            definition.texture_convert_tga(),
            definition.texture_compress_interface()
        )
        .expect("writing an authenticated contract to a String cannot fail");
        for asset in definition.authenticated_assets() {
            writeln!(
                contract,
                "{identity}.{}={}:{}",
                asset.contract_key(),
                asset.size_bytes(),
                asset.sha256()
            )
            .expect("writing an authenticated contract to a String cannot fail");
            writeln!(
                contract,
                "{identity}.{}.oracle_path={}",
                asset.contract_key(),
                asset.oracle_path()
            )
            .expect("writing an authenticated contract to a String cannot fail");
        }
    }
    contract
}

/// Scope used when discovering assets beneath the selected path.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ProcessingMode {
    /// Treat the selected path as one mod tree.
    #[default]
    SingleMod,
}

/// Mutable archive processing choices in a profile overlay.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArchiveChoices {
    extract: bool,
    create: bool,
    delete_backups: bool,
    merge_incompressible: bool,
    merge_textures: bool,
    process_content: bool,
    create_dummies: bool,
    compress: bool,
    delete_source: bool,
}

impl Default for ArchiveChoices {
    fn default() -> Self {
        Self {
            extract: false,
            create: false,
            delete_backups: false,
            merge_incompressible: true,
            merge_textures: false,
            process_content: false,
            create_dummies: true,
            compress: true,
            delete_source: true,
        }
    }
}

macro_rules! bool_getters {
    ($(($name:ident, $field:ident, $doc:literal)),+ $(,)?) => {
        $(
            #[doc = $doc]
            #[must_use]
            pub const fn $name(self) -> bool {
                self.$field
            }
        )+
    };
}

macro_rules! bool_builders {
    ($(($name:ident, $field:ident, $doc:literal)),+ $(,)?) => {
        $(
            #[doc = $doc]
            #[must_use]
            pub const fn $name(mut self, value: bool) -> Self {
                self.$field = value;
                self
            }
        )+
    };
}

impl ArchiveChoices {
    bool_getters!(
        (
            extract,
            extract,
            "Returns whether archives should be extracted."
        ),
        (
            create,
            create,
            "Returns whether archives should be created."
        ),
        (
            delete_backups,
            delete_backups,
            "Returns whether archive backups should be deleted."
        ),
        (
            merge_incompressible,
            merge_incompressible,
            "Returns whether incompressible files should be merged."
        ),
        (
            merge_textures,
            merge_textures,
            "Returns whether texture archives should be merged."
        ),
        (
            process_content,
            process_content,
            "Returns whether extracted archive content should be processed."
        ),
        (
            create_dummies,
            create_dummies,
            "Returns whether canonical dummy plugins should be created."
        ),
        (
            compress,
            compress,
            "Returns whether created archives should be compressed."
        ),
        (
            delete_source,
            delete_source,
            "Returns whether source files should be deleted after archive creation."
        ),
    );

    bool_builders!(
        (
            with_extract,
            extract,
            "Returns owned choices with archive extraction set as requested."
        ),
        (
            with_create,
            create,
            "Returns owned choices with archive creation set as requested."
        ),
        (
            with_delete_backups,
            delete_backups,
            "Returns owned choices with backup deletion set as requested."
        ),
        (
            with_merge_incompressible,
            merge_incompressible,
            "Returns owned choices with incompressible merging set as requested."
        ),
        (
            with_merge_textures,
            merge_textures,
            "Returns owned choices with texture merging set as requested."
        ),
        (
            with_process_content,
            process_content,
            "Returns owned choices with content processing set as requested."
        ),
        (
            with_create_dummies,
            create_dummies,
            "Returns owned choices with dummy creation set as requested."
        ),
        (
            with_compress,
            compress,
            "Returns owned choices with archive compression set as requested."
        ),
        (
            with_delete_source,
            delete_source,
            "Returns owned choices with source deletion set as requested."
        ),
    );
}

/// Mutable texture processing choices in a profile overlay.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextureChoices {
    process_necessary: bool,
    compress: bool,
    generate_mipmaps: bool,
    resize_to_fixed_size: bool,
    target_width: u32,
    target_height: u32,
    resize_by_ratio: bool,
    width_ratio: u32,
    height_ratio: u32,
}

impl Default for TextureChoices {
    fn default() -> Self {
        Self {
            process_necessary: true,
            compress: false,
            generate_mipmaps: false,
            resize_to_fixed_size: false,
            target_width: 2048,
            target_height: 2048,
            resize_by_ratio: false,
            width_ratio: 2,
            height_ratio: 2,
        }
    }
}

impl TextureChoices {
    bool_getters!(
        (
            process_necessary,
            process_necessary,
            "Returns whether required texture repairs should run."
        ),
        (
            compress,
            compress,
            "Returns whether textures should be compressed."
        ),
        (
            generate_mipmaps,
            generate_mipmaps,
            "Returns whether mipmaps should be generated."
        ),
        (
            resize_to_fixed_size,
            resize_to_fixed_size,
            "Returns whether fixed-size resizing is enabled."
        ),
        (
            resize_by_ratio,
            resize_by_ratio,
            "Returns whether ratio resizing is enabled."
        ),
    );

    /// Returns the fixed resize target width.
    #[must_use]
    pub const fn target_width(self) -> u32 {
        self.target_width
    }

    /// Returns the fixed resize target height.
    #[must_use]
    pub const fn target_height(self) -> u32 {
        self.target_height
    }

    /// Returns the width divisor used for ratio resizing.
    #[must_use]
    pub const fn width_ratio(self) -> u32 {
        self.width_ratio
    }

    /// Returns the height divisor used for ratio resizing.
    #[must_use]
    pub const fn height_ratio(self) -> u32 {
        self.height_ratio
    }

    bool_builders!(
        (
            with_process_necessary,
            process_necessary,
            "Returns owned choices with required processing set as requested."
        ),
        (
            with_compress,
            compress,
            "Returns owned choices with texture compression set as requested."
        ),
        (
            with_generate_mipmaps,
            generate_mipmaps,
            "Returns owned choices with mip generation set as requested."
        ),
        (
            with_resize_to_fixed_size,
            resize_to_fixed_size,
            "Returns owned choices with fixed-size resizing set as requested."
        ),
        (
            with_resize_by_ratio,
            resize_by_ratio,
            "Returns owned choices with ratio resizing set as requested."
        ),
    );

    /// Returns owned choices with the fixed resize dimensions replaced.
    #[must_use]
    pub const fn with_target_size(mut self, width: u32, height: u32) -> Self {
        self.target_width = width;
        self.target_height = height;
        self
    }

    /// Returns owned choices with the ratio divisors replaced.
    #[must_use]
    pub const fn with_ratios(mut self, width: u32, height: u32) -> Self {
        self.width_ratio = width;
        self.height_ratio = height;
        self
    }
}

/// Mutable mesh processing choices in a profile overlay.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MeshChoices {
    optimization_level: u8,
    handle_headparts: bool,
    resave: bool,
}

impl Default for MeshChoices {
    fn default() -> Self {
        Self {
            optimization_level: 0,
            handle_headparts: true,
            resave: false,
        }
    }
}

impl MeshChoices {
    /// Returns the selected mesh optimization level.
    #[must_use]
    pub const fn optimization_level(self) -> u8 {
        self.optimization_level
    }

    bool_getters!(
        (
            handle_headparts,
            handle_headparts,
            "Returns whether headpart-specific handling is enabled."
        ),
        (resave, resave, "Returns whether meshes should be resaved."),
    );

    bool_builders!(
        (
            with_handle_headparts,
            handle_headparts,
            "Returns owned choices with headpart handling set as requested."
        ),
        (
            with_resave,
            resave,
            "Returns owned choices with mesh resaving set as requested."
        ),
    );

    /// Returns owned choices with the requested optimization level.
    #[must_use]
    pub const fn with_optimization_level(mut self, optimization_level: u8) -> Self {
        self.optimization_level = optimization_level;
        self
    }
}

/// Mutable animation processing choices in a profile overlay.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AnimationChoices {
    optimize: bool,
}

impl AnimationChoices {
    /// Returns whether animation optimization is requested.
    #[must_use]
    pub const fn optimize(self) -> bool {
        self.optimize
    }

    /// Returns an owned choice with animation optimization set as requested.
    #[must_use]
    pub const fn with_optimize(mut self, optimize: bool) -> Self {
        self.optimize = optimize;
        self
    }
}

/// Mutable processing choices and remembered path applied over one profile definition.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProfileOverlay {
    mode: ProcessingMode,
    asset_path: Option<AssetPath>,
    dry_run: bool,
    debug_log: bool,
    archives: ArchiveChoices,
    textures: TextureChoices,
    meshes: MeshChoices,
    animations: AnimationChoices,
}

impl ProfileOverlay {
    /// Returns the selected asset-discovery scope.
    #[must_use]
    pub const fn mode(&self) -> ProcessingMode {
        self.mode
    }

    /// Returns the remembered absolute asset path, or `None` before selection.
    #[must_use]
    pub const fn asset_path(&self) -> Option<&AssetPath> {
        self.asset_path.as_ref()
    }

    /// Returns whether processing should report writes without committing them.
    #[must_use]
    pub const fn dry_run(&self) -> bool {
        self.dry_run
    }

    /// Returns whether verbose diagnostic logging is requested.
    #[must_use]
    pub const fn debug_log(&self) -> bool {
        self.debug_log
    }

    /// Returns the stored archive choices even when the active definition masks them.
    #[must_use]
    pub const fn archives(&self) -> ArchiveChoices {
        self.archives
    }

    /// Returns the stored texture choices even when the active definition masks them.
    #[must_use]
    pub const fn textures(&self) -> TextureChoices {
        self.textures
    }

    /// Returns the stored mesh choices even when the active definition masks them.
    #[must_use]
    pub const fn meshes(&self) -> MeshChoices {
        self.meshes
    }

    /// Returns the stored animation choices even when the active definition masks them.
    #[must_use]
    pub const fn animations(&self) -> AnimationChoices {
        self.animations
    }

    /// Returns an owned overlay with the requested dry-run choice.
    #[must_use]
    pub fn with_dry_run(&self, dry_run: bool) -> Self {
        let mut overlay = self.clone();
        overlay.dry_run = dry_run;
        overlay
    }

    /// Returns an owned overlay with the requested processing scope.
    #[must_use]
    pub fn with_mode(&self, mode: ProcessingMode) -> Self {
        let mut overlay = self.clone();
        overlay.mode = mode;
        overlay
    }

    /// Returns an owned overlay with a validated remembered asset path.
    #[must_use]
    pub fn with_asset_path(&self, asset_path: AssetPath) -> Self {
        let mut overlay = self.clone();
        overlay.asset_path = Some(asset_path);
        overlay
    }

    /// Returns an owned overlay with no remembered asset path.
    #[must_use]
    pub fn without_asset_path(&self) -> Self {
        let mut overlay = self.clone();
        overlay.asset_path = None;
        overlay
    }

    /// Returns an owned overlay with the requested diagnostic verbosity.
    #[must_use]
    pub fn with_debug_log(&self, debug_log: bool) -> Self {
        let mut overlay = self.clone();
        overlay.debug_log = debug_log;
        overlay
    }

    /// Returns an owned overlay with the requested archive choices.
    #[must_use]
    pub fn with_archives(&self, archives: ArchiveChoices) -> Self {
        let mut overlay = self.clone();
        overlay.archives = archives;
        overlay
    }

    /// Returns an owned overlay with the requested texture choices.
    #[must_use]
    pub fn with_textures(&self, textures: TextureChoices) -> Self {
        let mut overlay = self.clone();
        overlay.textures = textures;
        overlay
    }

    /// Returns an owned overlay with the requested mesh choices.
    #[must_use]
    pub fn with_meshes(&self, meshes: MeshChoices) -> Self {
        let mut overlay = self.clone();
        overlay.meshes = meshes;
        overlay
    }

    /// Returns an owned overlay with the requested animation choices.
    #[must_use]
    pub fn with_animations(&self, animations: AnimationChoices) -> Self {
        let mut overlay = self.clone();
        overlay.animations = animations;
        overlay
    }

    /// Resolves capability masking without changing the stored overlay.
    #[must_use]
    pub const fn effective_for(&self, profile: ActiveProfileId) -> EffectiveProfileOverlay<'_> {
        EffectiveProfileOverlay {
            overlay: self,
            definition: profile.definition(),
        }
    }
}

/// Read-only effective choices after unsupported categories have been masked.
#[derive(Clone, Copy, Debug)]
pub struct EffectiveProfileOverlay<'a> {
    overlay: &'a ProfileOverlay,
    definition: &'static BuiltInProfileDefinition,
}

impl EffectiveProfileOverlay<'_> {
    /// Returns the immutable definition governing this resolution.
    #[must_use]
    pub const fn definition(&self) -> &'static BuiltInProfileDefinition {
        self.definition
    }

    /// Returns common stored choices that are never capability-masked.
    #[must_use]
    pub const fn common(&self) -> &ProfileOverlay {
        self.overlay
    }

    /// Returns archive choices only when the definition supports archives.
    #[must_use]
    pub const fn archives(&self) -> Option<ArchiveChoices> {
        if self.definition.capabilities.archives {
            Some(self.overlay.archives)
        } else {
            None
        }
    }

    /// Returns texture choices only when the definition supports textures.
    #[must_use]
    pub const fn textures(&self) -> Option<TextureChoices> {
        if self.definition.capabilities.textures {
            Some(self.overlay.textures)
        } else {
            None
        }
    }

    /// Returns mesh choices only when the definition supports meshes.
    #[must_use]
    pub const fn meshes(&self) -> Option<MeshChoices> {
        if self.definition.capabilities.meshes {
            Some(self.overlay.meshes)
        } else {
            None
        }
    }

    /// Returns animation choices only when the definition supports animations.
    #[must_use]
    pub const fn animations(&self) -> Option<AnimationChoices> {
        if self.definition.capabilities.animations {
            Some(self.overlay.animations)
        } else {
            None
        }
    }
}

/// Visible reason that a persisted selection resolved to the SSE default.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileSelectionFallback {
    /// No active-profile field was present in otherwise valid global state.
    Missing,
    /// The stored identity did not name a loadable built-in profile.
    Invalid,
}

/// Persisted setup values that are authoritative before a processing run begins.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetupState {
    active_profile: ActiveProfileId,
    overlays: [ProfileOverlay; 3],
    selection_fallback: Option<ProfileSelectionFallback>,
}

impl Default for SetupState {
    fn default() -> Self {
        Self {
            active_profile: ActiveProfileId::Sse,
            overlays: [
                ProfileOverlay::default(),
                ProfileOverlay::default(),
                ProfileOverlay::default(),
            ],
            selection_fallback: None,
        }
    }
}

impl SetupState {
    /// Returns the stable identity of the profile governing setup.
    #[must_use]
    pub const fn active_profile(&self) -> ActiveProfileId {
        self.active_profile
    }

    /// Returns an owned setup value with the supplied stable active-profile identity.
    #[must_use]
    pub fn with_active_profile(&self, active_profile: ActiveProfileId) -> Self {
        let mut setup = self.clone();
        setup.active_profile = active_profile;
        setup.selection_fallback = None;
        setup
    }

    /// Returns an owned setup visibly resolved to SSE for the supplied fallback reason.
    #[must_use]
    pub fn with_selection_fallback(&self, reason: ProfileSelectionFallback) -> Self {
        let mut setup = self.clone();
        setup.active_profile = ActiveProfileId::Sse;
        setup.selection_fallback = Some(reason);
        setup
    }

    /// Returns the visible selection fallback, if startup had to choose SSE.
    #[must_use]
    pub const fn selection_fallback(&self) -> Option<ProfileSelectionFallback> {
        self.selection_fallback
    }

    /// Returns the processing choices isolated to the active profile.
    #[must_use]
    pub fn profile_overlay(&self) -> &ProfileOverlay {
        self.overlay_for(self.active_profile)
    }

    /// Returns the processing choices isolated to a specified stable profile identity.
    #[must_use]
    pub fn overlay_for(&self, profile: ActiveProfileId) -> &ProfileOverlay {
        &self.overlays[profile.storage_index()]
    }

    /// Returns an owned setup value with the active profile's overlay replaced.
    #[must_use]
    pub fn with_profile_overlay(&self, profile_overlay: ProfileOverlay) -> Self {
        self.with_profile_overlay_for(self.active_profile, profile_overlay)
    }

    /// Returns an owned setup value with only the specified profile's overlay replaced.
    #[must_use]
    pub fn with_profile_overlay_for(
        &self,
        profile: ActiveProfileId,
        profile_overlay: ProfileOverlay,
    ) -> Self {
        let mut setup = self.clone();
        setup.overlays[profile.storage_index()] = profile_overlay;
        setup
    }

    /// Returns an owned setup with one profile reset to documented defaults.
    #[must_use]
    pub fn reset_profile_overlay(&self, profile: ActiveProfileId) -> Self {
        self.with_profile_overlay_for(profile, ProfileOverlay::default())
    }

    /// Resolves the active overlay against its immutable definition without erasing it.
    #[must_use]
    pub fn effective_profile_overlay(&self) -> EffectiveProfileOverlay<'_> {
        self.profile_overlay().effective_for(self.active_profile)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ActiveProfileId, AssetPath, AssetPathError, ProcessingMode, ProfileOverlay,
        authenticated_built_in_profile_contract,
    };

    #[test]
    fn built_in_profiles_expose_authenticated_capabilities_and_fresh_overlay_defaults() {
        assert_eq!(
            authenticated_built_in_profile_contract(),
            include_str!("../../../resources/profiles/built-ins.state")
        );
        let fo4 = ActiveProfileId::Fo4.definition();
        let sse = ActiveProfileId::Sse.definition();
        let tes5 = ActiveProfileId::Tes5.definition();

        assert_eq!(fo4.id(), ActiveProfileId::Fo4);
        assert_eq!(fo4.display_name(), "Fallout 4");
        assert_eq!(fo4.resource_directory(), "fallout-4");
        assert_eq!(fo4.state_directory(), "builtin-fo4");
        assert_eq!(sse.display_name(), "Skyrim Special Edition");
        assert_eq!(sse.resource_directory(), "skyrim-special-edition");
        assert_eq!(sse.state_directory(), "builtin-sse");
        assert_eq!(tes5.display_name(), "Skyrim");
        assert!(fo4.capabilities().archives());
        assert!(!fo4.capabilities().meshes());
        assert!(!fo4.capabilities().animations());
        assert_eq!(
            fo4.mesh_format()
                .expect("FO4 should retain its authenticated disabled mesh target")
                .stream_version(),
            130
        );
        assert!(sse.capabilities().archives());
        assert!(sse.capabilities().meshes());
        assert!(sse.capabilities().textures());
        assert!(sse.capabilities().animations());
        assert!(tes5.capabilities().meshes());
        assert!(!tes5.capabilities().animations());
        assert_eq!(fo4.max_archive_uncompressed_size(), 4_187_593_113.6);
        assert_eq!(sse.max_archive_uncompressed_size(), 2_115_271_393.28);
        assert_eq!(tes5.max_archive_uncompressed_size(), 2_118_123_520.0);
        assert_eq!(fo4.texture_unwanted_formats(), &[86, 85, 115]);
        assert_eq!(sse.authenticated_assets().len(), 9);
        assert!(sse.authenticated_assets().iter().any(|asset| {
            asset.oracle_path() == "profiles/SSE/settings.ini"
                && asset.size_bytes() == 658
                && asset.sha256()
                    == "46B31F2F151DF9516E4232D6E497975117211B1FA3D962E445A3F8A9C2C7E91E"
        }));

        let overlay = ProfileOverlay::default();
        assert_eq!(overlay.mode(), ProcessingMode::SingleMod);
        assert_eq!(overlay.asset_path(), None);
        assert!(!overlay.dry_run());
        assert!(!overlay.debug_log());
        assert!(!overlay.archives().extract());
        assert!(!overlay.archives().create());
        assert!(!overlay.archives().delete_backups());
        assert!(overlay.archives().merge_incompressible());
        assert!(!overlay.archives().merge_textures());
        assert!(!overlay.archives().process_content());
        assert!(overlay.archives().create_dummies());
        assert!(overlay.archives().compress());
        assert!(overlay.archives().delete_source());
        assert!(overlay.textures().process_necessary());
        assert!(!overlay.textures().compress());
        assert!(!overlay.textures().generate_mipmaps());
        assert!(!overlay.textures().resize_to_fixed_size());
        assert!(!overlay.textures().resize_by_ratio());
        assert_eq!(overlay.textures().target_width(), 2048);
        assert_eq!(overlay.textures().target_height(), 2048);
        assert_eq!(overlay.textures().width_ratio(), 2);
        assert_eq!(overlay.textures().height_ratio(), 2);
        assert_eq!(overlay.meshes().optimization_level(), 0);
        assert!(overlay.meshes().handle_headparts());
        assert!(!overlay.meshes().resave());
        assert!(!overlay.animations().optimize());
        assert_eq!(
            AssetPath::new("relative\\assets"),
            Err(AssetPathError::NotAbsolute)
        );
        assert_eq!(AssetPath::new(""), Err(AssetPathError::Empty));
        assert_eq!(
            AssetPath::new("E:\\Mods\\safe\nactive_profile=FO4"),
            Err(AssetPathError::ForbiddenCharacter)
        );
        assert_eq!(
            AssetPath::new(r"E:\Mods\Example")
                .expect("an absolute Windows path should validate")
                .as_str(),
            r"E:\Mods\Example"
        );
    }
}
