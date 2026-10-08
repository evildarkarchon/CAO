//! Links CAO's application manifest into every artifact of this crate,
//! including its `#[test]` binaries (#471).
//!
//! `longPathAware` and the UTF-8 active code page apply per process, and raw
//! Win32 calls such as `MoveFileExW` and `GetVolumePathNameW` only handle long
//! paths when the running executable declares them. Without the manifest, this
//! crate's tests would exercise different path behaviour from the app.
//!
//! It also probes whether file symlinks can be created here, for the tests
//! that need one; see `probe_symlink_privilege`.

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

    probe_symlink_privilege();
}

/// Sets `cfg(symlink_privilege)` when this host can create file symlinks
/// (`SeCreateSymbolicLinkPrivilege`, or Developer Mode).
///
/// Tests that need a file symlink are `ignore`d without it, so the harness
/// reports them as skipped rather than passing. Run them anyway with
/// `--include-ignored`: they then fail, never pass. The probe only reruns when
/// this crate rebuilds, so after enabling Developer Mode run
/// `cargo clean -p cao-winfs`.
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
