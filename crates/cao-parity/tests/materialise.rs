//! The tree recipe and its materialiser (#490): seeded content written once
//! into `input/`, copied to both sides, then shaped per copy. Everything is
//! observed through the files a case directory ends up holding.

mod common;

use std::path::{Path, PathBuf};

use cao_parity::HarnessError;
use cao_parity::case::{CaseFile, CaseLayout, CaseSpec, Side, SideResources};
use cao_parity::cases::{fixtures_dir, seed, seeds};
use cao_parity::local_assets::{Edition, LocalAssetPool, PinnedList, sha256_hex};
use cao_parity::materialise::{
    Environment, PATH_CAP_UTF16, Readiness, Sources, materialise, write_input, write_input_from,
};
use cao_parity::recipe::{ContentEntry, TextureFormat, TreeRecipe};
use common::{TempDir, shipped_profiles};
use directxtex::{DDS_FLAGS_NONE, DXGI_FORMAT, ScratchImage, TexMetadata};
use nifly_sys::{LoadOptions, Nif, NifVersion, OptimizeOptions};

/// A tree touching every content kind and every texture option.
fn every_content_kind() -> TreeRecipe {
    serde_json::from_value(serde_json::json!({
        "content": [
            {"kind": "texture", "path": "mods/Mod/textures/gradient.dds",
             "format": "R8G8B8A8_UNORM", "width": 16, "height": 8},
            {"kind": "texture", "path": "mods/Mod/textures/noise_bc1.dds",
             "format": "BC1_UNORM", "width": 32, "height": 32, "mip_levels": 0,
             "pattern": "noise"},
            {"kind": "texture", "path": "mods/Mod/textures/edges_bc7.dds",
             "format": "BC7_UNORM", "width": 16, "height": 16, "header": "dx10",
             "pattern": "edges"},
            {"kind": "texture", "path": "mods/Mod/textures/sky_cube.dds",
             "format": "BC3_UNORM", "width": 8, "height": 8, "cubemap": true,
             "pattern": "alpha_ramp"},
            {"kind": "texture", "path": "mods/Mod/textures/layers.dds",
             "format": "R8G8B8A8_UNORM", "width": 12, "height": 6, "array_size": 3,
             "mip_levels": 2, "header": "dx10", "pattern": "mask"},
            {"kind": "texture", "path": "mods/Mod/textures/bump_n.dds",
             "format": "BC5_UNORM", "width": 16, "height": 16, "pattern": "normal"},
            {"kind": "texture", "path": "mods/Mod/textures/raw.dds",
             "format": "R8G8B8A8_TYPELESS", "width": 4, "height": 4, "mip_levels": 0,
             "header": "dx10", "pattern": "bytes"},
            {"kind": "texture", "path": "mods/Mod/textures/old.tga",
             "format": "R8G8B8A8_UNORM", "width": 8, "height": 8, "pattern": "noise"},
            {"kind": "texture", "path": "mods/Mod/textures/cut.dds",
             "format": "R8G8B8A8_UNORM", "width": 8, "height": 8,
             "fault": {"truncate": 100}},
            {"kind": "texture", "path": "mods/Mod/textures/junk.dds",
             "format": "R8G8B8A8_UNORM", "width": 8, "height": 8, "fault": "garbage"},
            {"kind": "texture", "path": "mods/Mod/textures/empty.dds",
             "format": "R8G8B8A8_UNORM", "width": 8, "height": 8, "fault": "zero"},
            {"kind": "text", "path": "mods/Mod/readme.txt", "text": "Hello\r\n",
             "note": "not an Asset"},
            {"kind": "raw", "path": "mods/Mod/meshes/blob.nif", "base64": "AAEC/w=="},
            {"kind": "mesh", "path": "mods/Mod/meshes/bowl.nif", "version": "sse",
             "shapes": [{"name": "Bowl", "textures": ["textures\\old.tga"]}]},
            {"kind": "directory", "path": "mods/Mod/empty"}
        ]
    }))
    .unwrap()
}

fn case(tree: TreeRecipe) -> CaseFile {
    let spec: CaseSpec = seed("tracer-dry-run-textures").unwrap().unwrap().spec;
    CaseFile {
        tree,
        ..CaseFile::new(CaseSpec {
            mod_selection: cao_parity::case::ModSelection::OneMod {
                folder: "mods/Mod".into(),
            },
            ..spec
        })
    }
}

/// Every entry under `root` (directories as `None`), by relative path.
fn snapshot(root: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if path.is_dir() {
                found.push((relative, None));
                pending.push(path);
            } else {
                found.push((relative, Some(std::fs::read(&path).unwrap())));
            }
        }
    }
    found.sort();
    found
}

fn metadata(path: &Path) -> TexMetadata {
    let mut metadata = TexMetadata::default();
    ScratchImage::load_dds(
        &std::fs::read(path).unwrap(),
        DDS_FLAGS_NONE,
        Some(&mut metadata),
        None,
    )
    .unwrap();
    metadata
}

#[test]
fn the_same_case_id_gives_a_byte_identical_input() {
    let temp = TempDir::new("materialise-seed");
    let case = case(every_content_kind());
    let (first, second, other) = (
        temp.path().join("first"),
        temp.path().join("second"),
        temp.path().join("other"),
    );
    write_input(&case, "seeded-case", &first, &fixtures_dir()).unwrap();
    write_input(&case, "seeded-case", &second, &fixtures_dir()).unwrap();
    write_input(&case, "another-case", &other, &fixtures_dir()).unwrap();

    let first = snapshot(&first);
    let second = snapshot(&second);
    assert_eq!(first.len(), second.len());
    let differing: Vec<_> = first
        .iter()
        .zip(&second)
        .filter(|(a, b)| a != b)
        .map(|(a, _)| &a.0)
        .collect();
    assert!(differing.is_empty(), "not byte-identical: {differing:?}");
    // The seed is the case id: another id draws other noise.
    let noise = |files: &[(PathBuf, Option<Vec<u8>>)]| {
        files
            .iter()
            .find(|(path, _)| path.ends_with("noise_bc1.dds"))
            .unwrap()
            .1
            .clone()
    };
    assert_ne!(noise(&first), noise(&snapshot(&other)));
}

