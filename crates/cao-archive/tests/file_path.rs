//! Plugin and Archive name parsing (`btu::bsa::FilePath`), including the cases of
//! bethutil's own `tests/bsa/plugin.cpp` and deviation 9.

mod common;

use std::path::Path;

use cao_archive::{FilePath, FileType, Game, Settings, list_archives, list_plugins};

fn plugin(game: Game, path: &str) -> Option<FilePath> {
    FilePath::make(Path::new(path), &Settings::get(game), FileType::Plugin)
}

fn archive(game: Game, path: &str) -> Option<FilePath> {
    FilePath::make(Path::new(path), &Settings::get(game), FileType::Archive)
}

#[test]
fn a_simple_plugin_name_has_no_suffix_or_counter() {
    let plug = plugin(Game::Sse, "C:/SomeDir/Requiem.esp").unwrap();
    assert_eq!(plug.dir, Path::new("C:/SomeDir"));
    assert_eq!(plug.name, "Requiem");
    assert_eq!(plug.suffix, "");
    assert_eq!(plug.ext, ".esp");
    assert_eq!(plug.counter, None);
    assert_eq!(plug.kind, FileType::Plugin);
    assert!(plugin(Game::Sse, "").is_none());
}

#[test]
fn digits_may_come_before_or_after_the_suffix() {
    let after = archive(Game::Sse, "C:/SomeDir/Requiem - Textures01.bsa").unwrap();
    let before = archive(Game::Sse, "C:/SomeDir/Requiem01 - Textures.bsa").unwrap();
    assert_eq!(after.name, "Requiem");
    assert_eq!(after.suffix, "Textures");
    assert_eq!(after.counter, Some(1));
    assert_eq!(after.ext, ".bsa");
    assert_eq!(before.name, "Requiem");
    assert_eq!(before.counter, Some(1));
}

#[test]
fn only_trailing_digits_and_the_last_known_suffix_are_parsed() {
    let plug = archive(
        Game::Sse,
        "C:/AnotherSomeDir/Requiem01 - Enhancement - Textures.bsa",
    )
    .unwrap();
    assert_eq!(plug.name, "Requiem01 - Enhancement");
    assert_eq!(plug.suffix, "Textures");
    assert_eq!(plug.counter, None);

    let plug = archive(Game::Sse, "C:/AnotherSomeDir/Requiem - Enhancement01.bsa").unwrap();
    assert_eq!(plug.name, "Requiem - Enhancement");
    assert_eq!(plug.suffix, "");
    assert_eq!(plug.counter, Some(1));
}

#[test]
fn an_unknown_suffix_stays_part_of_the_name() {
    let plug = plugin(Game::Fo4, "C:/Mk. II - Frag.esp").unwrap();
    assert_eq!(plug.name, "Mk. II - Frag");
    assert_eq!(plug.suffix, "");
    assert_eq!(plug.counter, None);
    // Suffixes match case-sensitively.
    let plug = archive(Game::Sse, "C:/Foo - textures.bsa").unwrap();
    assert_eq!(plug.name, "Foo - textures");
    assert_eq!(plug.suffix, "");
    // TES5 has no suffixes at all.
    let plug = archive(Game::Tes5, "C:/Foo - Textures.bsa").unwrap();
    assert_eq!(plug.name, "Foo - Textures");
    assert_eq!(plug.suffix, "");
}

#[test]
fn a_counter_takes_every_trailing_digit() {
    let plug = plugin(Game::Fo4, "some_dir/Colt 6520.esp").unwrap();
    assert_eq!(plug.name, "Colt ");
    assert_eq!(plug.counter, Some(6520));
    let plug = plugin(Game::Fo4, "some_dir/Colt. 6520.esp").unwrap();
    assert_eq!(plug.name, "Colt. ");
    assert_eq!(plug.counter, Some(6520));
}

#[test]
fn a_counter_too_large_for_u32_stays_part_of_the_name() {
    // C++ `stoul` throws out_of_range and the digits are left alone.
    let plug = plugin(Game::Sse, "Mod99999999999.esp").unwrap();
    assert_eq!(plug.name, "Mod99999999999");
    assert_eq!(plug.counter, None);
}

