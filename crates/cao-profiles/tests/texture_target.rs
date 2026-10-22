//! A Profile's Texture Target: the format it compresses Textures to, and the
//! formats it converts Textures away from. Both are closed sets, read and written
//! on their own.

mod common;

use cao_profiles::{ClosedSet, Profile, ProfileError, Profiles, TextureFormat, UnwantedFormat};

/// A copy of the shipped SSE Profile in a fresh app directory named `name`.
fn sse_copy(name: &str) -> Profile {
    Profiles::new(&common::copy_of_shipped(name)).open("SSE")
}

/// Replaces the line starting `key=` in `profile`'s `profile.ini` with `line`, or
/// drops it when `line` is empty, keeping every other byte.
fn replace_line(profile: &Profile, key: &str, line: &str) {
    let text = String::from_utf8(common::read(&profile.profile_ini())).unwrap();
    let prefix = format!("{key}=");
    assert!(
        text.lines().any(|existing| existing.starts_with(&prefix)),
        "{key} is in the file"
    );
    let replaced: Vec<&str> = text
        .split("\r\n")
        .filter_map(|existing| {
            if !existing.starts_with(&prefix) {
                Some(existing)
            } else if line.is_empty() {
                None
            } else {
                Some(line)
            }
        })
        .collect();
    std::fs::write(profile.profile_ini(), replaced.join("\r\n")).unwrap();
}

/// The line `profile.ini` holds for `key`, such as `texturesFormat=98`.
fn saved_line(profile: &Profile, key: &str) -> String {
    let text = String::from_utf8(common::read(&profile.profile_ini())).unwrap();
    let prefix = format!("{key}=");
    text.lines()
        .find(|line| line.starts_with(&prefix))
        .unwrap_or_else(|| panic!("{key} is saved:\n{text}"))
        .to_owned()
}

#[test]
fn a_texture_format_outside_its_set_is_reported_with_its_key_value_and_accepted_values() {
    let profile = sse_copy("texture-format-99");
    replace_line(&profile, "texturesFormat", "texturesFormat=99");

    let error = profile.load_settings().unwrap_err();

    assert!(
        matches!(
            &error,
            ProfileError::UnsupportedValue { file, key, value, .. }
                if file == "profile.ini"
                    && key == "Textures/texturesFormat"
                    && value.as_deref() == Some("99")
        ),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "`profile.ini`: `Textures/texturesFormat` is `99`; expected one of 98 (BC7_UNORM), \
         83 (BC5_UNORM), 77 (BC3_UNORM), 71 (BC1_UNORM), 28 (R8G8B8A8_UNORM)"
    );
}

#[test]
fn a_missing_texture_format_makes_the_profile_unreadable() {
    let profile = sse_copy("texture-format-missing");
    replace_line(&profile, "texturesFormat", "");

    let error = profile.load_settings().unwrap_err();

    assert!(
        matches!(&error, ProfileError::UnsupportedValue { value: None, .. }),
        "{error:?}"
    );
    assert!(
        error
            .to_string()
            .starts_with("`profile.ini`: `Textures/texturesFormat` is missing; expected one of"),
        "{error}"
    );
}

#[test]
fn an_unwanted_format_outside_its_set_is_reported_with_its_key_value_and_accepted_values() {
    // 1 is `DXGI_FORMAT_R32G32B32A32_TYPELESS`, which the dialog never offered.
    let profile = sse_copy("unwanted-format-outside");
    replace_line(
        &profile,
        "texturesUnwantedFormats",
        "texturesUnwantedFormats=85, 1, 115",
    );

    let error = profile.load_settings().unwrap_err();

    assert!(
        matches!(
            &error,
            ProfileError::UnsupportedValue { file, key, value, .. }
                if file == "profile.ini"
                    && key == "Textures/texturesUnwantedFormats"
                    && value.as_deref() == Some("1")
        ),
        "{error:?}"
    );
    let message = error.to_string();
    assert!(
        message.starts_with(
            "`profile.ini`: `Textures/texturesUnwantedFormats` is `1`; expected each entry \
             to be one of 2 (R32G32B32A32_FLOAT), 3 (R32G32B32A32_UINT), "
        ),
        "{message}"
    );
    assert!(
        message.ends_with(", 109 (Y216), 115 (B4G4R4A4_UNORM)"),
        "{message}"
    );
}

