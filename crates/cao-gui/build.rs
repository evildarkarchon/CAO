//! Links the application manifest, the icon and the version resource into the
//! `cao-gui` exe and its test binaries (#471, #508, WS-9). The `.slint` files
//! compile in `cao-gui-ui`.

use std::env;
use std::fs;
use std::path::Path;

/// Links the shared application manifest, `cao.ico` and the VERSIONINFO into
/// every artifact.
///
/// `compile_for_everything` rather than `compile`: `compile` reaches only the
/// exe, but the headless tests save real profiles under scratch paths, and
/// `longPathAware` and the UTF-8 code page apply per process, so the test
/// binaries must run with the exe's settings too (#471). The icon rides along
/// harmlessly; a second, manifest-only script for the tests cannot be added
/// instead, because two manifests with ID 1 in the exe would not link.
fn main() {
    const RESOURCE_SCRIPT: &str = "../../resources/cao-gui.rc";
    const MANIFEST: &str = "../../resources/Cathedral_Assets_Optimizer.manifest";
    const ICON: &str = "../../resources/cao.ico";

    // embed-resource emits no rerun annotations for files the resource script
    // pulls in, so name every input.
    println!("cargo:rerun-if-changed={RESOURCE_SCRIPT}");
    println!("cargo:rerun-if-changed={MANIFEST}");
    println!("cargo:rerun-if-changed={ICON}");

    write_version_header();

    embed_resource::compile_for_everything(RESOURCE_SCRIPT, embed_resource::NONE)
        .manifest_required()
        .unwrap();
}

/// Writes `cao-version.h` into `OUT_DIR` for `cao-gui.rc`'s VERSIONINFO, from
/// the package version Cargo hands the build script.
///
/// Every member inherits the workspace version, so this is the version the
/// About box shows. A change of version changes the package's fingerprint, so
/// Cargo reruns this script without a `rerun-if-changed` for `Cargo.toml`.
/// The fourth field of the numeric version is always 0, and a version with a
/// pre-release part (`1.1.0-beta.1`) sets `VS_FF_PRERELEASE`, since the numbers
/// alone cannot say so.
///
/// # Panics
///
/// Panics, failing the build, when Cargo's version variables are missing or
/// `OUT_DIR` cannot be written.
fn write_version_header() {
    let var = |name: &str| env::var(name).unwrap_or_else(|_| panic!("Cargo sets {name}"));
    let numbers = format!(
        "{},{},{},0",
        var("CARGO_PKG_VERSION_MAJOR"),
        var("CARGO_PKG_VERSION_MINOR"),
        var("CARGO_PKG_VERSION_PATCH"),
    );
    // 0x2L = VS_FF_PRERELEASE.
    let flags = if var("CARGO_PKG_VERSION_PRE").is_empty() {
        "0x0L"
    } else {
        "0x2L"
    };
    // Semver allows only ASCII alphanumerics, '.', '-' and '+', so the version
    // needs no escaping inside an RC string literal.
    let header = format!(
        "#define CAO_VERSION_NUMBERS {numbers}\n\
         #define CAO_VERSION_FLAGS {flags}\n\
         #define CAO_VERSION_STRING \"{}\"\n",
        var("CARGO_PKG_VERSION"),
    );
    let out_dir = var("OUT_DIR");
    fs::write(Path::new(&out_dir).join("cao-version.h"), header)
        .expect("write cao-version.h into OUT_DIR");
}
