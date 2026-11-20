//! Behaviour of the safe `Nif` type beyond nifly's own fixture cases: load and
//! save failures, non-ASCII paths, the `OptimizeFor` report and texture
//! references.

mod common;

use common::{assert_same_bytes, fixture, scratch_dir};
use nifly_sys::{LoadOptions, Nif, NifError, NifVersion, OptimizeOptions, OptimizeReport};

const STATIC_SE: &str = "TestNifFile_Static_SE.nif";
const STATIC_SE_EXPECTED: &str = "TestNifFile_Static_SE_expected.nif";

fn loaded(file_name: &str) -> Nif {
    let mut nif = Nif::new();
    nif.load(&fixture(file_name), LoadOptions::default())
        .unwrap();
    nif
}

/// The Run Worker moves a `Nif` between threads; it must stay `Send`.
#[test]
fn nif_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<Nif>();
}

#[test]
fn a_new_nif_is_not_valid_until_loaded() {
    let mut nif = Nif::new();
    assert!(!nif.is_valid());
    nif.load(&fixture(STATIC_SE), LoadOptions::default())
        .unwrap();
    assert!(nif.is_valid());
}

/// A missing file is a read failure, never nifly's load code 1, which would
/// say nothing about whether the file is corrupt.
#[test]
fn loading_a_missing_file_is_a_read_error() {
    let dir = scratch_dir("load_missing");
    let mut nif = Nif::new();

    let error = nif
        .load(&dir.join("absent.nif"), LoadOptions::default())
        .unwrap_err();

    assert!(matches!(error, NifError::Read(_)), "{error:?}");
    assert!(!nif.is_valid());
}

/// A Mesh another process holds open without sharing it cannot be read. That
/// is a read failure: the Mesh itself may be fine.
#[test]
fn loading_a_file_held_open_without_sharing_is_a_read_error() {
    use std::os::windows::fs::OpenOptionsExt;

    let path = scratch_dir("load_locked").join("locked.nif");
    std::fs::copy(fixture(STATIC_SE), &path).unwrap();
    let _lock = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&path)
        .unwrap();
    let mut nif = Nif::new();

    let error = nif.load(&path, LoadOptions::default()).unwrap_err();

    assert!(matches!(error, NifError::Read(_)), "{error:?}");
    assert!(!nif.is_valid());
}

/// An empty handle has nothing for nifly to read or write, and nifly would
/// index through a null pointer if asked, so every such call is refused.
#[test]
fn an_empty_handle_refuses_every_call_on_the_mesh() {
    let output = scratch_dir("empty_handle").join("out.nif");
    let mut nif = Nif::new();

    assert_eq!(nif.is_sse_compatible(), Err(NifError::NotLoaded));
    assert_eq!(nif.version(), Err(NifError::NotLoaded));
    assert_eq!(nif.texture_paths().err(), Some(NifError::NotLoaded));
    let options = OptimizeOptions {
        target: NifVersion::SSE,
        head_parts: false,
        remove_parallax: false,
    };
    assert_eq!(nif.optimize_for(&options).err(), Some(NifError::NotLoaded));
    assert_eq!(nif.save(&output), Err(NifError::NotLoaded));
    assert!(!output.exists(), "a refused save writes nothing");
}

#[test]
fn loading_a_file_that_is_not_a_nif_fails_with_nifly_code_1() {
    let path = scratch_dir("load_not_a_nif").join("text.nif");
    std::fs::write(&path, b"This is not a NIF file.\n").unwrap();
    let mut nif = Nif::new();

    let error = nif.load(&path, LoadOptions::default()).unwrap_err();

    assert!(matches!(error, NifError::Load { code: 1 }), "{error:?}");
    assert!(!nif.is_valid());
}

#[test]
fn a_failed_load_clears_an_earlier_mesh_and_a_later_load_still_works() {
    let dir = scratch_dir("load_after_failure");
    let mut nif = loaded(STATIC_SE);

    nif.load(&dir.join("absent.nif"), LoadOptions::default())
        .unwrap_err();
    assert!(!nif.is_valid());

    nif.load(&fixture(STATIC_SE), LoadOptions::default())
        .unwrap();
    let output = dir.join("out.nif");
    nif.save(&output).unwrap();
    assert_same_bytes(&output, &fixture(STATIC_SE_EXPECTED));
}