#[test]
fn repeated_unwanted_formats_load_once_each_in_first_occurrence_order() {
    let profile = sse_copy("unwanted-format-repeats");
    replace_line(
        &profile,
        "texturesUnwantedFormats",
        "texturesUnwantedFormats=86, 85, 86, 115, 85",
    );

    let settings = profile.load_settings().unwrap();

    assert_eq!(
        settings.textures_unwanted_formats,
        [
            UnwantedFormat::B5G5R5A1Unorm,
            UnwantedFormat::B5G6R5Unorm,
            UnwantedFormat::B4G4R4A4Unorm,
        ]
    );
}

#[test]
fn a_missing_unwanted_format_list_reads_as_empty() {
    let profile = sse_copy("unwanted-format-missing");
    replace_line(&profile, "texturesUnwantedFormats", "");

    assert!(
        profile
            .load_settings()
            .unwrap()
            .textures_unwanted_formats
            .is_empty()
    );
}

#[test]
fn every_accepted_texture_format_round_trips_as_the_number_the_file_holds() {
    let expected = [
        (TextureFormat::Bc7Unorm, "98"),
        (TextureFormat::Bc5Unorm, "83"),
        (TextureFormat::Bc3Unorm, "77"),
        (TextureFormat::Bc1Unorm, "71"),
        (TextureFormat::R8G8B8A8Unorm, "28"),
    ];
    assert_eq!(TextureFormat::ALL.len(), expected.len());
    for (format, number) in expected {
        let profile = sse_copy(&format!("texture-format-{number}"));
        let mut edited = profile.load_settings().unwrap();
        edited.textures_format = format;

        profile.save_settings(&edited).unwrap();

        assert_eq!(profile.load_settings().unwrap(), edited, "{number}");
        assert_eq!(
            saved_line(&profile, "texturesFormat"),
            format!("texturesFormat={number}")
        );
    }
}

#[test]
fn every_accepted_unwanted_format_round_trips_as_the_number_the_file_holds() {
    let profile = sse_copy("unwanted-format-all");
    let mut edited = profile.load_settings().unwrap();
    // Reversed, so the saved order is the user's and not the set's.
    edited.textures_unwanted_formats = UnwantedFormat::ALL.iter().rev().copied().collect();

    profile.save_settings(&edited).unwrap();

    assert_eq!(profile.load_settings().unwrap(), edited);
    assert_eq!(UnwantedFormat::ALL.len(), 75);
    // The numbers the file has always held: every `DXGI_FORMAT` from 2 to 115
    // that is neither `_TYPELESS`, depth/stencil, `R1_UNORM`, nor a planar or
    // palette format, highest first.
    let line = saved_line(&profile, "texturesUnwantedFormats");
    assert_eq!(
        line,
        "texturesUnwantedFormats=115, 109, 108, 107, 102, 101, 100, 99, 98, 96, 95, 93, \
         91, 89, 88, 87, 86, 85, 84, 83, 81, 80, 78, 77, 75, 74, 72, 71, 69, 68, 67, 65, \
         64, 63, 62, 61, 59, 58, 57, 56, 54, 52, 51, 50, 49, 43, 42, 41, 38, 37, 36, 35, \
         34, 32, 31, 30, 29, 28, 26, 25, 24, 18, 17, 16, 14, 13, 12, 11, 10, 8, 7, 6, 4, \
         3, 2"
    );
}

#[test]
fn every_unwanted_format_has_the_name_the_window_shows() {
    let named: Vec<(&str, u32)> = UnwantedFormat::ALL
        .iter()
        .map(|&format| (format.name(), format.number()))
        .collect();

    assert_eq!(named.first(), Some(&("R32G32B32A32_FLOAT", 2)));
    assert_eq!(named.last(), Some(&("B4G4R4A4_UNORM", 115)));
    assert_eq!(
        UnwantedFormat::from_name("BC7_UNORM_SRGB"),
        Some(UnwantedFormat::Bc7UnormSrgb)
    );
    // Exact, as the window has always matched.
    assert_eq!(UnwantedFormat::from_name("bc7_unorm_srgb"), None);
    assert_eq!(UnwantedFormat::from_name("DXGI_FORMAT_UNKNOWN"), None);
}