#[test]
fn extensions_match_case_sensitively() {
    assert!(plugin(Game::Sse, "C:/Mod.ESP").is_none());
    assert!(archive(Game::Sse, "C:/Mod.BSA").is_none());
    assert!(plugin(Game::Tes5, "C:/Mod.esl").is_none());
    assert!(archive(Game::Sse, "C:/Mod.ba2").is_none());
    assert!(archive(Game::Fo4, "C:/Mod - Main.ba2").is_some());
}

#[test]
fn names_re_render_with_the_counter_before_the_suffix() {
    let plug = archive(Game::Sse, "C:/Mods/Foo - Textures2.bsa").unwrap();
    assert_eq!(plug.full_name(), "Foo2 - Textures");
    assert_eq!(plug.full_path(), Path::new("C:/Mods/Foo2 - Textures.bsa"));
    // A counter loses its leading zeros.
    let plug = archive(Game::Sse, "C:/Mods/Requiem - Textures01.bsa").unwrap();
    assert_eq!(
        plug.full_path(),
        Path::new("C:/Mods/Requiem1 - Textures.bsa")
    );
    let plug = archive(Game::Fo4, "C:/Mods/Foo - Main.ba2").unwrap();
    assert_eq!(plug.full_path(), Path::new("C:/Mods/Foo - Main.ba2"));
}

/// **Deviation 9:** bethutil's `eat_digits` walks off the front of an all-digit
/// stem, which is undefined behaviour. The port takes the whole stem as the counter.
#[test]
fn deviation_9_an_all_digit_stem_is_all_counter() {
    let plug = plugin(Game::Sse, "C:/Mods/2.esp").unwrap();
    assert_eq!(plug.name, "");
    assert_eq!(plug.counter, Some(2));
    assert_eq!(plug.full_path(), Path::new("C:/Mods/2.esp"));

    let plug = archive(Game::Sse, "C:/Mods/2 - Textures.bsa").unwrap();
    assert_eq!(plug.name, "");
    assert_eq!(plug.suffix, "Textures");
    assert_eq!(plug.counter, Some(2));

    // A stem that is only a suffix leaves an empty name with no digits, the
    // other way bethutil indexed before the string.
    let plug = archive(Game::Sse, "C:/Mods/ - Textures.bsa").unwrap();
    assert_eq!(plug.name, "");
    assert_eq!(plug.suffix, "Textures");
    assert_eq!(plug.counter, None);
}

#[test]
fn names_order_by_directory_name_suffix_extension_then_counter() {
    let mut plugins: Vec<_> = ["C:/B.esp", "C:/A.esp", "C:/A.esm", "C:/A1.esp", "C:/A0.esm"]
        .into_iter()
        .map(|path| plugin(Game::Sse, path).unwrap())
        .collect();
    plugins.sort();
    let sorted: Vec<_> = plugins.iter().map(FilePath::full_path).collect();
    // `A0.esm` and `A1.esp` parse to name `A`; with equal name and suffix the
    // extension decides, then the counter, where no counter sorts first.
    assert_eq!(
        sorted,
        ["C:/A.esm", "C:/A0.esm", "C:/A.esp", "C:/A1.esp", "C:/B.esp"].map(Path::new)
    );
}

#[test]
fn listing_skips_directories_and_names_of_other_kinds() {
    let dir = common::scratch_dir("file_path_listing");
    for name in [
        "Mod.esp",
        "Mod.esm",
        "Other.ESP",
        "Mod - Textures.bsa",
        "Mod.ba2",
        "notes.txt",
    ] {
        common::write(&dir, name, b"");
    }
    std::fs::create_dir(dir.join("Folder.esp")).unwrap();
    let sets = Settings::get(Game::Sse);

    let mut plugins = list_plugins(&dir, &sets).unwrap();
    plugins.sort();
    let plugins: Vec<_> = plugins.iter().map(FilePath::full_path).collect();
    assert_eq!(plugins, [dir.join("Mod.esm"), dir.join("Mod.esp")]);

    let archives = list_archives(&dir, &sets).unwrap();
    assert_eq!(archives.len(), 1);
    assert_eq!(archives[0].full_path(), dir.join("Mod - Textures.bsa"));
    assert_eq!(archives[0].kind, FileType::Archive);
}
