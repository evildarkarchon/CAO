//! A Profile's Mesh Target: the NIF file, user and stream version it writes Meshes
//! as. Each axis is a closed set, read and written on its own.

mod common;

use cao_profiles::{
    ClosedSet, MeshFileVersion, MeshStreamVersion, MeshUserVersion, Profile, ProfileError,
    ProfileSettings, Profiles,
};

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

/// The line `profile.ini` holds for `key`, such as `meshesUser=12`.
fn saved_line(profile: &Profile, key: &str) -> String {
    let text = String::from_utf8(common::read(&profile.profile_ini())).unwrap();
    let prefix = format!("{key}=");
    text.lines()
        .find(|line| line.starts_with(&prefix))
        .unwrap_or_else(|| panic!("{key} is saved:\n{text}"))
        .to_owned()
}

/// Saves `settings` into a fresh SSE copy and reloads them, checking the key holds
/// `number` as the file always has.
fn round_trip(name: &str, settings: impl Fn(&mut ProfileSettings), key: &str, number: &str) {
    let profile = sse_copy(name);
    let mut edited = profile.load_settings().unwrap();
    settings(&mut edited);

    profile.save_settings(&edited).unwrap();

    assert_eq!(profile.load_settings().unwrap(), edited, "{name}");
    assert_eq!(saved_line(&profile, key), format!("{key}={number}"));
}

#[test]
fn a_mesh_value_just_outside_its_set_is_reported_with_its_key_value_and_accepted_values() {
    for (key, line, message) in [
        (
            "meshesUser",
            "meshesUser=13",
            "`profile.ini`: `Meshes/meshesUser` is `13`; expected one of 11, 12",
        ),
        (
            "meshesStream",
            "meshesStream=84",
            "`profile.ini`: `Meshes/meshesStream` is `84`; expected one of 82, 83, 100, 130",
        ),
        (
            "meshesFileVersion",
            "meshesFileVersion=335675400",
            "`profile.ini`: `Meshes/meshesFileVersion` is `335675400`; expected one of \
             335544325 (20.0.0.5), 335675399 (20.2.0.7)",
        ),
    ] {
        let profile = sse_copy(&format!("mesh-outside-{key}"));
        replace_line(&profile, key, line);

        let error = profile.load_settings().unwrap_err();

        assert!(
            matches!(
                &error,
                ProfileError::UnsupportedValue { file, key: found, value, .. }
                    if file == "profile.ini"
                        && *found == format!("Meshes/{key}")
                        && value.as_deref() == line.split_once('=').map(|(_, value)| value)
            ),
            "{error:?}"
        );
        assert_eq!(error.to_string(), message);
    }
}

#[test]
fn a_mesh_value_that_only_truncates_into_its_set_is_rejected() {
    // 2^32 + 12 is 12 after Qt's `toInt()`, but it is not the number 12.
    let profile = sse_copy("mesh-user-wraps");
    replace_line(&profile, "meshesUser", "meshesUser=4294967308");

    let error = profile.load_settings().unwrap_err();

    assert!(
        matches!(&error, ProfileError::UnsupportedValue { .. }),
        "{error:?}"
    );
}

#[test]
fn a_missing_mesh_key_makes_the_profile_unreadable() {
    for key in ["meshesFileVersion", "meshesUser", "meshesStream"] {
        let profile = sse_copy(&format!("mesh-missing-{key}"));
        replace_line(&profile, key, "");

        let error = profile.load_settings().unwrap_err();

        assert!(
            matches!(&error, ProfileError::UnsupportedValue { value: None, .. }),
            "{key}: {error:?}"
        );
        assert!(
            error.to_string().starts_with(&format!(
                "`profile.ini`: `Meshes/{key}` is missing; expected one of"
            )),
            "{error}"
        );
    }
}

#[test]
fn every_accepted_mesh_file_version_round_trips_as_the_number_the_file_holds() {
    let expected = [
        (MeshFileVersion::V20_0_0_5, "335544325"),
        (MeshFileVersion::V20_2_0_7, "335675399"),
    ];
    assert_eq!(MeshFileVersion::ALL.len(), expected.len());
    for (version, number) in expected {
        round_trip(
            &format!("mesh-file-version-{number}"),
            |settings| settings.meshes_file_version = version,
            "meshesFileVersion",
            number,
        );
    }
}

#[test]
fn every_accepted_mesh_user_version_round_trips_as_the_number_the_file_holds() {
    let expected = [(MeshUserVersion::V11, "11"), (MeshUserVersion::V12, "12")];
    assert_eq!(MeshUserVersion::ALL.len(), expected.len());
    for (user, number) in expected {
        round_trip(
            &format!("mesh-user-{number}"),
            |settings| settings.meshes_user = user,
            "meshesUser",
            number,
        );
    }
}

#[test]
fn every_accepted_mesh_stream_version_round_trips_as_the_number_the_file_holds() {
    let expected = [
        (MeshStreamVersion::V82, "82"),
        (MeshStreamVersion::V83, "83"),
        (MeshStreamVersion::V100, "100"),
        (MeshStreamVersion::V130, "130"),
    ];
    assert_eq!(MeshStreamVersion::ALL.len(), expected.len());
    for (stream, number) in expected {
        round_trip(
            &format!("mesh-stream-{number}"),
            |settings| settings.meshes_stream = stream,
            "meshesStream",
            number,
        );
    }
}
