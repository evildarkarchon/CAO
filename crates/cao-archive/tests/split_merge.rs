//! Splitting a Mod Root's sources into Archives (strict `>`) and merging the open
//! partitions (strict `<`), as C++ CAO's `planFinalization` and bethutil's
//! `ArchiveData` and `merge` do (#461).

use std::path::Path;

use cao_archive::{
    ArchiveData, ArchiveError, ArchiveType, ArchiveVersion, Game, MergeSettings, PackSource,
    Settings, SplitArchives,
};

const ROOT: &str = r"C:\Mods\Example";

/// `game`'s rules with a 100-byte limit, so boundaries are easy to reach.
fn small(game: Game) -> Settings {
    Settings {
        max_size: 100,
        ..Settings::get(game)
    }
}

fn source(relative: &str, size: u64) -> PackSource {
    PackSource {
        path: Path::new(ROOT).join(relative),
        size,
    }
}

fn split(settings: &Settings, sources: Vec<PackSource>) -> SplitArchives {
    SplitArchives::split(Path::new(ROOT), sources, settings).unwrap()
}

/// Each Archive as its type and its files relative to [`ROOT`].
fn layout(archives: &[ArchiveData]) -> Vec<(ArchiveType, Vec<String>)> {
    archives
        .iter()
        .map(|archive| {
            let files = archive.files().iter().map(|file| relative(file)).collect();
            (archive.archive_type(), files)
        })
        .collect()
}

fn relative(path: &Path) -> String {
    path.strip_prefix(ROOT)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned()
}