/// Paths cross the shim as raw UTF-16, so characters outside the active code
/// page, including a surrogate pair, must reach nifly intact.
#[test]
fn non_ascii_paths_load_and_save() {
    let dir = scratch_dir("non_ascii").join("ñ_日本_メッシュ_😀");
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("入力_ñ.nif");
    std::fs::copy(fixture(STATIC_SE), &input).unwrap();
    let output = dir.join("出力_😀.nif");

    let mut nif = Nif::new();
    nif.load(&input, LoadOptions::default()).unwrap();
    nif.save(&output).unwrap();

    assert_same_bytes(&output, &fixture(STATIC_SE_EXPECTED));
}

#[test]
fn saving_into_a_missing_directory_fails() {
    let dir = scratch_dir("save_missing_dir");
    let mut nif = loaded(STATIC_SE);

    let error = nif.save(&dir.join("absent").join("out.nif")).unwrap_err();

    assert!(matches!(error, NifError::Save), "{error:?}");
}

#[test]
fn errors_describe_what_failed() {
    assert_eq!(
        NifError::Load { code: 2 }.to_string(),
        "nifly cannot load this file version (load code 2)"
    );
    assert_eq!(
        NifError::Save.to_string(),
        "nifly could not open the output file"
    );
    assert_eq!(
        NifError::Write.to_string(),
        "nifly could not write the whole output file"
    );
    assert_eq!(
        NifError::Exception("bad allocation".into()).to_string(),
        "nifly raised a C++ exception: bad allocation"
    );
}

#[test]
fn nif_versions_match_nifly() {
    // BasicTypes.hpp: getSK(), getSSE() and getFO4(); IsSK() is file 20.2.0.7
    // and stream 83.
    assert_eq!(
        NifVersion::FO4,
        NifVersion {
            file: 0x1402_0007,
            user: 12,
            stream: 130
        }
    );
    assert_eq!(
        NifVersion::SK,
        NifVersion {
            file: 0x1402_0007,
            user: 12,
            stream: 83
        }
    );
    assert_eq!(
        NifVersion::SSE,
        NifVersion {
            file: 0x1402_0007,
            user: 12,
            stream: 100
        }
    );
    assert!(NifVersion::SK.is_sk());
    assert!(!NifVersion::SSE.is_sk());
    // IsSK ignores the user version.
    assert!(
        NifVersion {
            user: 0,
            ..NifVersion::SK
        }
        .is_sk()
    );
}

/// A loaded Mesh reports its own header version, which is what decides
/// whether `OptimizeFor` converts it (audit BE-6).
#[test]
fn a_loaded_mesh_reports_its_header_version() {
    let sse = loaded(STATIC_SE).version().unwrap();
    assert!(sse.is_sse() && !sse.is_sk(), "{sse:?}");
    let le = loaded("TestNifFile_Animated_LE.nif").version().unwrap();
    assert!(le.is_sk() && !le.is_sse(), "{le:?}");
}

/// `converts_to` is nifly's own rule: only LE to SSE and SSE to LE convert.
#[test]
fn only_le_and_sse_convert_to_each_other() {
    let sk = NifVersion::SK;
    let sse = NifVersion::SSE;
    let fo4 = NifVersion::FO4;
    assert!(sk.converts_to(&sse));
    assert!(sse.converts_to(&sk));
    for (from, to) in [(sk, sk), (sse, sse), (fo4, sse), (sse, fo4), (fo4, fo4)] {
        assert!(!from.converts_to(&to), "{from:?} to {to:?}");
    }
    // The predicate agrees with the report of the real call.
    let mut nif = loaded(STATIC_SE);
    let report = nif
        .optimize_for(&OptimizeOptions {
            target: NifVersion::SSE,
            head_parts: false,
            remove_parallax: false,
        })
        .unwrap();
    assert!(report.version_mismatch);
}

