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
    assert_eq!(sets.max_size, 2_097_152_000);
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
    assert_eq!(sets.max_size, 2_097_152_000);
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
    assert_eq!(sets.max_size, 4_194_304_000);
}

#[test]
fn shipped_profile_limits_give_the_effective_maximum_sizes() {
    // `maxBsaUncompressedSize` from `profiles/*/profile.ini`. The BSA profiles'
    // value beats btu's 2000 MiB and is truncated; FO4's is smaller, so btu wins.
    let tes5 = Settings::get(Game::Tes5).with_profile_max_size(2_104_533_975.04);
    let sse = Settings::get(Game::Sse).with_profile_max_size(2_104_533_975.04);
    let fo4 = Settings::get(Game::Fo4).with_profile_max_size(4_187_593_113.6);
    assert_eq!(tes5.max_size, 2_104_533_975);
    assert_eq!(sse.max_size, 2_104_533_975);
    assert_eq!(fo4.max_size, 4_194_304_000);
}

#[test]
fn a_profile_limit_never_lowers_the_table_maximum() {
    let sets = Settings::get(Game::Sse).with_profile_max_size(0.0);
    assert_eq!(sets.max_size, 2_097_152_000);
    let sets = Settings::get(Game::Sse).with_profile_max_size(f64::NAN);
    assert_eq!(sets.max_size, 2_097_152_000);
}
