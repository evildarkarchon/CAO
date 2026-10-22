//! A Profile's Texture Target as the texture optimizer takes it: the `profile`
//! module maps each closed-set member to the `DXGI_FORMAT` of the same name.

use cao_optimizers::profile::texture_profile;
use cao_profiles::{ClosedSet, ProfileSettings, Profiles, TextureFormat, UnwantedFormat};

/// The shipped SSE Profile's settings, read from the repository's `profiles/`.
fn shipped_sse() -> ProfileSettings {
    let repository = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
    Profiles::new(repository)
        .open("SSE")
        .load_settings()
        .unwrap()
}

#[test]
fn every_texture_format_maps_to_the_dxgi_format_of_its_name() {
    let mut settings = shipped_sse();
    for &format in TextureFormat::ALL {
        settings.textures_format = format;

        let profile = texture_profile(&settings);

        // DirectXTex names each `DXGI_FORMAT` in its `Debug` output, so this
        // checks the stored number against DirectXTex's own table.
        assert_eq!(
            format!("{:?}", profile.format),
            format!("DXGI_FORMAT_{}", format.name())
        );
    }
}

#[test]
fn every_unwanted_format_maps_to_the_dxgi_format_of_its_name_in_the_users_order() {
    let mut settings = shipped_sse();
    settings.textures_unwanted_formats = UnwantedFormat::ALL.iter().rev().copied().collect();

    let profile = texture_profile(&settings);

    let mapped: Vec<String> = profile
        .unwanted_formats
        .iter()
        .map(|format| format!("{format:?}"))
        .collect();
    let named: Vec<String> = settings
        .textures_unwanted_formats
        .iter()
        .map(|format| format!("DXGI_FORMAT_{}", format.name()))
        .collect();
    assert_eq!(mapped, named);
}