#[test]
fn a_raw_entry_takes_exactly_one_source_inside_the_fixtures_folder() {
    let temp = TempDir::new("materialise-raw");
    let fixtures = temp.path().join("fixtures");
    common::write(&fixtures, "archives/patched.bsa", b"BSA\0patched");
    let raw = |entry: serde_json::Value| shaped(serde_json::json!([entry]), serde_json::json!([]));

    let root = temp.path().join("fixture");
    let case = raw(
        serde_json::json!({"kind": "raw", "path": "mods/Mod/Mod.bsa",
                                      "fixture": "archives/patched.bsa"}),
    );
    write_input(&case, "raw", &root, &fixtures).unwrap();
    assert_eq!(
        std::fs::read(root.join("mods/Mod/Mod.bsa")).unwrap(),
        b"BSA\0patched"
    );

    for (index, invalid) in [
        serde_json::json!({"kind": "raw", "path": "mods/Mod/a.bin", "fixture": "../escape.bin"}),
        serde_json::json!({"kind": "raw", "path": "mods/Mod/a.bin"}),
        serde_json::json!({"kind": "raw", "path": "mods/Mod/a.bin", "base64": "AA==",
                           "fixture": "archives/patched.bsa"}),
        serde_json::json!({"kind": "raw", "path": "mods/Mod/a.bin", "base64": "A=A="}),
    ]
    .into_iter()
    .enumerate()
    {
        let root = temp.path().join(format!("invalid-{index}"));
        let error = write_input(&raw(invalid.clone()), "raw", &root, &fixtures).unwrap_err();
        assert!(
            matches!(error, HarnessError::InvalidCase(_)),
            "{invalid}: {error}"
        );
    }
}

/// An `archive` entry packs its own content entries, as game paths, into an
/// Archive of the game's container, the same bytes for the same case id; and
/// it may pack only files with safe, ASCII game paths.
#[test]
fn an_archive_entry_packs_its_files_into_the_games_container() {
    let temp = TempDir::new("materialise-archive");
    let archives = || {
        shaped(
            serde_json::json!([
                {"kind": "archive", "path": "mods/Mod/Mod - Textures.ba2", "game": "fo4",
                 "type": "textures", "content": [
                    {"kind": "texture", "path": "textures/sky.dds", "format": "BC1_UNORM",
                     "width": 16, "height": 16, "mip_levels": 0}]},
                {"kind": "archive", "path": "mods/Mod/Mod.bsa", "game": "sse",
                 "type": "standard", "compress": false, "content": [
                    {"kind": "text", "path": "scripts/quest.pex", "text": "compiled"},
                    {"kind": "raw", "path": "meshes/blob.nif", "base64": "AAEC/w=="}]}
            ]),
            serde_json::json!([]),
        )
    };
    let (first, second) = (temp.path().join("first"), temp.path().join("second"));
    write_input(&archives(), "archives", &first, &fixtures_dir()).unwrap();
    write_input(&archives(), "archives", &second, &fixtures_dir()).unwrap();
    assert_eq!(snapshot(&first), snapshot(&second));

    let textures = cao_archive::ReadArchive::open(&first.join("mods/Mod/Mod - Textures.ba2"))
        .unwrap()
        .unwrap();
    assert_eq!(textures.version(), Some(cao_archive::ArchiveVersion::Fo4Dx));
    let names: Vec<_> = textures
        .archived_assets()
        .unwrap()
        .into_iter()
        .map(|asset| asset.name)
        .collect();
    assert_eq!(names, [r"textures\sky.dds"]);
    let general = cao_archive::ReadArchive::open(&first.join("mods/Mod/Mod.bsa"))
        .unwrap()
        .unwrap();
    assert_eq!(general.version(), Some(cao_archive::ArchiveVersion::Sse));
    let mut script = Vec::new();
    general.extract(r"scripts\quest.pex", &mut script).unwrap();
    assert_eq!(script, b"compiled");
    assert!(
        general
            .archived_assets()
            .unwrap()
            .iter()
            .all(|asset| !asset.compressed)
    );

    for (index, packed) in [
        serde_json::json!({"kind": "directory", "path": "meshes"}),
        serde_json::json!({"kind": "text", "path": "../escape.txt", "text": "x"}),
        serde_json::json!({"kind": "text", "path": "sound/aux.wav", "text": "x"}),
        serde_json::json!({"kind": "text", "path": "sound/caf\u{e9}.wav", "text": "x"}),
    ]
    .into_iter()
    .enumerate()
    {
        let case = shaped(
            serde_json::json!([{"kind": "archive", "path": "mods/Mod/Mod.bsa", "game": "sse",
                                "type": "standard", "content": [packed.clone()]}]),
            serde_json::json!([]),
        );
        let root = temp.path().join(format!("invalid-{index}"));
        let error = write_input(&case, "archives", &root, &fixtures_dir()).unwrap_err();
        assert!(
            matches!(error, HarnessError::InvalidCase(_)),
            "{packed}: {error}"
        );
    }
}

#[test]
fn every_named_format_round_trips_through_its_json_name() {
    let bc7: TextureFormat = serde_json::from_value(serde_json::json!("BC7_UNORM")).unwrap();
    assert_eq!(bc7.0, DXGI_FORMAT::DXGI_FORMAT_BC7_UNORM);
    assert_eq!(serde_json::to_value(bc7).unwrap(), "BC7_UNORM");
    // Every format DirectXTex names, vendor formats included.
    for value in 1..=191u32 {
        let format = TextureFormat(DXGI_FORMAT::from(value));
        if format!("{:?}", format.0).starts_with("0x") {
            continue;
        }
        let json = serde_json::to_value(format).unwrap();
        assert_eq!(
            serde_json::from_value::<TextureFormat>(json.clone()).unwrap(),
            format,
            "{json}"
        );
    }
    assert!(serde_json::from_value::<TextureFormat>(serde_json::json!("BC8_UNORM")).is_err());
}

#[test]
fn a_junction_path_may_not_hold_what_cmd_would_interpret() {
    let temp = TempDir::new("materialise-junction-cmd");
    // `^` rather than `&`: the deviation guard already rejects `&` in any path
    // (deviation 22), before this check could see it.
    let case = shaped(
        serde_json::json!([{"kind": "directory", "path": "mods/Mod/real"}]),
        serde_json::json!([{"op": "junction", "path": "mods/Mod/a^b", "target": "mods/Mod/real"}]),
    );
    let (layout, readiness) = materialised(&temp, "junction-cmd", &case, false);
    let error = readiness.unwrap_err();
    assert!(
        matches!(&error, HarnessError::InvalidCase(message) if message.contains("a^b")),
        "{error}"
    );
    assert!(!layout.input().exists());
}