/// Every raw nifly fixture, one per game and Mesh shape nifly tests: the
/// `_expected` files are nifly's own saves of these.
const RAW_FIXTURES: &[&str] = &[
    "TestNifFile_Animated_LE.nif",
    "TestNifFile_DeepGraph_SE.nif",
    "TestNifFile_FO76.nif",
    "TestNifFile_Furniture_Col_SE.nif",
    "TestNifFile_LooseBlocks_SE.nif",
    "TestNifFile_MultiBound_SE.nif",
    "TestNifFile_Optimize_Dynamic_LE_to_SE.nif",
    "TestNifFile_Optimize_Dynamic_SE_to_LE.nif",
    "TestNifFile_Optimize_LE_to_SE.nif",
    "TestNifFile_Optimize_SE_to_LE.nif",
    "TestNifFile_OrderedNode_SE.nif",
    "TestNifFile_RootNonZero.nif",
    "TestNifFile_Skinned_Dynamic_SE.nif",
    "TestNifFile_Skinned_FO4.nif",
    "TestNifFile_Skinned_OB.nif",
    "TestNifFile_Skinned_SE.nif",
    "TestNifFile_Static_FO4.nif",
    "TestNifFile_Static_FO4_132.nif",
    "TestNifFile_Static_FO4_139.nif",
    "TestNifFile_Static_SE.nif",
];

/// Loads `source` and saves it to `output`.
fn resave(source: &std::path::Path, output: &std::path::Path) {
    let mut nif = Nif::new();
    nif.load(source, LoadOptions::default()).unwrap();
    nif.save(output).unwrap();
}

/// Fixtures whose header string table nifly reorders once more on the
/// second save, so only the third save repeats the second.
const SETTLING_ON_THE_SECOND_SAVE: &[&str] =
    &["TestNifFile_Animated_LE.nif", "TestNifFile_FO76.nif"];

/// Saving a Mesh through nifly reaches a fixed point: every fixture's third
/// save repeats its second, and all but [`SETTLING_ON_THE_SECOND_SAVE`]
/// already repeat their first.
///
/// The first save may differ from the source, as nifly's save updates
/// bounds, drops unreferenced blocks and sorts them. The string table it
/// writes follows the order of the file it loaded, so a file whose blocks the
/// first save reordered can see its strings reordered by the second. Core
/// drops a staged output identical to the file it would replace, so a Mesh
/// re-saved on every run, at the medium level or with Resave on, is a change
/// on the first run, at most once more on the second, and never after
/// (audit BE-6).
#[test]
fn saving_a_mesh_again_reaches_the_same_bytes() {
    let dir = scratch_dir("resave_fixed_point");
    for name in RAW_FIXTURES {
        let saves: Vec<_> = (1..=3)
            .map(|count| dir.join(format!("save{count}-{name}")))
            .collect();
        resave(&fixture(name), &saves[0]);
        resave(&saves[0], &saves[1]);
        resave(&saves[1], &saves[2]);
        assert_same_bytes(&saves[2], &saves[1]);
        let settles_at_once =
            std::fs::read(&saves[1]).unwrap() == std::fs::read(&saves[0]).unwrap();
        assert_eq!(
            settles_at_once,
            !SETTLING_ON_THE_SECOND_SAVE.contains(name),
            "{name}: whether the second save repeats the first"
        );
    }
}

/// The runs that follow a conversion: an LE Mesh converted to SSE and saved,
/// then loaded and saved again as the medium level would on each later run,
/// reaches a fixed point by the second re-save, as an unconverted Mesh does.
#[test]
fn a_converted_mesh_saved_again_reaches_the_same_bytes() {
    let dir = scratch_dir("resave_converted");
    for name in [
        "TestNifFile_Optimize_LE_to_SE.nif",
        "TestNifFile_Optimize_Dynamic_LE_to_SE.nif",
        "TestNifFile_Animated_LE.nif",
    ] {
        let converted = dir.join(format!("converted-{name}"));
        let mut nif = loaded(name);
        let report = nif
            .optimize_for(&OptimizeOptions {
                target: NifVersion::SSE,
                head_parts: false,
                remove_parallax: false,
            })
            .unwrap();
        assert!(!report.version_mismatch, "{name}");
        nif.save(&converted).unwrap();
        let again = dir.join(format!("again-{name}"));
        let settled = dir.join(format!("settled-{name}"));
        resave(&converted, &again);
        resave(&again, &settled);
        assert_same_bytes(&settled, &again);
    }
}

