//! Builds vendored nifly and CAO's C shim over it with `cc`, and links CAO's
//! application manifest into this crate's test binaries (#460, #471).
//!
//! nifly's own build is one static library of 13 sources with no configure
//! step, so `cc` reproduces it from an explicit file list. The flags mirror
//! nifly's CMake options; see `vendor/nifly/VENDORED.md` for the pin and patch.

use std::path::Path;

/// nifly's sources, as its own `src/CMakeLists.txt` lists them.
const NIFLY_SOURCES: [&str; 13] = [
    "Animation",
    "BasicTypes",
    "bhk",
    "ExtraData",
    "Factory",
    "Geometry",
    "NifFile",
    "Nodes",
    "Objects",
    "Particles",
    "Shaders",
    "Skin",
    "Object3d",
];

fn main() {
    build_nifly();
    embed_manifest();
}

/// Compiles nifly and `shim/cao_nif.cpp` into one static library.
fn build_nifly() {
    const SHIM: &str = "shim/cao_nif.cpp";
    let nifly = Path::new("vendor/nifly");

    // A directory makes Cargo rerun when any file under it changes.
    println!("cargo:rerun-if-changed={SHIM}");
    println!("cargo:rerun-if-changed={}", nifly.display());

    let mut build = cc::Build::new();
    // The `corpus` feature's mesh-creation entry points. Cargo reruns this
    // script whenever the feature set changes, so no rerun line is needed.
    if std::env::var_os("CARGO_FEATURE_CORPUS").is_some() {
        build.define("CAO_NIF_CORPUS", None);
    }
    build
        .cpp(true)
        // nifly's declared standard (its CMakeLists.txt); the patch makes C++20
        // compile too, but C++17 is what nifly is tested with.
        .std("c++17")
        .include(nifly.join("include"))
        .include(nifly.join("external"))
        .files(
            NIFLY_SOURCES
                .iter()
                .map(|name| nifly.join("src").join(format!("{name}.cpp"))),
        )
        .file(SHIM)
        // `cc` passes no exception model. Without /EHsc MSVC does not unwind, so
        // the shim's catch-all would leak destructors (C4530). /EHsc also lets
        // the compiler assume `extern "C"` functions never throw, which holds
        // because every shim entry point catches everything. /bigobj and
        // /Zc:inline are nifly's own MSVC options.
        .flag("/EHsc")
        .flag("/bigobj")
        .flag("/Zc:inline")
        // nifly is third-party code we never edit; its warnings are noise here.
        .warnings(false)
        .compile("nifly_cao");
}

/// Links CAO's application manifest into every artifact of this crate.
///
/// The manifest's `longPathAware` decides how nifly's `std::ifstream` and
/// `std::ofstream` treat long paths, so the tests must run under it to
/// exercise what the app will.
fn embed_manifest() {
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