#[test]
fn every_committed_seed_builds() {
    let temp = TempDir::new("materialise-seeds");
    let seeds = seeds().unwrap();
    assert!(
        seeds.iter().any(|(id, _)| id == "tracer-dry-run-textures"),
        "{:?}",
        seeds.iter().map(|(id, _)| id).collect::<Vec<_>>()
    );
    // Seeds using the pool are built from a stand-in for it, so they are
    // checked here whether or not this host has the real one.
    let ids: Vec<String> = seeds
        .iter()
        .flat_map(|(_, case)| local_asset_ids(case))
        .collect();
    let pool = stand_in_pool(&temp.path().join("pool"), &ids);
    for (id, case) in seeds {
        let local_assets = pool
            .fetch(local_asset_ids(&case).iter().map(String::as_str))
            .unwrap_or_else(|reason| panic!("seed `{id}`: {reason}"));
        let sources = Sources {
            fixtures: &fixtures_dir(),
            local_assets: &local_assets,
        };
        write_input_from(&case, &id, &temp.path().join(&id), &sources)
            .unwrap_or_else(|error| panic!("seed `{id}`: {error}"));
    }
}

/// The ids of every `local_asset` entry in `case`, packed ones included.
fn local_asset_ids(case: &CaseFile) -> Vec<String> {
    case.tree
        .content
        .iter()
        .flat_map(|entry| match entry {
            ContentEntry::Archive(archive) => archive.content.as_slice(),
            _ => std::slice::from_ref(entry),
        })
        .filter_map(|entry| match entry {
            ContentEntry::LocalAsset(asset) => Some(asset.asset.clone()),
            _ => None,
        })
        .collect()
}

/// A pool at `root` standing in for the committed pinned list's entries
/// `ids`: each keeps its id, edition, BSA and internal path, but holds
/// stand-in bytes, and is pinned by their hash.
///
/// Panics when an id is not in the committed list, which is a seed error.
fn stand_in_pool(root: &Path, ids: &[String]) -> LocalAssetPool {
    let committed = PinnedList::committed().unwrap();
    let mut pinned = PinnedList::default();
    for id in ids {
        if pinned.get(id).is_some() {
            continue;
        }
        let mut asset = committed
            .get(id)
            .unwrap_or_else(|| panic!("`{id}` is not in the pinned list"))
            .clone();
        asset.sha256 = sha256_hex(stand_in_bytes(id).as_bytes());
        pinned.assets.push(asset);
    }
    // One BSA per edition folder and archive name, as the real pool holds.
    let mut archives: Vec<(String, Edition, Vec<serde_json::Value>)> = Vec::new();
    for asset in &pinned.assets {
        let path = format!("{}/{}", asset.edition.folder(), asset.archive);
        let entry = serde_json::json!(
            {"kind": "text", "path": asset.path, "text": stand_in_bytes(&asset.id)}
        );
        match archives.iter_mut().find(|(known, _, _)| *known == path) {
            Some((_, _, content)) => content.push(entry),
            None => archives.push((path, asset.edition, vec![entry])),
        }
    }
    let content: Vec<serde_json::Value> = archives
        .into_iter()
        .map(|(path, edition, content)| {
            // LE's BSAs are TES5 (v104), SSE's are v105.
            let game = match edition {
                Edition::Le => "tes5",
                Edition::Sse => "sse",
            };
            serde_json::json!(
                {"kind": "archive", "path": path, "game": game, "type": "standard",
                 "content": content}
            )
        })
        .collect();
    write_input(
        &shaped(serde_json::Value::Array(content), serde_json::json!([])),
        "stand-in-pool",
        root,
        &fixtures_dir(),
    )
    .unwrap();
    LocalAssetPool::new(root.to_path_buf(), pinned)
}

/// The stand-in bytes of the pinned entry `id`.
fn stand_in_bytes(id: &str) -> String {
    format!("stand-in for {id}")
}

#[test]
fn textures_have_the_shape_the_recipe_asks_for() {
    let temp = TempDir::new("materialise-textures");
    let root = temp.path();
    write_input(&case(every_content_kind()), "shapes", root, &fixtures_dir()).unwrap();
    let textures = root.join("mods/Mod/textures");

    let gradient = metadata(&textures.join("gradient.dds"));
    assert_eq!(
        (gradient.width, gradient.height, gradient.mip_levels),
        (16, 8, 1)
    );
    assert_eq!(gradient.format, DXGI_FORMAT::DXGI_FORMAT_R8G8B8A8_UNORM);

    // 32x32 has six levels down to 1x1.
    let noise = metadata(&textures.join("noise_bc1.dds"));
    assert_eq!(noise.format, DXGI_FORMAT::DXGI_FORMAT_BC1_UNORM);
    assert_eq!(noise.mip_levels, 6);

    let cube = metadata(&textures.join("sky_cube.dds"));
    assert!(cube.is_cubemap());
    assert_eq!(
        (cube.array_size, cube.format),
        (6, DXGI_FORMAT::DXGI_FORMAT_BC3_UNORM)
    );

    let layers = metadata(&textures.join("layers.dds"));
    assert_eq!((layers.array_size, layers.mip_levels), (3, 2));
    assert_eq!((layers.width, layers.height), (12, 6));

    assert_eq!(
        metadata(&textures.join("raw.dds")).format,
        DXGI_FORMAT::DXGI_FORMAT_R8G8B8A8_TYPELESS
    );
    assert_eq!(
        metadata(&textures.join("bump_n.dds")).format,
        DXGI_FORMAT::DXGI_FORMAT_BC5_UNORM
    );

    // Byte 84 is the pixel format's FourCC: "DX10" only when asked for.
    let fourcc = |name: &str| std::fs::read(textures.join(name)).unwrap()[84..88].to_vec();
    assert_eq!(fourcc("edges_bc7.dds"), b"DX10");
    assert_ne!(fourcc("gradient.dds"), b"DX10");

    // A TGA: an 18-byte header with image type 2 (uncompressed true-colour).
    let tga = std::fs::read(textures.join("old.tga")).unwrap();
    assert_eq!(tga[2], 2);
    assert_eq!(u16::from_le_bytes([tga[12], tga[13]]), 8);

    // Faults damage the generated file.
    assert_eq!(std::fs::read(textures.join("cut.dds")).unwrap().len(), 100);
    assert!(
        std::fs::read(textures.join("empty.dds"))
            .unwrap()
            .is_empty()
    );
    // An 8x8 R8G8B8A8 DDS is a 128-byte legacy header and 256 bytes of pixels.
    let junk = std::fs::read(textures.join("junk.dds")).unwrap();
    assert_eq!(junk.len(), 384);
    assert_ne!(&junk[..4], b"DDS ");

    assert_eq!(
        std::fs::read(root.join("mods/Mod/readme.txt")).unwrap(),
        b"Hello\r\n"
    );
    assert_eq!(
        std::fs::read(root.join("mods/Mod/meshes/blob.nif")).unwrap(),
        [0x00, 0x01, 0x02, 0xff]
    );
    assert!(root.join("mods/Mod/empty").is_dir());
}

