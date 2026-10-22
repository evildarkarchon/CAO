//! The one place a Profile becomes run facts.
//!
//! From a Profile's settings (`profile.ini`), this module derives its Profile
//! Capabilities with its game's Archive extension, the `cao-archive` game and
//! packer its Archives follow, and the backend's Texture profile and Mesh
//! target. Preparing compiles a run's routing and builds its work from these,
//! and the GUI gates its asset tabs from the same Profile Capabilities, so the
//! two never disagree about what a Profile supports.

use cao_archive::{Game, Settings};
use cao_core::routing::{ProfileCapabilities, ProfileCapability};
use cao_profiles::{BsaGame, ProfileSettings};
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
fn archive_extension(game: BsaGame) -> &'static str {
    Settings::get(archive_game(game)).extension
}

/// The `cao-archive` game whose tables a profile's `bsaGame` names.
pub fn archive_game(game: BsaGame) -> Game {
    match game {
        BsaGame::Tes5 => Game::Tes5,
        BsaGame::Sse => Game::Sse,
        BsaGame::Fo4 => Game::Fo4,
    }
}

/// The packer for a profile's game, with its maximum Archive size raised to
/// the profile's `maxBsaUncompressedSize` when that is larger, as C++
/// `archiveSettings` did.
pub fn archive_packer(settings: &ProfileSettings) -> GameArchivePacker {
    GameArchivePacker::new(
        Settings::get(archive_game(settings.bsa_game))
            .with_profile_max_size(settings.max_bsa_uncompressed_size),
    )
}

/// The profile's Texture settings, as the texture optimizer takes them.
pub fn texture_profile(settings: &ProfileSettings) -> TextureProfile {
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
pub fn mesh_target(settings: &ProfileSettings) -> NifVersion {
    NifVersion {
        file: settings.meshes_file_version,
        user: settings.meshes_user,
        stream: settings.meshes_stream,
    }
}
