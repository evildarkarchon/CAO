//! nifly's own fixture tests (`TestNifFile.cpp` at the pin), run through the
//! shim and the safe `Nif` type.
//!
//! Each case loads `<name>.nif`, optionally runs `OptimizeFor`, saves, and
//! requires the output to equal nifly's `<name>_expected.nif` byte for byte.
//! This is the #460 prototype's check, extended to every case nifly ships, and
//! it guards the `cc` build against drifting from how nifly is meant to be
//! compiled.

mod common;

use common::{assert_same_bytes, fixture, scratch_dir};
use nifly_sys::{LoadOptions, Nif, NifVersion, OptimizeOptions};

/// Loads and saves `name`, optimizing first when `optimize` is given, and
/// compares the result with nifly's expected file.
fn check_fixture(name: &str, optimize: Option<OptimizeOptions>) {
    let input = fixture(&format!("{name}.nif"));
    let expected = fixture(&format!("{name}_expected.nif"));
    let output = scratch_dir(name).join(format!("{name}_output.nif"));

    let mut nif = Nif::new();
    nif.load(&input, LoadOptions::default()).unwrap();
    if let Some(options) = optimize {
        nif.optimize_for(&options).unwrap();
    }
    nif.save(&output).unwrap();

    assert_same_bytes(&output, &expected);
}

/// nifly's own tests leave `removeParallax` at its default, `true`.
fn optimize_to(target: NifVersion, head_parts: bool) -> Option<OptimizeOptions> {
    Some(OptimizeOptions {
        target,
        head_parts,
        remove_parallax: true,
    })
}

macro_rules! round_trip {
    ($($test:ident => $name:literal,)*) => {$(
        #[test]
        fn $test() {
            check_fixture($name, None);
        }
    )*};
}

round_trip! {
    static_se => "TestNifFile_Static_SE",
    static_fo4 => "TestNifFile_Static_FO4",
    static_fo4_132 => "TestNifFile_Static_FO4_132",
    static_fo4_139 => "TestNifFile_Static_FO4_139",
    skinned_ob => "TestNifFile_Skinned_OB",
    skinned_se => "TestNifFile_Skinned_SE",
    skinned_dynamic_se => "TestNifFile_Skinned_Dynamic_SE",
    skinned_fo4 => "TestNifFile_Skinned_FO4",
    furniture_col_se => "TestNifFile_Furniture_Col_SE",
    loose_blocks_se => "TestNifFile_LooseBlocks_SE",
    multi_bound_se => "TestNifFile_MultiBound_SE",
    animated_le => "TestNifFile_Animated_LE",
    deep_graph_se => "TestNifFile_DeepGraph_SE",
    ordered_node_se => "TestNifFile_OrderedNode_SE",
    root_non_zero => "TestNifFile_RootNonZero",
    fo76 => "TestNifFile_FO76",
}

#[test]
fn optimize_le_to_se() {
    check_fixture(
        "TestNifFile_Optimize_LE_to_SE",
        optimize_to(NifVersion::SSE, false),
    );
}

#[test]
fn optimize_dynamic_le_to_se() {
    check_fixture(
        "TestNifFile_Optimize_Dynamic_LE_to_SE",
        optimize_to(NifVersion::SSE, true),
    );
}

#[test]
fn optimize_se_to_le() {
    check_fixture(
        "TestNifFile_Optimize_SE_to_LE",
        optimize_to(NifVersion::SK, false),
    );
}

#[test]
fn optimize_dynamic_se_to_le() {
    check_fixture(
        "TestNifFile_Optimize_Dynamic_SE_to_LE",
        optimize_to(NifVersion::SK, true),
    );
}