/// Materialises `case` as case `id` under `temp`, with no `hkxcmd.exe`.
fn materialised(
    temp: &TempDir,
    id: &str,
    case: &CaseFile,
    symlink_rights: bool,
) -> (CaseLayout, Result<Readiness, HarnessError>) {
    materialised_with_pool(temp, id, case, symlink_rights, common::empty_pool())
}

/// [`materialised`], with `pool` as the local asset pool.
fn materialised_with_pool(
    temp: &TempDir,
    id: &str,
    case: &CaseFile,
    symlink_rights: bool,
    pool: &LocalAssetPool,
) -> (CaseLayout, Result<Readiness, HarnessError>) {
    let layout = CaseLayout::new(temp.path(), id).unwrap();
    let profiles = shipped_profiles();
    let environment = Environment {
        resources: SideResources {
            profiles: &profiles,
            hkxcmd: None,
        },
        fixtures: &fixtures_dir(),
        symlink_rights,
        local_assets: pool,
    };
    let readiness = materialise(&layout, case, &environment);
    (layout, readiness)
}

/// The case tree under a side's root: everything but its harness-owned folders.
fn case_tree(root: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
    snapshot(root)
        .into_iter()
        .filter(|(path, _)| {
            let first = path.components().next().unwrap().as_os_str();
            !cao_parity::case::is_harness_owned(first)
        })
        .collect()
}

/// The three copies a case is materialised into.
fn roots(layout: &CaseLayout) -> [PathBuf; 3] {
    [
        layout.input(),
        layout.side(Side::Oracle),
        layout.side(Side::Rust),
    ]
}

/// A case whose tree is `content` and `fs_shape`, given as JSON.
fn shaped(content: serde_json::Value, fs_shape: serde_json::Value) -> CaseFile {
    case(
        serde_json::from_value(serde_json::json!({"content": content, "fs_shape": fs_shape}))
            .unwrap(),
    )
}

#[test]
fn both_sides_are_byte_identical_copies_of_the_input_before_shaping() {
    let temp = TempDir::new("materialise-copies");
    let (layout, readiness) = materialised(&temp, "copies", &case(every_content_kind()), false);
    assert_eq!(readiness.unwrap(), Readiness::Ready);

    let input = case_tree(&layout.input());
    assert_eq!(input.len(), 19, "{input:?}");
    assert_eq!(case_tree(&layout.side(Side::Oracle)), input);
    assert_eq!(case_tree(&layout.side(Side::Rust)), input);
    // Without overrides, each side's profile is the shipped one.
    for side in [Side::Oracle, Side::Rust] {
        assert_eq!(
            std::fs::read(layout.side(side).join("profiles/SSE/profile.ini")).unwrap(),
            std::fs::read(shipped_profiles().join("SSE/profile.ini")).unwrap()
        );
    }
}

#[test]
fn profile_overrides_are_written_into_each_sides_profile_ini() {
    let temp = TempDir::new("materialise-overrides");
    let mut case = case(every_content_kind());
    case.profile_overrides = serde_json::from_value(serde_json::json!({
        "output_format": "BC1_UNORM",
        "unwanted_formats": ["B5G6R5_UNORM"],
        "compress_interface": false,
        "convert_tga": false,
        "mesh_target": "le"
    }))
    .unwrap();
    let (layout, readiness) = materialised(&temp, "overrides", &case, false);
    assert_eq!(readiness.unwrap(), Readiness::Ready);

    let path = |side| layout.side(side).join("profiles/SSE/profile.ini");
    let ini = cao_profiles::IniFile::load(&path(Side::Rust)).unwrap();
    assert_eq!(ini.value("Textures/texturesFormat").to_i32(), 71);
    assert_eq!(
        ini.value("Textures/texturesUnwantedFormats").to_int_list(),
        [85]
    );
    assert!(!ini.value("Textures/texturesCompressInterface").to_bool());
    assert!(!ini.value("Textures/texturesConvertTga").to_bool());
    assert_eq!(ini.value("Meshes/meshesFileVersion").to_i32(), 335_675_399);
    assert_eq!(ini.value("Meshes/meshesUser").to_i32(), 12);
    assert_eq!(ini.value("Meshes/meshesStream").to_i32(), 83);
    // Values the overrides do not name keep the profile's, dead data included.
    assert_eq!(ini.value("BSA/bsaGame").to_i32(), 4);
    assert_eq!(ini.value("Animations/animationFormat").to_i32(), 3);
    // One writer, so both sides read the same bytes.
    assert_eq!(
        std::fs::read(path(Side::Oracle)).unwrap(),
        std::fs::read(path(Side::Rust)).unwrap()
    );
}

#[test]
fn a_case_needing_symlinks_is_not_run_without_symlink_rights() {
    let temp = TempDir::new("materialise-no-symlinks");
    let case = shaped(
        serde_json::json!([{"kind": "text", "path": "mods/Mod/a.txt", "text": "a"}]),
        serde_json::json!([{"op": "file_symlink", "path": "mods/Mod/b.txt", "target": "mods/Mod/a.txt"}]),
    );
    let (layout, readiness) = materialised(&temp, "no-symlinks", &case, false);
    let Readiness::NotRun(reason) = readiness.unwrap() else {
        panic!("a symlink case runs without symlink rights");
    };
    assert!(reason.contains("symlink"), "{reason}");
    for root in roots(&layout) {
        assert!(!root.exists(), "{} was written", root.display());
    }
}

