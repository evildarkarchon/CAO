//! The Selected Profile read: every profile fact one Optimization Run uses,
//! read once from copies of the shipped profiles and from edited copies.

mod common;

use cao_core::routing::{ProfileCapabilities, ProfileCapability};
use cao_optimizers::selected_profile::SelectedProfile;
use cao_optimizers::textures::TextureProfile;
use cao_profiles::{ProfileError, Profiles};
use common::{app_dir, edit_profile, write};
use directxtex::{
    DXGI_FORMAT_B5G6R5_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_BC3_UNORM,
    DXGI_FORMAT_BC7_UNORM, DXGI_FORMAT_BC7_UNORM_SRGB,
};
use nifly_sys::NifVersion;

/// Reads `identity` from the profiles of the install in `app`.
fn read(app: &std::path::Path, identity: &str) -> SelectedProfile {
    SelectedProfile::read(&Profiles::new(app), identity).unwrap()
}

#[test]
fn each_enabled_asset_kind_supports_all_of_its_work() {
    use ProfileCapability::*;
    let app = app_dir("selected-capabilities");

    // SSE enables every Asset Kind.
    assert_eq!(
        read(&app, "SSE").capabilities(),
        &ProfileCapabilities::define(
            ".bsa",
            &[
                NativeTextureOptimization,
                ConvertibleTextureConversion,
                StandardMeshOptimization,
                TerrainMeshOptimization,
                AnimationOptimization,
                ArchiveExtraction,
                MeshReferenceMaintenance,
                ArchiveCreation,
            ]
        )
    );
    // FO4 disables Meshes and Animations; Texture support still covers
    // conversion and Mesh Reference Maintenance.
    assert_eq!(
        read(&app, "FO4").capabilities(),
        &ProfileCapabilities::define(
            ".ba2",
            &[
                NativeTextureOptimization,
                ConvertibleTextureConversion,
                ArchiveExtraction,
                MeshReferenceMaintenance,
                ArchiveCreation,
            ]
        )
    );

    edit_profile(&app, "SSE", "texturesEnabled", "false");
    edit_profile(&app, "SSE", "bsaEnabled", "false");
    assert_eq!(
        read(&app, "SSE").capabilities(),
        &ProfileCapabilities::define(
            ".bsa",
            &[
                StandardMeshOptimization,
                TerrainMeshOptimization,
                AnimationOptimization,
            ]
        )
    );
}

#[test]
fn routing_and_the_archive_rules_name_one_game() {
    let app = app_dir("selected-archive-game");
    let cases: [(&str, &str, &[&str]); 3] = [
        ("TES5", ".bsa", &[".esm", ".esp"]),
        ("SSE", ".bsa", &[".esl", ".esm", ".esp"]),
        ("FO4", ".ba2", &[".esl", ".esm", ".esp"]),
    ];
    for (identity, extension, plugins) in cases {
        let selected = read(&app, identity);
        assert_eq!(
            selected.capabilities().archive_extension(),
            Some(extension),
            "{identity} routes by its game's Archive extension"
        );
        let archive = selected.archive_settings();
        assert_eq!(archive.extension, extension, "{identity} packs the same");
        assert_eq!(archive.plugin_extensions, plugins, "{identity}");
    }

    // The game number alone picks the table.
    edit_profile(&app, "FO4", "bsaGame", "4");
    let selected = read(&app, "FO4");
    assert_eq!(selected.capabilities().archive_extension(), Some(".bsa"));
    assert_eq!(selected.archive_settings().extension, ".bsa");
}

#[test]
fn the_conversion_preference_is_the_profile_tga_conversion_setting() {
    let app = app_dir("selected-conversion-preference");

    // The shipped SSE profile converts TGAs; TES5's does not.
    assert!(read(&app, "SSE").conversion_preference());
    assert!(!read(&app, "TES5").conversion_preference());

    edit_profile(&app, "SSE", "texturesConvertTga", "false");
    edit_profile(&app, "TES5", "texturesConvertTga", "true");
    assert!(!read(&app, "SSE").conversion_preference());
    assert!(read(&app, "TES5").conversion_preference());
}

#[test]
fn the_profile_can_only_raise_the_maximum_archive_size() {
    const MIB: u64 = 1024 * 1024;
    let app = app_dir("selected-archive-size");

    // SSE's 2104533975.04 is above the 2000 MiB table value and is truncated.
    assert_eq!(read(&app, "SSE").archive_settings().max_size, 2_104_533_975);
    // FO4's 4187593113.6 is below the 4000 MiB table value, which stands.
    assert_eq!(read(&app, "FO4").archive_settings().max_size, 4000 * MIB);

    edit_profile(&app, "SSE", "maxBsaUncompressedSize", "1000");
    assert_eq!(read(&app, "SSE").archive_settings().max_size, 2000 * MIB);
}

