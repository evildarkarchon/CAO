//! The per-game archive tables against bethutil's `Settings::get` at `81f882ed`
//! (`include/btu/bsa/settings.hpp`), and the effective maximum sizes the shipped
//! profiles produce (#461).

use cao_archive::{ArchiveVersion, Game, Settings};

#[test]
fn tes5_uses_bsa_v104_with_no_suffixes() {
    let sets = Settings::get(Game::Tes5);
    assert_eq!(sets.game, Game::Tes5);
    assert_eq!(sets.format, ArchiveVersion::Tes5);
    // TES5 has no texture format, so Textures archives fall back to v104.
    assert_eq!(sets.texture_format, None);
    assert_eq!(sets.suffix, None);
    assert_eq!(sets.texture_suffix, None);
    assert_eq!(sets.extension, ".bsa");
    assert_eq!(sets.plugin_extensions, [".esm", ".esp"]);
    assert_eq!(sets.archive_limit(), 2_097_152_000);
}

#[test]
fn sse_uses_bsa_v105_with_a_textures_suffix() {
    let sets = Settings::get(Game::Sse);
    assert_eq!(sets.game, Game::Sse);
    assert_eq!(sets.format, ArchiveVersion::Sse);
    assert_eq!(sets.texture_format, Some(ArchiveVersion::Sse));
    assert_eq!(sets.suffix, None);
    assert_eq!(sets.texture_suffix, Some("Textures"));
    assert_eq!(sets.extension, ".bsa");
    assert_eq!(sets.plugin_extensions, [".esl", ".esm", ".esp"]);
    assert_eq!(sets.archive_limit(), 2_097_152_000);
}

#[test]
fn fo4_uses_gnrl_and_dx10_ba2s_with_main_and_textures_suffixes() {
    let sets = Settings::get(Game::Fo4);
    assert_eq!(sets.game, Game::Fo4);
    assert_eq!(sets.format, ArchiveVersion::Fo4);
    assert_eq!(sets.texture_format, Some(ArchiveVersion::Fo4Dx));
    assert_eq!(sets.suffix, Some("Main"));
    assert_eq!(sets.texture_suffix, Some("Textures"));
    assert_eq!(sets.extension, ".ba2");
    assert_eq!(sets.plugin_extensions, [".esl", ".esm", ".esp"]);
    assert_eq!(sets.archive_limit(), 4_194_304_000);
}

#[test]
fn a_profile_with_no_archive_limit_gets_its_games_default() {
    for (game, default) in [
        (Game::Tes5, 2_097_152_000),
        (Game::Sse, 2_097_152_000),
        (Game::Fo4, 4_194_304_000),
    ] {
        let sets = Settings::for_profile(game, None);
        assert_eq!(sets.archive_limit(), default, "{game:?}");
    }
}

#[test]
fn a_profile_may_ask_for_archives_smaller_than_its_games_default() {
    // 1 GiB is under SSE's 2000 MiB table maximum, which is no longer a floor.
    let sets = Settings::for_profile(Game::Sse, Some(1_073_741_824));
    assert_eq!(sets.archive_limit(), 1_073_741_824);
    let sets = Settings::for_profile(Game::Fo4, Some(100));
    assert_eq!(sets.archive_limit(), 100);
}

#[test]
fn a_skyrim_profiles_archive_limit_stops_at_the_2_gib_size_cap() {
    // 2 GiB is the practical limit of a Skyrim BSA, so no Profile, copied or
    // not, packs one past it.
    for game in [Game::Tes5, Game::Sse] {
        assert_eq!(
            Settings::for_profile(game, None).size_cap(),
            Some(2_147_483_648),
            "{game:?}"
        );
        for (limit, expected) in [
            (2_147_483_647, 2_147_483_647),
            (2_147_483_648, 2_147_483_648),
            (2_147_483_649, 2_147_483_648),
            (8_589_934_592, 2_147_483_648),
            (u64::MAX, 2_147_483_648),
        ] {
            let sets = Settings::for_profile(game, Some(limit));
            assert_eq!(sets.archive_limit(), expected, "{limit} under {game:?}");
        }
    }
}

#[test]
fn a_fallout_4_profiles_archive_limit_is_never_capped() {
    // FO4's 4000 MiB is only the Creation Kit's limit; the BA2 itself and
    // the game take far larger Archives.
    assert_eq!(Settings::for_profile(Game::Fo4, None).size_cap(), None);
    for limit in [8_589_934_592, u64::MAX] {
        let sets = Settings::for_profile(Game::Fo4, Some(limit));
        assert_eq!(sets.archive_limit(), limit);
    }
}

#[test]
fn only_fallout_4_keeps_textures_in_their_own_archive() {
    // FO4 writes Textures as a DX10 BA2 beside its `GNRL` Main BA2; the
    // Skyrim games write both as the same BSA.
    assert!(!Settings::for_profile(Game::Tes5, None).keeps_textures_separate());
    assert!(!Settings::for_profile(Game::Sse, None).keeps_textures_separate());
    assert!(Settings::for_profile(Game::Fo4, None).keeps_textures_separate());
}

#[test]
fn shipped_profile_limits_give_the_effective_archive_limits() {
    // `maxBsaUncompressedSize` from `profiles/*/profile.ini`, as whole bytes.
    // The BSA Profiles' 1.96 GiB is above btu's old 2000 MiB floor and under
    // the 2 GiB cap, so they pack exactly as they always did. FO4's 3.9 GiB is
    // under its old 4000 MiB floor, which no longer applies, so it now packs
    // at the 3.9 GiB its Profile and the window show.
    for (game, limit, expected) in [
        (Game::Tes5, 2_104_533_975, 2_104_533_975),
        (Game::Sse, 2_104_533_975, 2_104_533_975),
        (Game::Fo4, 4_187_593_113, 4_187_593_113),
    ] {
        let sets = Settings::for_profile(game, Some(limit));
        assert_eq!(sets.archive_limit(), expected, "{game:?}");
    }
}