fn files(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

const NO_MERGE: MergeSettings = MergeSettings {
    textures: false,
    incompressible: false,
};

#[test]
fn a_partition_fills_up_to_exactly_the_limit() {
    let archives = split(
        &small(Game::Sse),
        vec![source(r"meshes\a.nif", 60), source(r"meshes\b.nif", 40)],
    )
    .merge(NO_MERGE);
    assert_eq!(
        layout(&archives),
        [(
            ArchiveType::Standard,
            files(&[r"meshes\a.nif", r"meshes\b.nif"])
        )]
    );
    assert_eq!(archives[0].size(), 100);
}

#[test]
fn one_byte_over_the_limit_starts_a_new_partition() {
    let archives = split(
        &small(Game::Sse),
        vec![
            source(r"meshes\a.nif", 60),
            source(r"meshes\b.nif", 41),
            source(r"meshes\c.nif", 40),
        ],
    )
    .merge(NO_MERGE);
    // The full partition comes first; the open one follows.
    assert_eq!(
        layout(&archives),
        [
            (ArchiveType::Standard, files(&[r"meshes\a.nif"])),
            (
                ArchiveType::Standard,
                files(&[r"meshes\b.nif", r"meshes\c.nif"])
            ),
        ]
    );
}

#[test]
fn an_asset_of_exactly_the_limit_fits_an_empty_partition() {
    let archives = split(&small(Game::Sse), vec![source(r"meshes\a.nif", 100)]).merge(NO_MERGE);
    assert_eq!(archives.len(), 1);
}

#[test]
fn an_asset_larger_than_the_limit_is_an_error() {
    let error = SplitArchives::split(
        Path::new(ROOT),
        vec![source(r"meshes\a.nif", 101)],
        &small(Game::Sse),
    )
    .unwrap_err();
    let ArchiveError::AssetTooLarge {
        path,
        size,
        max_size,
    } = error
    else {
        panic!("expected AssetTooLarge, got {error:?}");
    };
    assert_eq!(path, Path::new(ROOT).join(r"meshes\a.nif"));
    assert_eq!((size, max_size), (101, 100));
}

#[test]
fn sources_are_sorted_before_splitting() {
    let archives = split(
        &small(Game::Sse),
        vec![
            source(r"meshes\b.nif", 60),
            source(r"meshes\a.nif", 50),
            source(r"meshes\c.nif", 50),
        ],
    )
    .merge(NO_MERGE);
    // In the given order `b` would close and `a` and `c` would share one.
    assert_eq!(
        layout(&archives),
        [
            (ArchiveType::Standard, files(&[r"meshes\a.nif"])),
            (ArchiveType::Standard, files(&[r"meshes\b.nif"])),
            (ArchiveType::Standard, files(&[r"meshes\c.nif"])),
        ]
    );
}

#[test]
fn sorting_is_by_component_then_by_utf16_code_unit() {
    let archives = split(
        &Settings::get(Game::Sse),
        vec![
            source("meshes\\a-b.nif", 1),
            source("meshes\\a\\b.nif", 1),
            source("meshes\\\u{FF5E}.nif", 1),
            source("meshes\\\u{1F600}.nif", 1),
        ],
    )
    .merge(NO_MERGE);
    // `a` is a prefix of `a-b`, so the directory sorts first even though `-` is
    // below `\`. U+1F600 is a surrogate pair (0xD83D...) and sorts below U+FF5E
    // in UTF-16, as MSVC's `path` compares, though not in code point order.
    assert_eq!(
        layout(&archives),
        [(
            ArchiveType::Standard,
            files(&[
                "meshes\\a\\b.nif",
                "meshes\\a-b.nif",
                "meshes\\\u{1F600}.nif",
                "meshes\\\u{FF5E}.nif",
            ])
        )]
    );
}

#[test]
fn each_type_fills_its_own_partition_and_loose_files_are_skipped() {
    let sets = small(Game::Sse);
    let archives = split(
        &sets,
        vec![
            source(r"meshes\a.nif", 10),
            source(r"sound\a.wav", 10),
            source(r"textures\a.dds", 10),
            source(r"meshes\notes.md", 10),
            source("Example.esp", 10),
        ],
    )
    .merge(NO_MERGE);
    assert_eq!(
        layout(&archives),
        [
            (ArchiveType::Standard, files(&[r"meshes\a.nif"])),
            (ArchiveType::Incompressible, files(&[r"sound\a.wav"])),
            (ArchiveType::Textures, files(&[r"textures\a.dds"])),
        ]
    );
}

#[test]
fn partitions_take_the_games_versions() {
    let sources = || {
        vec![
            source(r"meshes\a.nif", 1),
            source(r"sound\a.wav", 1),
            source(r"textures\a.dds", 1),
        ]
    };
    let versions = |game| -> Vec<_> {
        split(&Settings::get(game), sources())
            .merge(NO_MERGE)
            .iter()
            .map(ArchiveData::version)
            .collect()
    };
    use ArchiveVersion::*;
    assert_eq!(versions(Game::Tes5), [Tes5, Tes5, Tes5]);
    assert_eq!(versions(Game::Sse), [Sse, Sse, Sse]);
    assert_eq!(versions(Game::Fo4), [Fo4, Fo4, Fo4Dx]);
    assert_eq!(
        split(&Settings::get(Game::Fo4), sources()).merge(NO_MERGE)[0].max_size(),
        4_194_304_000
    );
}

#[test]
fn incompressible_merges_only_when_the_total_is_strictly_under_the_limit() {
    let merge = MergeSettings {
        textures: false,
        incompressible: true,
    };
    let at_limit = split(
        &small(Game::Sse),
        vec![source(r"meshes\a.nif", 60), source(r"sound\a.wav", 40)],
    )
    .merge(merge);
    assert_eq!(
        layout(&at_limit),
        [
            (ArchiveType::Standard, files(&[r"meshes\a.nif"])),
            (ArchiveType::Incompressible, files(&[r"sound\a.wav"])),
        ]
    );

    let under = split(
        &small(Game::Sse),
        vec![source(r"meshes\a.nif", 60), source(r"sound\a.wav", 39)],
    )
    .merge(merge);
    // The merged Archive is Incompressible, so it is written uncompressed.
    assert_eq!(
        layout(&under),
        [(
            ArchiveType::Incompressible,
            files(&[r"meshes\a.nif", r"sound\a.wav"])
        )]
    );
    assert_eq!(under[0].version(), ArchiveVersion::Sse);
    assert_eq!(under[0].size(), 99);
}

#[test]
fn merging_an_empty_incompressible_partition_still_makes_standard_incompressible() {
    // bethutil merges the empty partition too, and `+=` takes its type.
    let archives =
        split(&small(Game::Sse), vec![source(r"meshes\a.nif", 60)]).merge(MergeSettings {
            textures: false,
            incompressible: true,
        });
    assert_eq!(
        layout(&archives),
        [(ArchiveType::Incompressible, files(&[r"meshes\a.nif"]))]
    );
}

#[test]
fn textures_merge_only_when_the_total_is_strictly_under_the_limit() {
    let merge = MergeSettings {
        textures: true,
        incompressible: false,
    };
    let at_limit = split(
        &small(Game::Fo4),
        vec![source(r"meshes\a.nif", 50), source(r"textures\a.dds", 50)],
    )
    .merge(merge);
    assert_eq!(at_limit.len(), 2);

    let under = split(
        &small(Game::Fo4),
        vec![source(r"meshes\a.nif", 50), source(r"textures\a.dds", 49)],
    )
    .merge(merge);
    // A merged Archive keeps Standard's type and version: FO4 puts the DDS into
    // the GNRL Main BA2. Refusing this is #498's job (deviation 21, core side).
    assert_eq!(
        layout(&under),
        [(
            ArchiveType::Standard,
            files(&[r"meshes\a.nif", r"textures\a.dds"])
        )]
    );
    assert_eq!(under[0].version(), ArchiveVersion::Fo4);
}

#[test]
fn merging_both_keeps_the_incompressible_type() {
    let archives = split(
        &small(Game::Sse),
        vec![
            source(r"meshes\a.nif", 10),
            source(r"sound\a.wav", 10),
            source(r"textures\a.dds", 10),
        ],
    )
    .merge(MergeSettings {
        textures: true,
        incompressible: true,
    });
    assert_eq!(
        layout(&archives),
        [(
            ArchiveType::Incompressible,
            files(&[r"meshes\a.nif", r"sound\a.wav", r"textures\a.dds"])
        )]
    );
}

#[test]
fn merging_only_touches_the_open_partitions() {
    let archives = split(
        &small(Game::Sse),
        vec![
            source(r"meshes\a.nif", 90),
            source(r"meshes\b.nif", 20),
            source(r"sound\a.wav", 20),
        ],
    )
    .merge(MergeSettings {
        textures: false,
        incompressible: true,
    });
    assert_eq!(
        layout(&archives),
        [
            (ArchiveType::Standard, files(&[r"meshes\a.nif"])),
            (
                ArchiveType::Incompressible,
                files(&[r"meshes\b.nif", r"sound\a.wav"])
            ),
        ]
    );
}

#[test]
fn nothing_to_pack_gives_no_archives() {
    assert!(
        split(&small(Game::Sse), Vec::new())
            .merge(NO_MERGE)
            .is_empty()
    );
    // A zero-byte file still makes an Archive: emptiness counts files, not bytes.
    let archives = split(&small(Game::Sse), vec![source(r"meshes\a.nif", 0)]).merge(NO_MERGE);
    assert_eq!(archives.len(), 1);
}
