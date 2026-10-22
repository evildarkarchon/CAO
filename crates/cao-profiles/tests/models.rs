//! The shipped `profiles/` loading into the options and settings model, and the
//! model saving back the way C++ CAO saves it.

mod common;

use cao_archive::Game;
use cao_profiles::ArchiveSize;
use cao_profiles::{
    CommonSettings, FormatErrorKind, IniFile, MeshFileVersion, MeshStreamVersion, MeshUserVersion,
    OptimizationMode, Options, ProfileError, ProfileSettings, Profiles, TextureFormat,
    UnwantedFormat,
};
use common::shipped_app_dir;

/// The shipped profiles, with the repository root as their app directory.
fn shipped() -> Profiles {
    Profiles::new(&shipped_app_dir())
}

#[test]
fn the_shipped_tes5_profile_loads_into_the_settings_model() {
    let settings = shipped().open("TES5").load_settings().unwrap();

    assert_eq!(
        settings,
        ProfileSettings {
            bsa_enabled: true,
            max_bsa_uncompressed_size: ArchiveSize::from_bytes(2_104_533_975),
            bsa_game: Game::Tes5,
            meshes_enabled: true,
            meshes_file_version: MeshFileVersion::V20_2_0_7,
            meshes_stream: MeshStreamVersion::V83,
            meshes_user: MeshUserVersion::V12,
            animations_enabled: false,
            textures_enabled: true,
            textures_format: TextureFormat::Bc3Unorm,
            textures_convert_tga: false,
            textures_unwanted_formats: vec![UnwantedFormat::Bc7Unorm, UnwantedFormat::Bc7UnormSrgb],
            textures_compress_interface: true,
        }
    );
}

#[test]
fn bsa_game_3_4_and_5_read_as_cao_archives_tes5_sse_and_fo4() {
    for (text, game) in [("3", Game::Tes5), ("4", Game::Sse), ("5", Game::Fo4)] {
        // A Mesh Target and Texture format too, whose missing keys would make the
        // Profile unreadable.
        let ini = IniFile::parse(
            format!(
                "[BSA]\r\nbsaGame={text}\r\n[Meshes]\r\nmeshesFileVersion=335675399\r\n\
                 meshesStream=100\r\nmeshesUser=12\r\n[Textures]\r\ntexturesFormat=98\r\n"
            )
            .as_bytes(),
        );

        assert_eq!(
            ProfileSettings::read(&ini).unwrap().bsa_game,
            game,
            "{text}"
        );
    }
}

#[test]
fn any_other_bsa_game_makes_the_profile_unreadable_naming_the_value() {
    // CAO has Archive Rules only for these three games, so bethutil's other
    // numbers (TES3 is 0, which text also reads as; TES4 1; FNV 2) and any
    // other number are refused rather than packed by another game's rules.
    for text in ["0", "1", "2", "6", "-4", "SSE", "4.0"] {
        let ini = IniFile::parse(format!("[BSA]\r\nbsaGame={text}\r\n").as_bytes());

        let error = ProfileSettings::read(&ini).unwrap_err();

        assert!(
            matches!(&error, ProfileError::UnsupportedBsaGame { value } if value == text),
            "{text}: {error:?}"
        );
        assert!(
            error.to_string().contains(&format!("bsaGame={text}")),
            "{error}"
        );
    }
}

#[test]
fn a_missing_bsa_game_makes_the_profile_unreadable() {
    // A missing key reads as 0, which names no game CAO has rules for.
    let ini = IniFile::parse(b"[BSA]\r\nbsaEnabled=true\r\n");

    assert!(matches!(
        ProfileSettings::read(&ini),
        Err(ProfileError::UnsupportedBsaGame { value }) if value.is_empty()
    ));
}

#[test]
fn the_shipped_sse_settings_load_into_the_options_model() {
    let options = shipped()
        .open("SSE")
        .load_options(&Options::default())
        .unwrap();

    assert_eq!(
        options,
        Options {
            dry_run: false,
            debug_log: false,
            mode: OptimizationMode::SingleMod,
            user_path: String::new(),
            bsa_extract: false,
            bsa_create: false,
            bsa_delete_backup: false,
            // Not in the shipped file. C++ reads a missing key as `false`, not as its
            // member default `true`, and the port keeps that.
            bsa_merge_incompressible: false,
            bsa_merge_textures: false,
            bsa_process_content: false,
            bsa_create_dummies: true,
            bsa_compress: true,
            bsa_delete_source: true,
            textures_necessary: true,
            textures_compress: false,
            textures_mipmaps: false,
            textures_resize_size: false,
            textures_target_width: 2048,
            textures_target_height: 2048,
            textures_resize_ratio: false,
            textures_target_width_ratio: 2,
            textures_target_height_ratio: 2,
            meshes_optimization_level: 0,
            meshes_headparts: true,
            meshes_resave: false,
            animations_optimization: false,
        }
    );
}

