//! File-type classification by first path component, as bethutil's
//! `get_filetype` does it (#461).

use std::path::Path;

use cao_archive::{FileType, Game, Settings, file_type};

const ROOT: &str = r"C:\Mods\Example";

/// Classifies `relative`, inside [`ROOT`], under `game`'s rules.
fn classify(game: Game, relative: &str) -> FileType {
    file_type(
        &Path::new(ROOT).join(relative),
        Path::new(ROOT),
        &Settings::get(game),
    )
}

#[test]
fn the_first_component_and_the_extension_pick_the_archive_type() {
    for game in [Game::Tes5, Game::Sse, Game::Fo4] {
        assert_eq!(
            classify(game, r"meshes\armor\cuirass.nif"),
            FileType::Standard
        );
        assert_eq!(
            classify(game, r"scripts\source\quest.psc"),
            FileType::Standard
        );
        assert_eq!(
            classify(game, r"textures\armor\cuirass.dds"),
            FileType::Texture
        );
        assert_eq!(
            classify(game, r"interface\icons\map.dds"),
            FileType::Texture
        );
        assert_eq!(
            classify(game, r"sound\fx\hit.wav"),
            FileType::Incompressible
        );
        assert_eq!(classify(game, r"music\theme.xwm"), FileType::Incompressible);
        assert_eq!(
            classify(game, r"meshes\actors\idle.hkt"),
            FileType::Incompressible
        );
        assert_eq!(
            classify(game, r"scripts\quest.pex"),
            FileType::Incompressible
        );
    }
}

#[test]
fn directory_and_extension_are_matched_without_regard_to_case() {
    assert_eq!(
        classify(Game::Sse, r"Meshes\Armor\Cuirass.NIF"),
        FileType::Standard
    );
    assert_eq!(classify(Game::Sse, r"TEXTURES\a.Dds"), FileType::Texture);
}

#[test]
fn an_extension_outside_its_listed_directories_stays_loose() {
    assert_eq!(
        classify(Game::Sse, r"textures\hit.wav"),
        FileType::Blacklist
    );
    assert_eq!(classify(Game::Sse, r"sounds\hit.wav"), FileType::Blacklist);
    assert_eq!(
        classify(Game::Sse, r"meshes\cuirass.nif.caobad"),
        FileType::Blacklist
    );
}

#[test]
fn standard_rules_win_over_texture_rules() {
    // `.tga` is a Standard rule under `textures`, checked before any Texture rule.
    assert_eq!(classify(Game::Sse, r"textures\old.tga"), FileType::Standard);
}

#[test]
fn png_textures_are_textures_except_under_fo4() {
    assert_eq!(classify(Game::Tes5, r"textures\ui.png"), FileType::Texture);
    assert_eq!(classify(Game::Sse, r"textures\ui.png"), FileType::Texture);
    // A DX10 BA2 holds DDS only, so FO4 packs PNGs into the Main BA2.
    assert_eq!(classify(Game::Fo4, r"textures\ui.png"), FileType::Standard);
}

#[test]
fn uvd_visibility_files_pack_only_under_fo4() {
    assert_eq!(classify(Game::Fo4, r"vis\cell.uvd"), FileType::Standard);
    assert_eq!(classify(Game::Sse, r"vis\cell.uvd"), FileType::Blacklist);
}

#[test]
fn the_root_token_matches_only_a_folder_named_root() {
    // A root-level file's first component is its own name, never `root`.
    assert_eq!(classify(Game::Sse, "splash.jpg"), FileType::Blacklist);
    assert_eq!(classify(Game::Sse, r"root\splash.jpg"), FileType::Standard);
}

#[test]
fn plugins_are_recognised_by_the_games_plugin_extensions() {
    assert_eq!(classify(Game::Sse, "Example.esp"), FileType::Plugin);
    assert_eq!(classify(Game::Sse, "Example.ESL"), FileType::Plugin);
    assert_eq!(classify(Game::Fo4, "Example.esl"), FileType::Plugin);
    // TES5 has no light plugins.
    assert_eq!(classify(Game::Tes5, "Example.esl"), FileType::Blacklist);
}

#[test]
fn archives_are_recognised_by_the_games_archive_extension() {
    assert_eq!(classify(Game::Sse, "Example.bsa"), FileType::Archive);
    assert_eq!(classify(Game::Fo4, "Example - Main.BA2"), FileType::Archive);
    assert_eq!(classify(Game::Sse, "Example.ba2"), FileType::Blacklist);
}
