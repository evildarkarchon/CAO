//! Links the application manifest and the icon into the `cao-gui` exe and its
//! test binaries (#471, #508). The `.slint` files compile in `cao-gui-ui`.

/// Links the shared application manifest and `cao.ico` into every artifact.
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

    embed_resource::compile_for_everything(RESOURCE_SCRIPT, embed_resource::NONE)
        .manifest_required()
        .unwrap();
}
