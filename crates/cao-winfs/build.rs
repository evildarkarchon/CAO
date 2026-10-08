//! Links CAO's application manifest into every artifact of this crate,
//! including its `#[test]` binaries (#471).
//!
//! `longPathAware` and the UTF-8 active code page apply per process, and raw
//! Win32 calls such as `MoveFileExW` and `GetVolumePathNameW` only handle long
//! paths when the running executable declares them. Without the manifest, this
//! crate's tests would exercise different path behaviour from the app.

fn main() {
    const RESOURCE_SCRIPT: &str = "../../resources/cao-manifest.rc";
    // Referenced by RESOURCE_SCRIPT; the unit tests `include_bytes!` it too.
    const MANIFEST: &str = "../../resources/Cathedral_Assets_Optimizer.manifest";

    // embed-resource cannot see through the resource compiler's preprocessing,
    // so it emits no rerun annotations; name both inputs so an edited manifest
    // relinks.
    println!("cargo:rerun-if-changed={RESOURCE_SCRIPT}");
    println!("cargo:rerun-if-changed={MANIFEST}");

    // `compile_for_everything` links through plain `cargo:rustc-link-arg`, which
    // reaches unit-test binaries; `compile_for_tests` only covers some test kinds.
    // The manifest changes runtime behaviour, so failing to embed it is an error.
    embed_resource::compile_for_everything(RESOURCE_SCRIPT, embed_resource::NONE)
        .manifest_required()
        .unwrap();
}
