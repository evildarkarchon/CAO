//! The one place a Profile becomes run facts.
//!
//! From a Profile's settings (`profile.ini`), this module derives its Profile
//! Capabilities with its game's Archive extension, the packer its Archives
//! follow, whether that game keeps Textures in their own
//! Archive and how large its Archives may ever grow, and the backend's
//! `TextureProfile` (its per-run texture settings)
//! and Mesh Target. Preparing compiles a run's routing and builds its work
//! from these, and the GUI gates its asset tabs from the same Profile
//! Capabilities, so the two never disagree about what a Profile supports.

use cao_archive::{Game, Settings};
use cao_core::routing::{ProfileCapabilities, ProfileCapability};
use cao_profiles::{ClosedSet, ProfileSettings};
use directxtex::DXGI_FORMAT;
use nifly_sys::NifVersion;

use crate::archives::GameArchivePacker;
use crate::textures::TextureProfile;

/// The Profile Capabilities a profile's settings declare, with its game's
/// Archive extension. This is the one place profile settings become
/// capabilities.
pub fn profile_capabilities(settings: &ProfileSettings) -> ProfileCapabilities {
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
    ProfileCapabilities::define(archive_extension(settings.bsa_game), &capabilities)
}

/// The Archive extension of a game, from `cao-archive`'s per-game tables.
fn archive_extension(game: Game) -> &'static str {
    Settings::get(game).extension
}

/// Whether `game` keeps Textures in their own Archive: its Textures Archive is
/// written as a different container than its Standard one (FO4's DX10 BA2
/// beside its `GNRL` Main BA2), so DDS files can never be merged into the
/// Standard Archive.
///
/// The packer's `separate_textures` naming rule and the GUI's "Separate
/// textures BSA" rule both read this, so they always agree.
pub fn keeps_textures_separate(game: Game) -> bool {
    textures_need_own_archive(&Settings::get(game))
}

/// [`keeps_textures_separate`] over a game's `cao-archive` tables, for the
/// packer, which is built from them rather than from a [`Game`].
pub(crate) fn textures_need_own_archive(settings: &Settings) -> bool {
    settings.keeps_textures_separate()
}

/// The most source bytes one Archive of `game` may ever hold, whatever its
/// Profile's maximum Archive size asks: 2 GiB for the BSA games, the practical
/// limit Skyrim loads, and `None` for FO4, whose BA2s have no such limit.
///
/// The packer stops there ([`archive_packer`]), and the GUI's Maximum size spin
/// box reads this so it never shows a size the run would not use.
pub fn archive_size_cap(game: Game) -> Option<u64> {
    Settings::get(game).size_cap()
}

/// The packer for a profile's game, grouping files by the Profile's Archive
/// Limit: its `maxBsaUncompressedSize`, or the game's default when it gives
/// none, never past the game's [`archive_size_cap`] (see
/// [`Settings::for_profile`]).
pub fn archive_packer(settings: &ProfileSettings) -> GameArchivePacker {
    GameArchivePacker::new(Settings::for_profile(
        settings.bsa_game,
        settings.max_bsa_uncompressed_size.requested(),
    ))
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