#[test]
fn a_first_load_without_settings_ini_has_the_default_options() {
    // C++ keeps its member defaults only when the file does not exist at all.
    let app_dir = common::scratch_dir("options-without-settings-ini");
    let options = Profiles::new(&app_dir)
        .open("Bare")
        .load_options(&Options::default())
        .unwrap();

    assert_eq!(options, Options::default());
    assert!(options.bsa_merge_incompressible);
    assert_eq!(options.textures_target_width_ratio, 1);
    assert_eq!(options.mode, OptimizationMode::SingleMod);
}

#[test]
fn switching_to_a_profile_without_settings_ini_keeps_the_current_options() {
    // C++ read settings.ini into the GUI's one live OptionsCAO and returned early
    // when the file was missing, so the previous profile's options carried over.
    let app_dir = common::scratch_dir("options-switch-without-settings-ini");
    let current = Options {
        dry_run: true,
        user_path: "D:/Mods".to_owned(),
        meshes_optimization_level: 3,
        ..Options::default()
    };

    let options = Profiles::new(&app_dir)
        .open("Bare")
        .load_options(&current)
        .unwrap();

    assert_eq!(options, current);
}

#[test]
fn an_empty_user_path_keeps_the_current_one_when_switching_profiles() {
    // Every shipped settings.ini has `userPath=`, so in C++ the selected folder
    // follows the user from one profile to the next. Every other key is replaced.
    let current = Options {
        user_path: "D:/Mods".to_owned(),
        dry_run: true,
        ..Options::default()
    };

    let options = shipped().open("FO4").load_options(&current).unwrap();

    assert_eq!(options.user_path, "D:/Mods");
    assert!(!options.dry_run);
    assert!(!options.meshes_headparts);
}

#[test]
fn a_saved_user_path_replaces_the_current_one() {
    let app_dir = common::copy_of_shipped("options-saved-user-path");
    let profile = Profiles::new(&app_dir).open("TES5");
    let saved = Options {
        user_path: "E:/Other".to_owned(),
        ..Options::default()
    };
    profile.save_options(&saved).unwrap();
    let current = Options {
        user_path: "D:/Mods".to_owned(),
        ..Options::default()
    };

    assert_eq!(
        profile.load_options(&current).unwrap().user_path,
        "E:/Other"
    );
}

#[test]
fn saving_an_unchanged_profile_ini_rewrites_the_shipped_bytes_but_the_fractional_byte() {
    // C++ writes the same keys back; `animationFormat` is dead data that neither
    // build reads or writes, and it stays (deviation 10). The maximum Archive
    // size is whole bytes, so its fraction is the one change.
    for (name, shipped_size, saved_size) in [
        (
            "FO4",
            "maxBsaUncompressedSize=4187593113.6\r\n",
            "maxBsaUncompressedSize=4187593113\r\n",
        ),
        (
            "SSE",
            "maxBsaUncompressedSize=2104533975.04\r\n",
            "maxBsaUncompressedSize=2104533975\r\n",
        ),
        (
            "TES5",
            "maxBsaUncompressedSize=2104533975.04\r\n",
            "maxBsaUncompressedSize=2104533975\r\n",
        ),
    ] {
        let app_dir = common::copy_of_shipped(&format!("save-settings-{name}"));
        let profile = Profiles::new(&app_dir).open(name);
        let original = String::from_utf8(common::read(&profile.profile_ini())).unwrap();
        assert_eq!(original.matches(shipped_size).count(), 1, "{name}");
        let expected = original.replace(shipped_size, saved_size);
        let loaded = profile.load_settings().unwrap();

        profile.save_settings(&loaded).unwrap();

        let saved = common::read(&profile.profile_ini());
        assert!(
            saved == expected.as_bytes(),
            "{name}:\n{}",
            common::show(&saved)
        );
        assert_eq!(profile.load_settings().unwrap(), loaded, "{name} reloads");
    }
}

