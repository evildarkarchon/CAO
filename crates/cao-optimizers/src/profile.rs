//! The one place a Profile becomes run facts.
//!
//! From a Profile's settings (`profile.ini`), this module derives its Archive
//! Rules (`cao-archive`'s [`Settings`], built once per read), its Profile
//! Capabilities with the Archive extension those rules write, and the
//! backend's `TextureProfile` (its per-run texture settings) and Mesh Target.
//! Preparing compiles a run's routing and builds its packer and work from
//! these, and the GUI gates its asset tabs from the same Profile
//! Capabilities, so the two never disagree about what a Profile supports.

use cao_archive::Settings;
use cao_core::routing::{ProfileCapabilities, ProfileCapability};
use cao_profiles::{ClosedSet, ProfileSettings};
use directxtex::DXGI_FORMAT;
use nifly_sys::NifVersion;

use crate::textures::TextureProfile;

/// A Profile's Archive Rules: its game's tables with its Archive Limit, its
/// `maxBsaUncompressedSize` or the game's default when it gives none, never
/// past the game's Archive Size Cap (see [`Settings::for_profile`]).
///
/// Build it once per Profile read and derive every Archive fact from that
/// one value: the packer (`GameArchivePacker::new`) and the Archive
/// extension [`profile_capabilities`] declares, so the two cannot disagree.
pub fn archive_rules(settings: &ProfileSettings) -> Settings {
    Settings::for_profile(
        settings.bsa_game,
        settings.max_bsa_uncompressed_size.requested(),
    )
}

/// The Profile Capabilities a profile's settings declare, with the Archive
/// extension its `archive_rules` write. This is the one place profile
/// settings become capabilities.
///
/// `archive_rules` must be [`archive_rules`] of the same `settings`; taking
/// the value rather than rebuilding it lets a caller hand the very same
/// rules to its packer.
pub fn profile_capabilities(
    settings: &ProfileSettings,
    archive_rules: &Settings,
) -> ProfileCapabilities {
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
    ProfileCapabilities::define(archive_rules.extension, &capabilities)
}

/// The profile's Texture settings, as the texture optimizer takes them: its
/// Texture Target (`texturesFormat`, `texturesUnwantedFormats`) and whether
/// interface Textures are compressed (`texturesCompressInterface`).
///
/// Each Texture Target member's stored number is its `DXGI_FORMAT` value, so
/// every member maps to the format of that number.
pub fn texture_profile(settings: &ProfileSettings) -> TextureProfile {
    TextureProfile {
        format: DXGI_FORMAT::from(settings.textures_format.number()),
        unwanted_formats: settings
            .textures_unwanted_formats
            .iter()
            .map(|format| DXGI_FORMAT::from(format.number()))
            .collect(),
        compress_interface: settings.textures_compress_interface,
    }
}

/// The version a profile writes Meshes as: its Mesh Target (`meshesFileVersion`,
/// `meshesUser`, `meshesStream`), which nifly takes through `SetFile`, `SetUser`
/// and `SetStream`.
///
/// Each axis is a closed set whose stored number is nifly's own (the file
/// version is `NiFileVersion`'s value), so every member maps to its number.
pub fn mesh_target(settings: &ProfileSettings) -> NifVersion {
    NifVersion {
        file: settings.meshes_file_version.number(),
        user: settings.meshes_user.number(),
        stream: settings.meshes_stream.number(),
    }
}
