//! The Selected Profile: every profile fact one Optimization Run uses, read
//! once, strictly, during Preparing (spec #566).
//!
//! [`SelectedProfile::read`] is the module's one read. Routing, Archive
//! extraction and finalization, Texture and Mesh optimization and the Headpart
//! Mesh scan all take their profile facts from the value it returns, so no
//! part of a run can see a different version of the profile than another.

use cao_archive::{Game, Settings};
use cao_core::routing::{ProfileCapabilities, ProfileCapability};
use cao_profiles::{BsaGame, ProfileError, ProfileSettings, Profiles};
use directxtex::DXGI_FORMAT;
use nifly_sys::NifVersion;

use crate::textures::TextureProfile;

/// The profile facts one Optimization Run reads once, during Preparing.
#[derive(Debug, Clone)]
pub struct SelectedProfile {
    capabilities: ProfileCapabilities,
    conversion_preference: bool,
    /// The one Archive settings value the routing extension, the packer's
    /// naming rules and the Headpart scan's plugin extensions all come from.
    archive: Settings,
    texture_profile: TextureProfile,
    mesh_target: NifVersion,
    ignored_mods: Vec<String>,
    packing_exclusions: Vec<String>,
    custom_headparts: Vec<String>,
}

impl SelectedProfile {
    /// Reads the profile named `identity` among `profiles`: its `profile.ini`,
    /// `ignoredMods.txt`, `FilesToNotPack.txt` and `customHeadparts.txt`, each
    /// once.
    ///
    /// # Errors
    /// The [`ProfileError`] when `profile.ini` cannot be read strictly, which
    /// includes an unsupported `bsaGame`, or when `ignoredMods.txt` exists but
    /// cannot be read. The other two lists never fail the read.
    pub fn read(profiles: &Profiles, identity: &str) -> Result<Self, ProfileError> {
        let profile = profiles.open(identity);
        // Strict, as C++ run setup rejected any QSettings status but NoError.
        let settings = profile.load_settings_checked()?;
        let ignored_mods = profile.ignored_mods()?;
        let archive = archive_settings(&settings);
        Ok(Self {
            capabilities: capabilities(&settings, &archive),
            conversion_preference: settings.textures_convert_tga,
            archive,
            texture_profile: texture_profile(&settings),
            mesh_target: mesh_target(&settings),
            ignored_mods,
            // A missing or unreadable list is empty, as in C++; finalization
            // logs it.
            packing_exclusions: profile.files_to_not_pack(),
            // The same holds here; the headpart scan logs it.
            custom_headparts: profile.custom_headparts(),
        })
    }

    /// The Profile Capabilities the run's Routing Policy compiles against,
    /// including the Archive extension.
    pub fn capabilities(&self) -> &ProfileCapabilities {
        &self.capabilities
    }

    /// The Conversion Preference: the profile's `texturesConvertTga` setting,
    /// which the Textures tab's "Convert TGA" checkbox writes. Preparing adds
    /// conversion to requested native Texture work only when it is on.
    pub fn conversion_preference(&self) -> bool {
        self.conversion_preference
    }

    /// The Archive settings of the profile's game, with the profile's maximum
    /// uncompressed Archive size applied. The Archive extension in
    /// [`Self::capabilities`] is this value's.
    pub fn archive_settings(&self) -> &Settings {
        &self.archive
    }

    /// The Texture profile: the target format, the unwanted formats and
    /// whether interface Textures are compressed.
    pub fn texture_profile(&self) -> &TextureProfile {
        &self.texture_profile
    }

    /// The version the run writes Meshes as.
    pub fn mesh_target(&self) -> NifVersion {
        self.mesh_target
    }

    /// The Mod Exclusion names of `ignoredMods.txt`, which Several Mods
    /// Preparing skips.
    pub fn ignored_mods(&self) -> &[String] {
        &self.ignored_mods
    }

    /// The Packing Exclusion rules of `FilesToNotPack.txt`, which Archive
    /// Finalization leaves loose.
    pub fn packing_exclusions(&self) -> &[String] {
        &self.packing_exclusions
    }

    /// The Headpart Meshes `customHeadparts.txt` lists, which the Headpart
    /// Mesh scan adds to those the plugins name.
    pub fn custom_headparts(&self) -> &[String] {
        &self.custom_headparts
    }
}

/// The Profile Capabilities `settings` declare, for the composition root's
/// pre-start capability check, which reads `profile.ini` leniently rather than
/// through [`SelectedProfile::read`].
///
/// The capabilities are derived exactly as the read derives them. Only that
/// check uses this; #570 removes the check, and this with it.
pub(crate) fn capabilities_of(settings: &ProfileSettings) -> ProfileCapabilities {
    capabilities(settings, &archive_settings(settings))
}

/// The Archive settings of a profile's game, with its maximum Archive size
/// raised to the profile's `maxBsaUncompressedSize` when that is larger, as
/// C++ `archiveSettings` did.
fn archive_settings(settings: &ProfileSettings) -> Settings {
    Settings::get(archive_game(settings.bsa_game))
        .with_profile_max_size(settings.max_bsa_uncompressed_size)
}

/// The `cao-archive` game whose tables a profile's `bsaGame` names.
fn archive_game(game: BsaGame) -> Game {
    match game {
        BsaGame::Tes5 => Game::Tes5,
        BsaGame::Sse => Game::Sse,
        BsaGame::Fo4 => Game::Fo4,
    }
}

/// The Profile Capabilities a profile's settings declare, as C++
/// `ApplicationRunConfigurationProvider` and `factsFromSelectedProfile` map them.
///
/// Each enabled Asset Kind supports all of its work, and the Archive extension
/// is `archive`'s, so routing and the Archive rules agree on it.
fn capabilities(settings: &ProfileSettings, archive: &Settings) -> ProfileCapabilities {
    let textures = settings.textures_enabled;
    let meshes = settings.meshes_enabled;
    let archives = settings.bsa_enabled;
    let supported = [
        (textures, ProfileCapability::NativeTextureOptimization),
        (textures, ProfileCapability::ConvertibleTextureConversion),
        (meshes, ProfileCapability::StandardMeshOptimization),
        (meshes, ProfileCapability::TerrainMeshOptimization),
        (
            settings.animations_enabled,
            ProfileCapability::AnimationOptimization,
        ),
        (archives, ProfileCapability::ArchiveExtraction),
        // Reference maintenance belongs to Texture conversion.
        (textures, ProfileCapability::MeshReferenceMaintenance),
        (archives, ProfileCapability::ArchiveCreation),
    ];
    let capabilities: Vec<_> = supported
        .into_iter()
        .filter(|(supported, _)| *supported)
        .map(|(_, capability)| capability)
        .collect();
    ProfileCapabilities::define(archive.extension, &capabilities)
}

/// The profile's Texture settings, as the texture optimizer takes them.
fn texture_profile(settings: &ProfileSettings) -> TextureProfile {
    TextureProfile {
        format: DXGI_FORMAT::from(settings.textures_format),
        unwanted_formats: settings
            .textures_unwanted_formats
            .iter()
            .map(|&format| DXGI_FORMAT::from(format))
            .collect(),
        compress_interface: settings.textures_compress_interface,
    }
}

/// The version a profile writes Meshes as (`meshesFileVersion`, `meshesUser`,
/// `meshesStream`), which C++ handed nifly through `SetFile`, `SetUser` and
/// `SetStream`.
fn mesh_target(settings: &ProfileSettings) -> NifVersion {
    NifVersion {
        file: settings.meshes_file_version,
        user: settings.meshes_user,
        stream: settings.meshes_stream,
    }
}