#[test]
fn saving_options_writes_what_qt_writes_and_keeps_dead_keys() {
    // Qt appends the two keys the shipped file lacks to the end of `[BSA]`, and
    // leaves `bBsaLeastBSA`, which nothing reads, where it was (deviation 10).
    let app_dir = common::copy_of_shipped("save-options-sse");
    let profile = Profiles::new(&app_dir).open("SSE");
    let mut options = profile.load_options(&Options::default()).unwrap();
    options.mode = OptimizationMode::SeveralMods;
    options.user_path = "D:/Mods/Café; v2".to_owned();
    options.meshes_optimization_level = 2;

    profile.save_options(&options).unwrap();

    let expected = concat!(
        "[General]\r\n",
        "bDryRun=false\r\n",
        "bDebugLog=false\r\n",
        "mode=1\r\n",
        r#"userPath="D:/Mods/Caf\xe9; v2""#,
        "\r\n",
        "\r\n",
        "[BSA]\r\n",
        "bBsaExtract=false\r\n",
        "bBsaCreate=false\r\n",
        "bBsaDeleteBackup=false\r\n",
        "bBsaLeastBSA=false\r\n",
        "bBsaProcessContent=false\r\n",
        "bBsaCreateDummies=true\r\n",
        "bBsaCompress=true\r\n",
        "bBsaDeleteSource=true\r\n",
        "bBsaMergeIncomp=false\r\n",
        "bBsaMergeTexture=false\r\n",
        "\r\n",
        "[Textures]\r\n",
        "bTexturesNecessary=true\r\n",
        "bTexturesCompress=false\r\n",
        "bTexturesMipmaps=false\r\n",
        "bTexturesResizeSize=false\r\n",
        "iTexturesTargetWidth=2048\r\n",
        "iTexturesTargetHeight=2048\r\n",
        "bTexturesResizeRatio=false\r\n",
        "iTexturesTargetHeightRatio=2\r\n",
        "iTexturesTargetWidthRatio=2\r\n",
        "\r\n",
        "[Meshes]\r\n",
        "iMeshesOptimizationLevel=2\r\n",
        "bMeshesHeadparts=true\r\n",
        "bMeshesResave=false\r\n",
        "\r\n",
        "[Animations]\r\n",
        "bAnimationsOptimization=false\r\n",
    );
    let saved = common::read(&profile.settings_ini());
    assert!(saved == expected.as_bytes(), "{}", common::show(&saved));
    assert_eq!(profile.load_options(&Options::default()).unwrap(), options);
}

#[test]
fn saved_settings_load_back_including_a_one_element_unwanted_format_list() {
    let app_dir = common::copy_of_shipped("save-settings-round-trip");
    let profile = Profiles::new(&app_dir).open("FO4");
    let mut settings = profile.load_settings().unwrap();
    settings.textures_unwanted_formats = vec![UnwantedFormat::B5G6R5Unorm];
    settings.max_bsa_uncompressed_size = ArchiveSize::from_bytes(2_147_483_648);
    settings.meshes_enabled = true;

    profile.save_settings(&settings).unwrap();

    assert_eq!(profile.load_settings().unwrap(), settings);
    let saved = String::from_utf8(common::read(&profile.profile_ini())).unwrap();
    // Qt writes a one-element list as a QDataStream blob, which the C++ build reads.
    assert!(
        saved.contains(r"texturesUnwantedFormats=@Variant(\0\0\0\t\0\0\0\x1\0\0\0\x2\0\0\0U)"),
        "{saved}"
    );
    assert!(
        saved.contains("maxBsaUncompressedSize=2147483648\r\n"),
        "{saved}"
    );
    assert!(saved.contains("animationFormat=3\r\n"), "{saved}");
}

#[test]
fn saving_options_creates_a_missing_settings_ini() {
    let app_dir = common::scratch_dir("save-options-new-file");
    let profile = Profiles::new(&app_dir).open("Fresh");

    profile.save_options(&Options::default()).unwrap();

    assert_eq!(
        profile.load_options(&Options::default()).unwrap(),
        Options::default()
    );
}