#[test]
fn a_case_requesting_animations_is_not_run_without_hkxcmd() {
    let temp = TempDir::new("materialise-no-hkxcmd");
    let mut case = case(every_content_kind());
    case.spec.animations = true;
    let (layout, readiness) = materialised(&temp, "no-hkxcmd", &case, true);
    let Readiness::NotRun(reason) = readiness.unwrap() else {
        panic!("an Animations case runs without hkxcmd.exe");
    };
    assert!(reason.contains("hkxcmd.exe"), "{reason}");
    assert!(!layout.input().exists());
}

/// A text entry's path below `mods/Mod/` whose length is exactly `length`
/// UTF-16 units, split into folders no longer than 100 characters.
fn path_of_length(length: usize) -> String {
    let mut path = String::from("mods/Mod/");
    while length - path.len() > 104 {
        path.push_str(&"d".repeat(99));
        path.push('/');
    }
    path.push_str(&"f".repeat(length - path.len() - 4));
    path.push_str(".txt");
    assert_eq!(path.len(), length);
    path
}

#[test]
fn the_path_cap_is_enforced_on_every_copy() {
    let temp = TempDir::new("materialise-cap");
    // Both cases use ids of one length, so one prefix length serves both and
    // the second case lands exactly one unit over the cap.
    let (fits, over) = ("cap-a", "cap-b");
    let layout = CaseLayout::new(temp.path(), fits).unwrap();
    // `oracle` is the longest of the three root names, so it meets the cap first.
    let oracle = std::path::absolute(layout.side(Side::Oracle)).unwrap();
    let prefix = oracle.as_os_str().len() + 1;
    let text = |path: String| {
        shaped(
            serde_json::json!([{"kind": "text", "path": path, "text": "x"}]),
            serde_json::json!([]),
        )
    };

    let at_cap = path_of_length(PATH_CAP_UTF16 - prefix);
    let (layout, readiness) = materialised(&temp, fits, &text(at_cap.clone()), false);
    assert_eq!(readiness.unwrap(), Readiness::Ready);
    let written = layout.side(Side::Oracle).join(&at_cap);
    assert_eq!(written.as_os_str().len(), PATH_CAP_UTF16);
    assert!(written.is_file());

    let one_over = path_of_length(PATH_CAP_UTF16 - prefix + 1);
    let (layout, readiness) = materialised(&temp, over, &text(one_over), false);
    let error = readiness.unwrap_err();
    assert!(
        matches!(&error, HarnessError::InvalidCase(message)
            if message.contains("is 401 UTF-16 units long, over the 400-unit cap")),
        "{error}"
    );
    assert!(
        !layout.input().exists(),
        "nothing is written for an invalid case"
    );
}

#[test]
fn a_hardlink_joins_two_names_within_each_copy() {
    let temp = TempDir::new("materialise-hardlink");
    let case = shaped(
        serde_json::json!([{"kind": "texture", "path": "mods/Mod/textures/a.dds",
                            "format": "R8G8B8A8_UNORM", "width": 4, "height": 4}]),
        serde_json::json!([{"op": "hardlink", "path": "mods/Mod/textures/b.dds",
                            "target": "mods/Mod/textures/a.dds"}]),
    );
    let (layout, readiness) = materialised(&temp, "hardlink", &case, false);
    assert_eq!(readiness.unwrap(), Readiness::Ready);
    let original = std::fs::read(layout.input().join("mods/Mod/textures/a.dds")).unwrap();
    for root in roots(&layout) {
        assert_eq!(
            std::fs::read(root.join("mods/Mod/textures/b.dds")).unwrap(),
            original
        );
    }
    // A write through the oracle's link reaches the oracle's file only.
    std::fs::write(
        layout.side(Side::Oracle).join("mods/Mod/textures/b.dds"),
        b"changed",
    )
    .unwrap();
    let read = |side| std::fs::read(layout.side(side).join("mods/Mod/textures/a.dds")).unwrap();
    assert_eq!(read(Side::Oracle), b"changed");
    assert_eq!(read(Side::Rust), original);
}

#[test]
fn a_junction_points_into_its_own_copy() {
    let temp = TempDir::new("materialise-junction");
    let case = shaped(
        serde_json::json!([{"kind": "text", "path": "mods/Mod/textures/real/a.txt", "text": "a"}]),
        serde_json::json!([{"op": "junction", "path": "mods/Mod/textures/alias",
                            "target": "mods/Mod/textures/real"}]),
    );
    let (layout, readiness) = materialised(&temp, "junction", &case, false);
    assert_eq!(readiness.unwrap(), Readiness::Ready);
    for root in roots(&layout) {
        let alias = root.join("mods/Mod/textures/alias");
        assert!(
            std::fs::symlink_metadata(&alias)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::canonicalize(&alias).unwrap(),
            std::fs::canonicalize(root.join("mods/Mod/textures/real")).unwrap()
        );
        assert_eq!(std::fs::read(alias.join("a.txt")).unwrap(), b"a");
    }
}

#[test]
#[cfg_attr(
    not(symlink_privilege),
    ignore = "creating file symlinks needs SeCreateSymbolicLinkPrivilege or Developer Mode"
)]
fn a_file_symlink_points_into_its_own_copy() {
    let temp = TempDir::new("materialise-symlink");
    let case = shaped(
        serde_json::json!([{"kind": "text", "path": "mods/Mod/a.txt", "text": "a"}]),
        serde_json::json!([{"op": "file_symlink", "path": "mods/Mod/b.txt", "target": "mods/Mod/a.txt"}]),
    );
    let (layout, readiness) = materialised(&temp, "symlink", &case, true);
    assert_eq!(readiness.unwrap(), Readiness::Ready);
    for root in roots(&layout) {
        let link = root.join("mods/Mod/b.txt");
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::canonicalize(&link).unwrap(),
            std::fs::canonicalize(root.join("mods/Mod/a.txt")).unwrap()
        );
    }
}

