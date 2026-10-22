//! Whether a game keeps Textures in their own Archive: the `profile` module's
//! query, which the GUI's "Separate textures BSA" rule reads, against the
//! packer's naming rule, which Archive Finalization reads.

use cao_archive::Settings;
use cao_core::run::ArchivePacker as _;
use cao_optimizers::archives::GameArchivePacker;
use cao_optimizers::profile::{archive_game, keeps_textures_separate};
use cao_profiles::BsaGame;

/// Every game a profile can name.
const EVERY_GAME: [BsaGame; 3] = [BsaGame::Tes5, BsaGame::Sse, BsaGame::Fo4];

#[test]
fn only_fallout_4_keeps_textures_in_their_own_archive() {
    // FO4 packs Textures into a DX10 BA2, unlike its `GNRL` Main BA2; both
    // Skyrims pack Textures in the same BSA version as everything else.
    assert!(!keeps_textures_separate(BsaGame::Tes5));
    assert!(!keeps_textures_separate(BsaGame::Sse));
    assert!(keeps_textures_separate(BsaGame::Fo4));
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
