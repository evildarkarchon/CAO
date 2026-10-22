//! A Profile's maximum Archive size (`[BSA] maxBsaUncompressedSize`): a whole
//! number of bytes, read strictly and written as integer text.

mod common;

use cao_profiles::{ArchiveSize, Profile, ProfileError, Profiles};

/// A copy of the shipped `name` Profile in a fresh app directory named `dir`.
fn shipped_copy(dir: &str, name: &str) -> Profile {
    Profiles::new(&common::copy_of_shipped(dir)).open(name)
}

/// Replaces the `maxBsaUncompressedSize=` line in `profile`'s `profile.ini` with
/// `line`, or drops it when `line` is empty, keeping every other byte.
fn replace_size_line(profile: &Profile, line: &str) {
    let text = String::from_utf8(common::read(&profile.profile_ini())).unwrap();
    let prefix = "maxBsaUncompressedSize=";
    assert!(
        text.lines().any(|existing| existing.starts_with(prefix)),
        "the size is in the file"
    );
    let replaced: Vec<&str> = text
        .split("\r\n")
        .filter_map(|existing| {
            if !existing.starts_with(prefix) {
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

#[test]
fn a_fractional_size_loads_truncated_to_whole_bytes() {
    let profile = shipped_copy("archive-size-fraction", "SSE");
    replace_size_line(&profile, "maxBsaUncompressedSize=2104533975.99");

    let settings = profile.load_settings().unwrap();

    assert_eq!(
        settings.max_bsa_uncompressed_size,
        ArchiveSize::from_bytes(2_104_533_975)
    );
    assert_eq!(settings.max_bsa_uncompressed_size.bytes(), 2_104_533_975);
}

#[test]
fn a_missing_size_reads_as_zero_so_the_games_table_maximum_applies() {
    let profile = shipped_copy("archive-size-missing", "SSE");
    replace_size_line(&profile, "");

    let settings = profile.load_settings().unwrap();

    assert_eq!(
        settings.max_bsa_uncompressed_size,
        ArchiveSize::from_bytes(0)
    );
}

#[test]
fn a_size_that_is_not_a_number_is_reported_rather_than_read_as_zero() {
    for value in ["", "2GB", "two"] {
        let profile = shipped_copy(&format!("archive-size-text-{value}"), "SSE");
        replace_size_line(&profile, &format!("maxBsaUncompressedSize={value}"));

        let error = profile.load_settings().unwrap_err();

        assert_eq!(
            error.to_string(),
            format!(
                "`profile.ini`: `BSA/maxBsaUncompressedSize` is `{value}`; expected a whole \
                 number of bytes from 0 to 18446744073709551615"
            )
        );
    }
}

#[test]
fn a_size_saves_as_whole_byte_integer_text_and_reloads_exactly() {
    // Qt's double writer would store 4 TB as `4e+12`, and a double cannot hold
    // the largest size at all.
    for (bytes, line) in [
        (0, "maxBsaUncompressedSize=0"),
        (4_000_000_000_000, "maxBsaUncompressedSize=4000000000000"),
        (u64::MAX, "maxBsaUncompressedSize=18446744073709551615"),
    ] {
        let profile = shipped_copy(&format!("archive-size-saved-{bytes}"), "SSE");
        let mut settings = profile.load_settings().unwrap();
        settings.max_bsa_uncompressed_size = ArchiveSize::from_bytes(bytes);

        profile.save_settings(&settings).unwrap();

        let saved = String::from_utf8(common::read(&profile.profile_ini())).unwrap();
        assert!(saved.contains(&format!("{line}\r\n")), "{saved}");
        assert_eq!(profile.load_settings().unwrap(), settings, "{bytes}");
    }
}

#[test]
fn a_negative_non_finite_or_overflowing_size_is_reported_with_its_key_value_and_accepted_values() {
    // 2^64 is one byte past the largest size; 1e400 overflows even a double.
    for value in [
        "-1",
        "-0.5",
        "inf",
        "-inf",
        "nan",
        "1e400",
        "18446744073709551616",
    ] {
        let profile = shipped_copy(&format!("archive-size-rejected-{value}"), "SSE");
        replace_size_line(&profile, &format!("maxBsaUncompressedSize={value}"));

        let error = profile.load_settings().unwrap_err();

        assert!(
            matches!(
                &error,
                ProfileError::UnsupportedValue { file, key, value: found, .. }
                    if file == "profile.ini"
                        && key == "BSA/maxBsaUncompressedSize"
                        && found.as_deref() == Some(value)
            ),
            "{value}: {error:?}"
        );
        assert_eq!(
            error.to_string(),
            format!(
                "`profile.ini`: `BSA/maxBsaUncompressedSize` is `{value}`; expected a whole \
                 number of bytes from 0 to 18446744073709551615"
            )
        );
    }
}