#[test]
fn a_profile_without_profile_ini_is_unavailable() {
    let app_dir = common::scratch_dir("settings-unavailable");
    let profile = Profiles::new(&app_dir).open("Gone");

    assert!(matches!(
        profile.load_settings(),
        Err(ProfileError::Unavailable { path }) if path == profile.profile_ini()
    ));
}

/// Appends `line` to `profile`'s `profile.ini`, keeping every byte before it.
fn append_line(profile: &cao_profiles::Profile, line: &[u8]) {
    let mut bytes = common::read(&profile.profile_ini());
    bytes.extend_from_slice(line);
    std::fs::write(profile.profile_ini(), &bytes).unwrap();
}

#[test]
fn a_format_error_makes_the_profile_unreadable() {
    // The GUI and Preparing share this read, so a line a save would drop is reported
    // when the Profile is selected rather than silently lost.
    let app_dir = common::copy_of_shipped("settings-format-error");
    let profile = Profiles::new(&app_dir).open("SSE");
    append_line(&profile, b"stray words\r\n");

    let error = profile.load_settings().unwrap_err();
    assert!(
        matches!(
            &error,
            ProfileError::Malformed { path, error }
                if *path == profile.profile_ini()
                    && error.kind == FormatErrorKind::MissingEquals
        ),
        "{error:?}"
    );
}

#[test]
fn a_format_error_is_reported_before_an_unsupported_bsa_game() {
    let app_dir = common::copy_of_shipped("settings-format-error-first");
    let profile = Profiles::new(&app_dir).open("SSE");
    let text = String::from_utf8(common::read(&profile.profile_ini())).unwrap();
    let text = text.replace("bsaGame=4", "bsaGame=99");
    assert!(text.contains("bsaGame=99"));
    std::fs::write(profile.profile_ini(), text).unwrap();
    append_line(&profile, b"stray words\r\n");

    let error = profile.load_settings().unwrap_err();
    assert!(
        matches!(&error, ProfileError::Malformed { .. }),
        "{error:?}"
    );
}

