//! The 49-byte Dummy Plugins against bethutil's `btu::bsa::dummy` arrays at
//! `81f882ed`, as the #461 research note transcribes them.

use cao_archive::{Game, Settings};

/// Parses the space-separated hex the research note uses.
fn hex(text: &str) -> Vec<u8> {
    text.split_whitespace()
        .map(|byte| u8::from_str_radix(byte, 16).unwrap())
        .collect()
}

/// The record flags of a plugin's `TES4` header (bytes 8-11).
fn record_flags(plugin: &[u8]) -> u32 {
    u32::from_le_bytes(plugin[8..12].try_into().unwrap())
}

#[test]
fn tes5_dummy_matches_bethutil_and_is_not_light() {
    let dummy = Settings::get(Game::Tes5).dummy_plugin;
    assert_eq!(
        dummy.as_slice(),
        hex(
            "54 45 53 34 19 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 2b 00 00 00 \
             48 45 44 52 0c 00 9a 99 d9 3f 00 00 00 00 00 08 00 00 43 4e 41 4d 01 00 00"
        )
    );
    assert_eq!(record_flags(dummy), 0);
}

#[test]
fn sse_dummy_matches_bethutil_and_carries_the_esl_flag() {
    let dummy = Settings::get(Game::Sse).dummy_plugin;
    assert_eq!(
        dummy.as_slice(),
        hex(
            "54 45 53 34 19 00 00 00 00 02 00 00 00 00 00 00 00 00 00 00 2c 00 00 00 \
             48 45 44 52 0c 00 9a 99 d9 3f 00 00 00 00 00 08 00 00 43 4e 41 4d 01 00 00"
        )
    );
    assert_eq!(record_flags(dummy), 0x200);
}

#[test]
fn fo4_dummy_matches_bethutil_and_carries_the_esl_flag() {
    let dummy = Settings::get(Game::Fo4).dummy_plugin;
    assert_eq!(
        dummy.as_slice(),
        hex(
            "54 45 53 34 19 00 00 00 00 02 00 00 00 00 00 00 00 00 00 00 83 00 00 00 \
             48 45 44 52 0c 00 33 33 73 3f 00 00 00 00 00 08 00 00 43 4e 41 4d 01 00 00"
        )
    );
    assert_eq!(record_flags(dummy), 0x200);
}
