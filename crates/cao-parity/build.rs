//! Links CAO's application manifest into the `cao-parity` binary and its test
//! binaries (#471).
//!
//! `cao-parity run` drives the same composition root as the GUI, so it must run
//! with the same `longPathAware` and UTF-8 code-page settings, or long case
//! paths would behave differently from the app.

fn main() {
    const RESOURCE_SCRIPT: &str = "../../resources/cao-manifest.rc";
    const MANIFEST: &str = "../../resources/Cathedral_Assets_Optimizer.manifest";

    // embed-resource emits no rerun annotations for files the resource script
    // pulls in, so name both inputs.
    println!("cargo:rerun-if-changed={RESOURCE_SCRIPT}");
    println!("cargo:rerun-if-changed={MANIFEST}");

    // `compile_for_everything` reaches unit-test binaries too; see cao-winfs's
    // build script for why `compile_for_tests` is not enough.
    embed_resource::compile_for_everything(RESOURCE_SCRIPT, embed_resource::NONE)
        .manifest_required()
        .unwrap();
}
