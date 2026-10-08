//! Links CAO's application manifest into every artifact of this crate,
//! including its `#[test]` binaries (#471).
//!
//! `longPathAware` and the UTF-8 active code page apply per process; without
//! the manifest, this crate's tests would read and write Archives under
//! different path behaviour from the app.

fn main() {
    const RESOURCE_SCRIPT: &str = "../../resources/cao-manifest.rc";
    const MANIFEST: &str = "../../resources/Cathedral_Assets_Optimizer.manifest";

    // embed-resource emits no rerun annotations for files the resource script
    // pulls in, so name both inputs.
    println!("cargo:rerun-if-changed={RESOURCE_SCRIPT}");
    println!("cargo:rerun-if-changed={MANIFEST}");

    // `compile_for_everything` reaches unit-test binaries too; see cao-winfs's
    // build script for why `compile_for_tests` is not enough. Link arguments
    // never reach dependents, so the app's binaries are unaffected.
    embed_resource::compile_for_everything(RESOURCE_SCRIPT, embed_resource::NONE)
        .manifest_required()
        .unwrap();
}
