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

    probe_symlink_privilege();
}

/// Sets `cfg(symlink_privilege)` when this host can create file symlinks, as
/// cao-winfs's build script does: the materialiser's `file_symlink` test is
/// `ignore`d without it, so it is reported as skipped rather than passing.
/// After enabling Developer Mode, run `cargo clean -p cao-parity`.
fn probe_symlink_privilege() {
    println!("cargo::rustc-check-cfg=cfg(symlink_privilege)");
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let target = out.join("symlink-probe-target");
    let link = out.join("symlink-probe-link");
    // A link left by an earlier probe would make the creation below fail.
    let _ = std::fs::remove_file(&link);
    std::fs::write(&target, b"").unwrap();
    if std::os::windows::fs::symlink_file(&target, &link).is_ok() {
        println!("cargo::rustc-cfg=symlink_privilege");
        // Leaving the probe link behind is harmless; the next probe removes it.
        let _ = std::fs::remove_file(&link);
    }
}