#[test]
fn sse_meshes_are_sse_compatible() {
    let mut nif = loaded(STATIC_SE);
    assert!(nif.is_sse_compatible().unwrap());
}

/// The OB fixture's skin partitions use triangle strips, which SSE lacks.
#[test]
fn stripped_meshes_are_not_sse_compatible() {
    let mut nif = loaded("TestNifFile_Skinned_OB.nif");
    assert!(!nif.is_sse_compatible().unwrap());
}

fn optimize_animated_le_to_sse(remove_parallax: bool) -> OptimizeReport {
    let mut nif = loaded("TestNifFile_Animated_LE.nif");
    nif.optimize_for(&OptimizeOptions {
        target: NifVersion::SSE,
        head_parts: false,
        remove_parallax,
    })
    .unwrap()
}

/// Of nifly's fixtures, the animated LE one is the conversion that touches
/// named shapes: SSE drops their all-white vertex colours.
#[test]
fn optimize_for_reports_what_it_changed() {
    assert_eq!(
        optimize_animated_le_to_sse(true),
        OptimizeReport {
            shapes_vcolors_removed: vec!["Low02:0".to_owned(), "Low02:1".to_owned()],
            ..OptimizeReport::default()
        }
    );
}

/// nifly only removes vertex colours when `removeParallax` is set
/// (`NifFile.cpp`, `OptimizeFor`), so CAO, which always passes `false`, keeps
/// them. This also shows the option reaches nifly.
#[test]
fn keeping_parallax_keeps_vertex_colours() {
    assert_eq!(
        optimize_animated_le_to_sse(false),
        OptimizeReport::default()
    );
}

/// `OptimizeFor` only converts between LE and SSE. Any other target, such as
/// FO4, sets `versionMismatch` and leaves the Mesh alone; CAO ports that as-is.
#[test]
fn optimize_for_an_unsupported_target_is_a_reported_no_op() {
    let dir = scratch_dir("optimize_mismatch");
    let mut nif = loaded(STATIC_SE);

    let report = nif
        .optimize_for(&OptimizeOptions {
            target: NifVersion {
                file: 0x1402_0007,
                user: 12,
                stream: 130,
            },
            head_parts: false,
            remove_parallax: false,
        })
        .unwrap();

    assert!(report.version_mismatch);
    assert!(!report.dupes_renamed);
    let output = dir.join("out.nif");
    nif.save(&output).unwrap();
    assert_same_bytes(&output, &fixture(STATIC_SE_EXPECTED));
}

#[test]
fn texture_paths_are_readable_as_bytes() {
    let mut nif = loaded(STATIC_SE);
    let textures = nif.texture_paths().unwrap();

    assert!(!textures.is_empty());
    let paths: Vec<&[u8]> = (0..textures.len())
        .map(|i| textures.get(i).unwrap())
        .collect();
    assert!(
        paths
            .iter()
            .any(|path| path.to_ascii_lowercase().ends_with(b".dds")),
        "{:?}",
        paths
            .iter()
            .map(|path| String::from_utf8_lossy(path))
            .collect::<Vec<_>>()
    );
    assert_eq!(textures.get(textures.len()), None);
}

#[test]
fn texture_paths_are_rewritten_in_place_and_saved() {
    let dir = scratch_dir("texture_rewrite");
    let renamed: &[u8] = b"textures\\cao\\r\xe9named.dds"; // Latin-1 é: paths are bytes.
    let mut nif = loaded(STATIC_SE);

    let mut textures = nif.texture_paths().unwrap();
    let index = (0..textures.len())
        .find(|&i| !textures.get(i).unwrap().is_empty())
        .expect("a non-empty texture path");
    textures.set(index, renamed).unwrap();
    assert_eq!(textures.get(index), Some(renamed));

    let output = dir.join("out.nif");
    nif.save(&output).unwrap();
    let mut reloaded = Nif::new();
    reloaded.load(&output, LoadOptions::default()).unwrap();
    let textures = reloaded.texture_paths().unwrap();
    assert!((0..textures.len()).any(|i| textures.get(i) == Some(renamed)));
}