#[test]
fn a_format_error_is_reported_before_an_out_of_set_mesh_or_texture_value() {
    for (index, (from, to)) in [
        ("meshesUser=12", "meshesUser=13"),
        ("texturesFormat=98", "texturesFormat=99"),
        (
            "texturesUnwantedFormats=85, 86, 115",
            "texturesUnwantedFormats=85, 1",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let app_dir = common::copy_of_shipped(&format!("settings-format-error-first-{index}"));
        let profile = Profiles::new(&app_dir).open("SSE");
        let text = String::from_utf8(common::read(&profile.profile_ini())).unwrap();
        assert!(text.contains(from), "{from}");
        std::fs::write(profile.profile_ini(), text.replace(from, to)).unwrap();
        append_line(&profile, b"stray words\r\n");

        let error = profile.load_settings().unwrap_err();
        assert!(
            matches!(&error, ProfileError::Malformed { .. }),
            "{to}: {error:?}"
        );
    }
}

#[test]
fn a_missing_capability_or_texture_switch_loads_as_false() {
    // Each bool key is checked on its own: removing it from the shipped SSE
    // Profile, where every one of them is `true`, turns only that field off.
    type Switch = fn(&mut ProfileSettings) -> &mut bool;
    let switches: [(&str, Switch); 6] = [
        ("bsaEnabled", |settings| &mut settings.bsa_enabled),
        ("meshesEnabled", |settings| &mut settings.meshes_enabled),
        ("animationsEnabled", |settings| {
            &mut settings.animations_enabled
        }),
        ("texturesEnabled", |settings| &mut settings.textures_enabled),
        ("texturesConvertTga", |settings| {
            &mut settings.textures_convert_tga
        }),
        ("texturesCompressInterface", |settings| {
            &mut settings.textures_compress_interface
        }),
    ];
    for (key, field) in switches {
        let app_dir = common::copy_of_shipped(&format!("settings-missing-{key}"));
        let profile = Profiles::new(&app_dir).open("SSE");
        let mut expected = profile.load_settings().unwrap();
        assert!(*field(&mut expected), "SSE ships {key}=true");
        *field(&mut expected) = false;
        let text = String::from_utf8(common::read(&profile.profile_ini())).unwrap();
        let prefix = format!("{key}=");
        let kept: Vec<&str> = text
            .split("\r\n")
            .filter(|line| !line.starts_with(&prefix))
            .collect();
        assert_ne!(
            kept.len(),
            text.split("\r\n").count(),
            "{key} is in the file"
        );
        std::fs::write(profile.profile_ini(), kept.join("\r\n")).unwrap();

        assert_eq!(profile.load_settings().unwrap(), expected, "{key}");
    }
}

#[test]
fn every_shipped_profile_loads_cleanly() {
    let profiles = shipped();
    let sse_options = profiles
        .open("SSE")
        .load_options(&Options::default())
        .unwrap();

    for name in profiles.list() {
        let profile = profiles.open(&name);
        profile.load_settings().unwrap();
        let options = profile.load_options(&Options::default()).unwrap();
        // The shipped settings.ini files differ only in FO4's headpart choice.
        let expected = Options {
            meshes_headparts: name != "FO4",
            ..sse_options.clone()
        };
        assert_eq!(options, expected, "{name}");
    }
}

#[test]
fn the_shipped_common_ini_loads_into_the_common_settings_model() {
    assert_eq!(
        shipped().load_common().unwrap(),
        CommonSettings {
            profile: "TES5".to_owned(),
            show_advanced_settings: true,
            dark_mode: true,
            show_tutorial: true,
            not_first_start: true,
        }
    );
}

#[test]
fn a_missing_common_ini_is_dark_shows_tutorials_and_is_a_first_start() {
    // C++ reads `showTutorial` with an explicit default of `true`, and the port is
    // dark by default (spec #476), so a missing `bDarkMode` reads as `true`; every
    // other key reads as `false` or empty.
    let app_dir = common::scratch_dir("common-missing");

    assert_eq!(
        Profiles::new(&app_dir).load_common().unwrap(),
        CommonSettings {
            profile: String::new(),
            show_advanced_settings: false,
            dark_mode: true,
            show_tutorial: true,
            not_first_start: false,
        }
    );
}

#[test]
fn an_explicit_light_theme_choice_is_honoured() {
    let ini = IniFile::parse(b"[General]\r\nbDarkMode=false\r\n");

    assert!(!CommonSettings::read(&ini).dark_mode);
}

#[test]
fn a_common_ini_without_a_theme_key_is_dark() {
    let ini = IniFile::parse(b"[General]\r\nprofile=SSE\r\n");

    assert!(CommonSettings::read(&ini).dark_mode);
}

#[test]
fn saving_common_settings_keeps_unknown_keys() {
    let app_dir = common::copy_of_shipped("common-save");
    let profiles = Profiles::new(&app_dir);
    let path = profiles.root().join("common.ini");
    let mut bytes = common::read(&path);
    bytes.extend_from_slice(b"futureKey=42\r\n");
    std::fs::write(&path, &bytes).unwrap();
    let mut settings = profiles.load_common().unwrap();
    settings.profile = "FO4".to_owned();
    settings.dark_mode = true;

    profiles.save_common(&settings).unwrap();

    let expected = concat!(
        "[General]\r\n",
        "profile=FO4\r\n",
        "bShowAdvancedSettings=true\r\n",
        "bDarkMode=true\r\n",
        "showTutorial=true\r\n",
        "notFirstStart=true\r\n",
        "futureKey=42\r\n",
    );
    let saved = common::read(&path);
    assert!(saved == expected.as_bytes(), "{}", common::show(&saved));
}

#[test]
fn a_remembered_profile_that_does_not_exist_resolves_to_sse() {
    let profiles = shipped();

    assert_eq!(profiles.resolve("FO4"), "FO4");
    assert_eq!(profiles.resolve("TES4"), "SSE");
    assert_eq!(profiles.resolve("fo4"), "SSE");
    assert_eq!(profiles.resolve(""), "SSE");
}

#[test]
fn a_clean_profile_reads_without_error() {
    let settings = shipped().open("FO4").load_settings().unwrap();

    assert_eq!(settings.bsa_game, Game::Fo4);
    assert_eq!(
        settings.textures_unwanted_formats,
        [
            UnwantedFormat::B5G5R5A1Unorm,
            UnwantedFormat::B5G6R5Unorm,
            UnwantedFormat::B4G4R4A4Unorm,
        ]
    );
    assert_eq!(
        settings.max_bsa_uncompressed_size,
        ArchiveSize::from_bytes(4_187_593_113)
    );
}