#[test]
fn a_readonly_file_is_readonly_in_every_copy_and_still_removable() {
    let temp = TempDir::new("materialise-readonly");
    let case = shaped(
        serde_json::json!([{"kind": "texture", "path": "mods/Mod/textures/a.dds",
                            "format": "R8G8B8A8_UNORM", "width": 4, "height": 4},
                           {"kind": "text", "path": "mods/Mod/textures/b.txt", "text": "b"}]),
        serde_json::json!([{"op": "readonly", "path": "mods/Mod/textures/a.dds"}]),
    );
    let (layout, readiness) = materialised(&temp, "readonly", &case, false);
    assert_eq!(readiness.unwrap(), Readiness::Ready);
    for root in roots(&layout) {
        let permissions = |name: &str| {
            std::fs::metadata(root.join("mods/Mod/textures").join(name))
                .unwrap()
                .permissions()
        };
        assert!(permissions("a.dds").readonly());
        assert!(!permissions("b.txt").readonly());
    }
    // Passing cases are deleted, read-only files included.
    std::fs::remove_dir_all(layout.root()).unwrap();
}

/// `path` as a `\\?\` path, which reaches a file named like a device.
fn verbatim(path: &Path) -> PathBuf {
    let mut text = std::ffi::OsString::from(r"\\?\");
    text.push(std::path::absolute(path).unwrap());
    PathBuf::from(text)
}

#[test]
fn a_reserved_name_is_created_through_a_verbatim_path_in_every_copy() {
    let temp = TempDir::new("materialise-reserved");
    let case = shaped(
        serde_json::json!([{"kind": "texture", "path": "mods/Mod/textures/source.dds",
                            "format": "R8G8B8A8_UNORM", "width": 4, "height": 4}]),
        serde_json::json!([{"op": "reserved_name", "path": "mods/Mod/textures/NUL.dds",
                            "from": "mods/Mod/textures/source.dds"}]),
    );
    let (layout, readiness) = materialised(&temp, "reserved", &case, false);
    assert_eq!(readiness.unwrap(), Readiness::Ready);
    let mut contents = Vec::new();
    for root in roots(&layout) {
        let textures = root.join("mods").join("Mod").join("textures");
        assert!(!textures.join("source.dds").exists());
        let bytes = std::fs::read(verbatim(&textures.join("NUL.dds"))).unwrap();
        assert_eq!(&bytes[..4], b"DDS ");
        contents.push(bytes);
    }
    assert!(contents.windows(2).all(|pair| pair[0] == pair[1]));
    std::fs::remove_dir_all(layout.root()).unwrap();
}

#[test]
fn reserved_names_come_only_from_the_reserved_name_operation() {
    let temp = TempDir::new("materialise-reserved-invalid");
    let content_named_like_a_device = shaped(
        serde_json::json!([{"kind": "text", "path": "mods/Mod/textures/com1.txt", "text": "x"}]),
        serde_json::json!([]),
    );
    let not_a_device = shaped(
        serde_json::json!([{"kind": "text", "path": "mods/Mod/a.txt", "text": "x"}]),
        serde_json::json!([{"op": "reserved_name", "path": "mods/Mod/b.txt", "from": "mods/Mod/a.txt"}]),
    );
    for (name, case) in [
        ("device", content_named_like_a_device),
        ("plain", not_a_device),
    ] {
        let error = write_input(&case, name, &temp.path().join(name), &fixtures_dir()).unwrap_err();
        assert!(
            matches!(error, HarnessError::InvalidCase(_)),
            "{name}: {error}"
        );
    }
}

#[test]
fn game_paths_stay_ascii_while_mod_roots_and_plugins_need_not() {
    let temp = TempDir::new("materialise-ascii");
    let several = |path: &str| {
        let mut case = shaped(
            serde_json::json!([{"kind": "text", "path": path, "text": "x"}]),
            serde_json::json!([]),
        );
        case.spec.mod_selection = cao_parity::case::ModSelection::SeveralMods {
            folder: "Modsé".into(),
        };
        case
    };
    for (index, accepted) in [
        "Modsé/Modé/textures/a.dds",
        "Modsé/Modé/Modé.esp",
        "Modsé/Modé/Modé - Textures.bsa",
        "notes/é.txt",
    ]
    .into_iter()
    .enumerate()
    {
        let root = temp.path().join(format!("accepted-{index}"));
        write_input(&several(accepted), "ascii", &root, &fixtures_dir())
            .unwrap_or_else(|error| panic!("{accepted}: {error}"));
    }
    for (index, rejected) in ["Modsé/Modé/textures/é.dds", "Modsé/Modé/é.txt"]
        .into_iter()
        .enumerate()
    {
        let root = temp.path().join(format!("rejected-{index}"));
        let error = write_input(&several(rejected), "ascii", &root, &fixtures_dir()).unwrap_err();
        assert!(
            matches!(error, HarnessError::InvalidCase(_)),
            "{rejected}: {error}"
        );
    }
}

/// The bytes of the pool's Mesh in [`pool_at`].
const POOL_MESH: &[u8] = b"a pooled mesh's bytes";
/// The bytes of the pool's Animation in [`pool_at`].
const POOL_ANIMATION: &[u8] = b"a pooled animation's bytes";

/// Builds a pool at `root` whose SSE BSA `Pool - Meshes.bsa` holds
/// [`POOL_MESH`] and [`POOL_ANIMATION`], and pins them as `sse-static-a.nif`
/// and `sse-animation-b.hkx`. `mesh_sha256` replaces the mesh's true hash, and
/// `mesh_path` its internal path, so a test can pin bytes the pool lacks.
fn pool_at(root: &Path, mesh_sha256: Option<&str>, mesh_path: &str) -> LocalAssetPool {
    let text = |bytes: &[u8]| std::str::from_utf8(bytes).unwrap().to_owned();
    let bsa = shaped(
        serde_json::json!([
            {"kind": "archive", "path": "sse/Pool - Meshes.bsa", "game": "sse",
             "type": "standard", "content": [
                {"kind": "text", "path": "meshes/pool/a.nif", "text": text(POOL_MESH)},
                {"kind": "text", "path": "meshes/actors/b.hkx", "text": text(POOL_ANIMATION)}]}
        ]),
        serde_json::json!([]),
    );
    // The case id seeds the Archive's scratch folder, so pools packed on
    // parallel test threads need their own ids, or they share one folder.
    let id = format!("pool-{}", root.file_name().unwrap().to_string_lossy());
    write_input(&bsa, &id, root, &fixtures_dir()).unwrap();
    let mesh_sha256 = mesh_sha256.map_or_else(|| sha256_hex(POOL_MESH), str::to_owned);
    let pinned = PinnedList::parse(&format!(
        r#"
[[asset]]
id = "sse-static-a.nif"
edition = "sse"
archive = "Pool - Meshes.bsa"
path = "{mesh_path}"
category = "static"
sha256 = "{mesh_sha256}"

[[asset]]
id = "sse-animation-b.hkx"
edition = "sse"
archive = "Pool - Meshes.bsa"
path = "meshes/actors/b.hkx"
category = "animation"
sha256 = "{}"
"#,
        sha256_hex(POOL_ANIMATION)
    ))
    .unwrap();
    LocalAssetPool::new(root.to_path_buf(), pinned)
}

/// A case using the pooled mesh loose, truncated, and the pooled animation
/// packed into an input Archive.
fn uses_the_pool() -> CaseFile {
    shaped(
        serde_json::json!([
            {"kind": "local_asset", "path": "mods/Mod/meshes/a.nif", "asset": "sse-static-a.nif"},
            {"kind": "local_asset", "path": "mods/Mod/meshes/cut.nif", "asset": "sse-static-a.nif",
             "fault": {"truncate": 3}},
            {"kind": "archive", "path": "mods/Mod/Mod.bsa", "game": "sse", "type": "standard",
             "content": [
                {"kind": "local_asset", "path": "meshes/actors/b.hkx",
                 "asset": "sse-animation-b.hkx"}]}
        ]),
        serde_json::json!([]),
    )
}

/// A `local_asset` entry writes the pinned entry's bytes into every copy,
/// loose or packed, and a fault decorator damages them as it would a texture.
#[test]
fn a_local_asset_entry_writes_the_pinned_bytes_into_every_copy() {
    let temp = TempDir::new("materialise-pool");
    let pool = pool_at(&temp.path().join("pool"), None, "meshes/pool/a.nif");
    let (layout, readiness) =
        materialised_with_pool(&temp, "pooled", &uses_the_pool(), false, &pool);
    assert_eq!(readiness.unwrap(), Readiness::Ready);

    for root in roots(&layout) {
        let read = |path: &str| std::fs::read(root.join(path)).unwrap();
        assert_eq!(read("mods/Mod/meshes/a.nif"), POOL_MESH);
        assert_eq!(read("mods/Mod/meshes/cut.nif"), &POOL_MESH[..3]);
        let archive = cao_archive::ReadArchive::open(&root.join("mods/Mod/Mod.bsa"))
            .unwrap()
            .unwrap();
        let mut packed = Vec::new();
        archive.extract("meshes/actors/b.hkx", &mut packed).unwrap();
        assert_eq!(packed, POOL_ANIMATION);
    }
}

/// A case whose pool entry is missing or changed is not run, with a reason
/// naming the entry, and nothing of it is written.
#[test]
fn a_case_is_not_run_when_the_pool_cannot_supply_a_pinned_entry() {
    let temp = TempDir::new("materialise-pool-not-run");
    let changed = "0".repeat(64);
    let pools = [
        (
            "no-pool",
            LocalAssetPool::new(
                temp.path().join("absent"),
                pool_at(&temp.path().join("unused"), None, "meshes/pool/a.nif")
                    .pinned()
                    .clone(),
            ),
            "Pool - Meshes.bsa is missing".to_owned(),
        ),
        (
            "no-entry",
            pool_at(&temp.path().join("no-entry"), None, "meshes/pool/gone.nif"),
            "`meshes/pool/gone.nif` cannot be extracted".to_owned(),
        ),
        (
            "changed",
            pool_at(
                &temp.path().join("changed"),
                Some(&changed),
                "meshes/pool/a.nif",
            ),
            format!(
                "has SHA-256 {}, not the pinned {changed}",
                sha256_hex(POOL_MESH)
            ),
        ),
    ];
    for (id, pool, expected) in pools {
        let (layout, readiness) = materialised_with_pool(&temp, id, &uses_the_pool(), false, &pool);
        let Readiness::NotRun(reason) = readiness.unwrap() else {
            panic!("{id}: the case ran");
        };
        assert!(
            reason.contains("the local asset `sse-static-a.nif`") && reason.contains(&expected),
            "{id}: {reason}"
        );
        assert!(!layout.input().exists(), "{id}: input/ was written");
    }
}

/// An id the pinned list lacks is a recipe error, reported even on a host
/// with no pool, rather than a reason not to run.
#[test]
fn an_unpinned_local_asset_is_a_recipe_error_even_without_a_pool() {
    let temp = TempDir::new("materialise-pool-unpinned");
    let case = shaped(
        serde_json::json!([
            {"kind": "local_asset", "path": "mods/Mod/meshes/a.nif", "asset": "sse-static-nope.nif"}
        ]),
        serde_json::json!([]),
    );
    let (_, readiness) = materialised(&temp, "unpinned", &case, false);
    let error = readiness.unwrap_err();
    assert!(
        matches!(&error, HarnessError::InvalidCase(message) if message.contains("sse-static-nope.nif")),
        "{error}"
    );
}

/// Pool bytes keep their pinned extension, loose or packed, so they never
/// pose as a plugin or Texture whose content the deviation guard reads. The
/// pool is never touched: it is a recipe error.
#[test]
fn a_local_asset_must_keep_its_pinned_extension() {
    let temp = TempDir::new("materialise-pool-extension");
    let pool = LocalAssetPool::new(
        temp.path().join("absent"),
        pool_at(&temp.path().join("pinned"), None, "meshes/pool/a.nif")
            .pinned()
            .clone(),
    );
    let loose = serde_json::json!(
        {"kind": "local_asset", "path": "mods/Mod/Mod.esp", "asset": "sse-static-a.nif"});
    let packed = serde_json::json!(
        {"kind": "archive", "path": "mods/Mod/Mod.bsa", "game": "sse", "type": "standard",
         "content": [{"kind": "local_asset", "path": "textures/a.dds",
                      "asset": "sse-static-a.nif"}]});
    for (index, entry) in [loose, packed].into_iter().enumerate() {
        let case = shaped(serde_json::json!([entry]), serde_json::json!([]));
        let (_, readiness) =
            materialised_with_pool(&temp, &format!("ext-{index}"), &case, false, &pool);
        let error = readiness.unwrap_err();
        assert!(
            matches!(&error, HarnessError::InvalidCase(message)
                if message.contains("keeps its pinned extension")),
            "{entry}: {error}"
        );
    }
    let case = shaped(
        serde_json::json!([
            {"kind": "local_asset", "path": "mods/Mod/meshes/A.NIF", "asset": "sse-static-a.nif"}
        ]),
        serde_json::json!([]),
    );
    let (_, readiness) = materialised_with_pool(&temp, "ext-case", &case, false, &pool);
    assert!(
        matches!(readiness.unwrap(), Readiness::NotRun(_)),
        "the extension is matched ignoring case"
    );
}

/// Loads a Mesh the materialiser wrote, through the same nifly the Rust
/// build's mesh backend loads with.
fn load_mesh(path: &Path) -> Nif {
    let mut nif = Nif::new();
    nif.load(path, LoadOptions::default())
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    nif
}

/// Every texture-set slot of every shape, in shape order, as text.
fn texture_slots(nif: &mut Nif) -> Vec<String> {
    let paths = nif.texture_paths().unwrap();
    (0..paths.len())
        .map(|index| String::from_utf8(paths.get(index).unwrap().to_vec()).unwrap())
        .collect()
}

/// `filled` in slot order, then empty slots up to `slots`.
fn padded(filled: &[&str], slots: usize) -> Vec<String> {
    let mut paths: Vec<String> = filled.iter().map(|path| (*path).to_owned()).collect();
    paths.resize(slots, String::new());
    paths
}

/// A `mesh` entry is built by nifly (#503), as the C++ tests build theirs:
/// one shape per recipe shape, each texture in its slot, at the version asked
/// for.
#[test]
fn a_mesh_entry_writes_a_mesh_nifly_loads_back() {
    let temp = TempDir::new("materialise-mesh");
    let case = shaped(
        serde_json::json!([
            {"kind": "mesh", "path": "mods/Mod/meshes/clutter/bowl.nif", "version": "sse",
             "shapes": [
                {"name": "Bowl",
                 "textures": ["textures\\clutter\\bowl.tga", "", "textures\\clutter\\bowl_g.dds"]},
                {"name": "Lid"}]},
            {"kind": "mesh", "path": "mods/Mod/meshes/clutter/old.nif", "version": "le",
             "shapes": [{"name": "Old", "textures": ["textures\\clutter\\old.dds"]}]},
            {"kind": "mesh", "path": "mods/Mod/meshes/clutter/empty.nif", "version": "fo4"}
        ]),
        serde_json::json!([]),
    );
    let root = temp.path().join("input");
    write_input(&case, "meshes", &root, &fixtures_dir()).unwrap();

    let mut bowl = load_mesh(&root.join("mods/Mod/meshes/clutter/bowl.nif"));
    assert!(bowl.is_sse_compatible().unwrap());
    // Skyrim's texture sets have 9 slots; the Lid's are all empty.
    let mut expected = padded(
        &[
            r"textures\clutter\bowl.tga",
            "",
            r"textures\clutter\bowl_g.dds",
        ],
        9,
    );
    expected.extend(padded(&[], 9));
    assert_eq!(texture_slots(&mut bowl), expected);

    // An LE Mesh is really LE: nifly converts it to SSE without a mismatch.
    let mut old = load_mesh(&root.join("mods/Mod/meshes/clutter/old.nif"));
    let report = old
        .optimize_for(&OptimizeOptions {
            target: NifVersion::SSE,
            head_parts: false,
            remove_parallax: false,
        })
        .unwrap();
    assert!(!report.version_mismatch, "{report:?}");
    assert_eq!(texture_slots(&mut old)[0], r"textures\clutter\old.dds");

    // No shapes is a Mesh holding only its root node.
    let mut empty = load_mesh(&root.join("mods/Mod/meshes/clutter/empty.nif"));
    assert!(texture_slots(&mut empty).is_empty());
}

#[test]
fn a_mesh_entry_takes_faults_and_may_be_packed() {
    let temp = TempDir::new("materialise-mesh-packed");
    let mesh = serde_json::json!(
        {"kind": "mesh", "path": "mods/Mod/meshes/whole.nif", "version": "sse",
         "shapes": [{"name": "Shape", "textures": ["textures\\a.dds"]}]});
    let case = shaped(
        serde_json::json!([
            mesh,
            {"kind": "mesh", "path": "mods/Mod/meshes/cut.nif", "version": "sse",
             "shapes": [{"name": "Shape", "textures": ["textures\\a.dds"]}],
             "fault": {"truncate": 40}},
            {"kind": "archive", "path": "mods/Mod/Mod.bsa", "game": "sse", "type": "standard",
             "content": [
                {"kind": "mesh", "path": "meshes/packed.nif", "version": "sse",
                 "shapes": [{"name": "Shape"}]}]}
        ]),
        serde_json::json!([]),
    );
    let root = temp.path().join("input");
    write_input(&case, "meshes", &root, &fixtures_dir()).unwrap();

    let whole = std::fs::read(root.join("mods/Mod/meshes/whole.nif")).unwrap();
    let cut = std::fs::read(root.join("mods/Mod/meshes/cut.nif")).unwrap();
    assert_eq!(cut, whole[..40], "the fault cuts the same Mesh short");
    assert!(root.join("mods/Mod/Mod.bsa").is_file());
}

#[test]
fn a_mesh_entry_must_be_a_mesh_nifly_can_build() {
    let temp = TempDir::new("materialise-mesh-invalid");
    let ten_textures: Vec<String> = (0..10).map(|n| format!("textures\\{n}.dds")).collect();
    for (index, (entry, reason)) in [
        (
            serde_json::json!({"kind": "mesh", "path": "mods/Mod/textures/a.dds",
                               "version": "sse"}),
            "`.nif`, `.btr` or `.bto`",
        ),
        (
            serde_json::json!({"kind": "mesh", "path": "mods/Mod/meshes/a.nif", "version": "sse",
                               "shapes": [{"name": "Shape", "textures": ten_textures}]}),
            "9 texture slots",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let case = shaped(serde_json::json!([entry.clone()]), serde_json::json!([]));
        let root = temp.path().join(format!("invalid-{index}"));
        let error = write_input(&case, "meshes", &root, &fixtures_dir()).unwrap_err();
        assert!(
            matches!(&error, HarnessError::InvalidCase(message) if message.contains(reason)),
            "{entry}: {error}"
        );
    }
}
