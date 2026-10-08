//! Ordinal case-insensitive comparison, as a case-insensitive volume compares
//! names.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use cao_winfs::{OrdinalIgnoreCase, compare_ordinal_ignore_case};

#[test]
fn case_variants_are_one_name() {
    assert_eq!(
        compare_ordinal_ignore_case("textures/Armor/A.dds", "TEXTURES/armor/a.DDS"),
        Ordering::Equal
    );
    assert_eq!(
        compare_ordinal_ignore_case("Ärmel", "ärmel"),
        Ordering::Equal
    );
}

/// Unlike Rust's case mapping, no character expands: `ß` is not `SS`.
#[test]
fn sharp_s_does_not_expand() {
    assert_ne!(
        compare_ordinal_ignore_case("straße", "STRASSE"),
        Ordering::Equal
    );
    assert_eq!("straße".to_uppercase(), "STRASSE");
}

#[test]
fn ordering_ignores_case_then_compares_units() {
    // Byte order would put "B" before "a".
    assert_eq!(compare_ordinal_ignore_case("a", "B"), Ordering::Less);
    assert_eq!(compare_ordinal_ignore_case("b", "A"), Ordering::Greater);
    assert_eq!(compare_ordinal_ignore_case("a", "ab"), Ordering::Less);
    assert_eq!(compare_ordinal_ignore_case("", ""), Ordering::Equal);
}

#[test]
fn ordered_sets_treat_equivalent_spellings_as_one_key() {
    let mut names = BTreeSet::new();
    assert!(names.insert(OrdinalIgnoreCase("meshes/b.nif")));
    assert!(names.insert(OrdinalIgnoreCase("meshes/a.nif")));
    assert!(!names.insert(OrdinalIgnoreCase("MESHES/A.NIF")));
    let kept: Vec<&str> = names.iter().map(|name| name.0).collect();
    assert_eq!(kept, ["meshes/a.nif", "meshes/b.nif"]);
}