#[test]
fn the_texture_profile_comes_from_the_texture_keys() {
    let app = app_dir("selected-texture-profile");
    assert_eq!(
        read(&app, "TES5").texture_profile(),
        &TextureProfile {
            format: DXGI_FORMAT_BC3_UNORM,
            unwanted_formats: vec![DXGI_FORMAT_BC7_UNORM, DXGI_FORMAT_BC7_UNORM_SRGB],
            compress_interface: true,
        }
    );

    edit_profile(&app, "SSE", "texturesFormat", "87");
    edit_profile(&app, "SSE", "texturesUnwantedFormats", "85");
    edit_profile(&app, "SSE", "texturesCompressInterface", "false");
    assert_eq!(
        read(&app, "SSE").texture_profile(),
        &TextureProfile {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            unwanted_formats: vec![DXGI_FORMAT_B5G6R5_UNORM],
            compress_interface: false,
        }
    );
}

#[test]
fn the_mesh_target_is_the_profile_version_triple() {
    let app = app_dir("selected-mesh-target");
    assert_eq!(
        read(&app, "TES5").mesh_target(),
        NifVersion {
            file: 335_675_399,
            user: 12,
            stream: 83,
        }
    );

    edit_profile(&app, "SSE", "meshesStream", "130");
    edit_profile(&app, "SSE", "meshesUser", "11");
    assert_eq!(
        read(&app, "SSE").mesh_target(),
        NifVersion {
            file: 335_675_399,
            user: 11,
            stream: 130,
        }
    );
}

#[test]
fn the_profile_lists_are_read_with_the_profile() {
    let app = app_dir("selected-lists");
    let sse = app.join("profiles/SSE");
    write(&sse, "ignoredMods.txt", b"# comment\n  Tool   Mod  \n\n");
    write(
        &sse,
        "FilesToNotPack.txt",
        b"# comment\nreadme.txt\nCalienteTools/\n",
    );
    write(&sse, "customHeadparts.txt", b"meshes/hair/long.nif\n");

    // FO4 has none of these lists of its own, so it falls back to SSE's.
    let selected = read(&app, "FO4");
    assert_eq!(selected.ignored_mods(), ["Tool Mod"]);
    assert_eq!(
        selected.packing_exclusions(),
        ["readme.txt", "CalienteTools/"]
    );
    assert_eq!(selected.custom_headparts(), ["meshes/hair/long.nif"]);
}

#[test]
fn a_missing_or_unreadable_optional_list_reads_as_empty() {
    let app = app_dir("selected-lists-missing");
    let sse = app.join("profiles/SSE");
    for list in [
        "ignoredMods.txt",
        "FilesToNotPack.txt",
        "customHeadparts.txt",
    ] {
        std::fs::remove_file(sse.join(list)).unwrap();
    }
    let selected = read(&app, "SSE");
    assert!(selected.ignored_mods().is_empty());
    assert!(selected.packing_exclusions().is_empty());
    assert!(selected.custom_headparts().is_empty());

    // A directory cannot be read as a list.
    std::fs::create_dir(sse.join("FilesToNotPack.txt")).unwrap();
    std::fs::create_dir(sse.join("customHeadparts.txt")).unwrap();
    let selected = read(&app, "SSE");
    assert!(selected.packing_exclusions().is_empty());
    assert!(selected.custom_headparts().is_empty());
}

#[test]
fn an_unreadable_ignored_mod_list_fails_the_read() {
    let app = app_dir("selected-ignored-unreadable");
    // Ignoring it would process mods the profile means to exclude.
    std::fs::create_dir(app.join("profiles/FO4/ignoredMods.txt")).unwrap();
    let error = SelectedProfile::read(&Profiles::new(&app), "FO4").unwrap_err();
    assert!(matches!(error, ProfileError::IgnoredMods { .. }), "{error}");
}

#[test]
fn an_unsupported_game_number_fails_the_read() {
    let app = app_dir("selected-unsupported-game");
    // Deviation 11: there are no tables for any other game.
    edit_profile(&app, "SSE", "bsaGame", "7");
    let error = SelectedProfile::read(&Profiles::new(&app), "SSE").unwrap_err();
    assert!(
        matches!(&error, ProfileError::UnsupportedBsaGame { value } if value == "7"),
        "{error}"
    );

    let missing = SelectedProfile::read(&Profiles::new(&app), "MissingProfile").unwrap_err();
    assert!(
        matches!(missing, ProfileError::Unavailable { .. }),
        "{missing}"
    );
}
