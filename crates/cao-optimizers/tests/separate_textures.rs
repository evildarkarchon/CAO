//! Whether a game keeps Textures in their own Archive: the `profile` module's
//! query, which the GUI's "Separate textures BSA" rule reads, against the
//! packer's naming rule, which Archive Finalization reads. With it, the
//! Archive extension a Profile's capabilities carry, against the extension the
//! packer built from the same Profile writes.

use std::path::Path;

use cao_archive::Settings;
use cao_core::routing::{ProfileCapabilities, ProfileCapability};
use cao_core::run::ArchivePacker as _;
use cao_optimizers::archives::GameArchivePacker;
use cao_optimizers::profile::{
    archive_game, archive_packer, keeps_textures_separate, profile_capabilities,
};
use cao_profiles::{BsaGame, ProfileSettings, Profiles};

/// Every game a profile can name.
const EVERY_GAME: [BsaGame; 3] = [BsaGame::Tes5, BsaGame::Sse, BsaGame::Fo4];

/// Every Profile Capability.
const EVERY_CAPABILITY: [ProfileCapability; 8] = [
    ProfileCapability::NativeTextureOptimization,
    ProfileCapability::ConvertibleTextureConversion,
    ProfileCapability::StandardMeshOptimization,
    ProfileCapability::TerrainMeshOptimization,
    ProfileCapability::AnimationOptimization,
    ProfileCapability::ArchiveExtraction,
    ProfileCapability::MeshReferenceMaintenance,
    ProfileCapability::ArchiveCreation,
];

#[test]
fn only_fallout_4_keeps_textures_in_their_own_archive() {
    // FO4 packs Textures into a DX10 BA2, unlike its `GNRL` Main BA2; both
    // Skyrims pack Textures in the same BSA version as everything else.
    assert!(!keeps_textures_separate(BsaGame::Tes5));
    assert!(!keeps_textures_separate(BsaGame::Sse));
    assert!(keeps_textures_separate(BsaGame::Fo4));
}

#[test]
fn a_profiles_archive_facts_agree_with_the_packer_built_from_it_for_every_game() {
    // The shipped SSE Profile declares every Profile Capability, so only the
    // Archive extension can differ between the two capability sets compared.
    let app_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let shipped = Profiles::new(&app_dir).open("SSE").load_settings().unwrap();
    for game in EVERY_GAME {
        let settings = ProfileSettings {
            bsa_game: game,
            ..shipped.clone()
        };
        let packer = archive_packer(&settings);
        let rules = packer.rules();

        assert_eq!(
            profile_capabilities(&settings),
            ProfileCapabilities::define(rules.extension.clone(), &EVERY_CAPABILITY),
            "{game:?}: Preparing routes Archives by the extension the packer writes"
        );
        assert_eq!(
            keeps_textures_separate(game),
            rules.separate_textures,
            "{game:?}: the GUI's rule matches the packer's"
        );
    }
}

#[test]
fn the_query_agrees_with_the_packers_naming_rule_for_every_game() {
    for game in EVERY_GAME {
        let packer = GameArchivePacker::new(Settings::get(archive_game(game)));
        assert_eq!(
            keeps_textures_separate(game),
            packer.rules().separate_textures,
            "{game:?}"
        );
    }
}
