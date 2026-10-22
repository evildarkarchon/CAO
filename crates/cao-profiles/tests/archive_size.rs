//! A Profile's maximum Archive size (`[BSA] maxBsaUncompressedSize`): a whole
//! number of bytes, read strictly from bytes or a size with a unit, and written
//! as integer text unless the user's text still holds it; or not given, when
//! the key is missing, and then still missing after a save.

mod common;

use cao_profiles::{ArchiveSize, Profile, ProfileError, Profiles};
use common::size;

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

    assert_eq!(settings.max_bsa_uncompressed_size, size(2_104_533_975));
    assert_eq!(
        settings.max_bsa_uncompressed_size.requested(),
        Some(2_104_533_975)
    );
}

#[test]
fn a_missing_size_loads_as_not_given_and_saves_without_the_key() {
    let profile = shipped_copy("archive-size-missing", "SSE");
    replace_size_line(&profile, "");
    let before = common::read(&profile.profile_ini());

    let settings = profile.load_settings().unwrap();

    // Not given, so the game's default applies; nothing asks for a size.
    assert_eq!(settings.max_bsa_uncompressed_size, ArchiveSize::NOT_GIVEN);
    assert_eq!(settings.max_bsa_uncompressed_size.requested(), None);

    // A save leaves the key out, so the Profile keeps following the default.
    profile.save_settings(&settings).unwrap();
    let saved = String::from_utf8(common::read(&profile.profile_ini())).unwrap();
    assert!(!saved.contains("maxBsaUncompressedSize"), "{saved}");
    assert_eq!(saved, String::from_utf8(before).unwrap());
    assert_eq!(profile.load_settings().unwrap(), settings);
}

#[test]
fn a_size_set_to_not_given_saves_by_removing_the_key() {
    let profile = shipped_copy("archive-size-cleared", "SSE");
    let mut settings = profile.load_settings().unwrap();
    assert_ne!(settings.max_bsa_uncompressed_size, ArchiveSize::NOT_GIVEN);
    settings.max_bsa_uncompressed_size = ArchiveSize::NOT_GIVEN;

    profile.save_settings(&settings).unwrap();

    let saved = String::from_utf8(common::read(&profile.profile_ini())).unwrap();
    assert!(!saved.contains("maxBsaUncompressedSize"), "{saved}");
    // The rest of `[BSA]` is kept, in its order.
    assert!(
        saved.contains("[BSA]\r\nbsaEnabled=true\r\nbsaGame=4\r\n"),
        "{saved}"
    );
    assert_eq!(profile.load_settings().unwrap(), settings);
}

#[test]
fn an_explicit_zero_size_makes_the_profile_unreadable() {
    // A limit of 0 could never pack anything, so it is reported rather than
    // silently replaced by the game's default; a size whose whole bytes
    // truncate to 0 is the same 0.
    for value in ["0", "0.0", "0.5", "0GB", "0.1B"] {
        let profile = shipped_copy(&format!("archive-size-zero-{value}"), "SSE");
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
                "`profile.ini`: `BSA/maxBsaUncompressedSize` is `{value}`; expected {ACCEPTED}"
            )
        );
    }
}

/// What a rejected size's error says the key accepts.
const ACCEPTED: &str = "a whole number of bytes from 1 to 18446744073709551615, or a size \
                        such as `8GB` in B, KB, MB, GB or TB, counted in 1024s; leave the \
                        key out for the game's default";

#[test]
fn a_size_with_a_unit_loads_counted_in_1024s() {
    // A copied Profile may raise its Archives past the game's table maximum,
    // such as FO4 Archives past the Creation Kit's 4 GB, so a hand-edited size
    // can say so in units. Case and the space before the unit don't matter,
    // the IEC spellings mean the same, and a fraction of a byte is truncated.
    for (value, bytes) in [
        ("8GB", 8_589_934_592),
        ("8 gb", 8_589_934_592),
        ("4.5GB", 4_831_838_208),
        ("500MB", 524_288_000),
        ("500MiB", 524_288_000),
        ("2TB", 2_199_023_255_552),
        ("1.5KB", 1_536),
        ("1023B", 1_023),
        ("0.1KB", 102),
        // The largest whole number of terabytes a size holds, 2^64 - 2^40.
        ("16777215TB", 18_446_742_974_197_923_840),
    ] {
        let profile = shipped_copy(&format!("archive-size-unit-{value}"), "SSE");
        replace_size_line(&profile, &format!("maxBsaUncompressedSize={value}"));

        let settings = profile.load_settings().unwrap();

        assert_eq!(
            settings.max_bsa_uncompressed_size.requested(),
            Some(bytes),
            "{value}"
        );
    }
}

#[test]
fn a_size_with_a_unit_saves_as_written_until_it_changes() {
    let profile = shipped_copy("archive-size-unit-kept", "FO4");
    replace_size_line(&profile, "maxBsaUncompressedSize=8 GB");
    let before = common::read(&profile.profile_ini());

    let mut settings = profile.load_settings().unwrap();
    profile.save_settings(&settings).unwrap();

    // The user's text survives a save of the unchanged size, and reloads as
    // the same size.
    let saved = common::read(&profile.profile_ini());
    assert_eq!(
        String::from_utf8(saved).unwrap(),
        String::from_utf8(before).unwrap()
    );
    assert_eq!(profile.load_settings().unwrap(), settings);

    // A size set in bytes is written in bytes.
    settings.max_bsa_uncompressed_size = size(6_442_450_944);
    profile.save_settings(&settings).unwrap();

    let saved = String::from_utf8(common::read(&profile.profile_ini())).unwrap();
    assert!(
        saved.contains("maxBsaUncompressedSize=6442450944\r\n"),
        "{saved}"
    );
}

#[test]
fn a_size_that_is_not_a_number_of_bytes_or_units_is_reported_rather_than_read_as_zero() {
    for value in [
        "",
        "two",
        "GB",
        "2XB",
        "2 G",
        "2GB2",
        "-1GB",
        "inf GB",
        "nan GB",
        // 2^64 bytes, one past the largest size, whole and fractional.
        "16777216TB",
        "16777216.5TB",
    ] {
        let profile = shipped_copy(&format!("archive-size-text-{value}"), "SSE");
        replace_size_line(&profile, &format!("maxBsaUncompressedSize={value}"));

        let error = profile.load_settings().unwrap_err();

        assert_eq!(
            error.to_string(),
            format!(
                "`profile.ini`: `BSA/maxBsaUncompressedSize` is `{value}`; expected {ACCEPTED}"
            ),
            "{value}"
        );
    }
}

#[test]
fn a_size_saves_as_whole_byte_integer_text_and_reloads_exactly() {
    // Qt's double writer would store 4 TB as `4e+12`, and a double cannot hold
    // the largest size at all.
    for (bytes, line) in [
        (1, "maxBsaUncompressedSize=1"),
        (4_000_000_000_000, "maxBsaUncompressedSize=4000000000000"),
        (u64::MAX, "maxBsaUncompressedSize=18446744073709551615"),
    ] {
        let profile = shipped_copy(&format!("archive-size-saved-{bytes}"), "SSE");
        let mut settings = profile.load_settings().unwrap();
        settings.max_bsa_uncompressed_size = size(bytes);

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
                "`profile.ini`: `BSA/maxBsaUncompressedSize` is `{value}`; expected {ACCEPTED}"
            )
        );
    }
}
