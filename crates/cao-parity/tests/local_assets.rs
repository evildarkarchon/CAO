//! The pinned local asset list and the pool it pins (#501): the committed
//! list is valid, the pool folder stays out of git, and an invalid list is
//! rejected before any case reads the pool.

use std::path::Path;
use std::process::Command;

use cao_parity::local_assets::{Category, Edition, PinnedList, default_pool_dir, pinned_list_path};

/// One valid `[[asset]]` table, with `field = value` lines replaced or added.
fn list_with(overrides: &[(&str, &str)]) -> String {
    let mut fields = vec![
        ("id", "\"sse-static-a.nif\""),
        ("edition", "\"sse\""),
        ("archive", "\"Skyrim - Meshes0.bsa\""),
        ("path", "\"meshes/clutter/a.nif\""),
        ("category", "\"static\""),
        (
            "sha256",
            "\"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\"",
        ),
    ];
    for &(name, value) in overrides {
        match fields.iter_mut().find(|(field, _)| *field == name) {
            Some(field) => field.1 = value,
            None => fields.push((name, value)),
        }
    }
    let body: Vec<String> = fields
        .iter()
        .map(|(name, value)| format!("{name} = {value}"))
        .collect();
    format!("[[asset]]\n{}\n", body.join("\n"))
}

#[test]
fn the_committed_pinned_list_is_valid_and_covers_every_mesh_kind() {
    let list = PinnedList::committed().unwrap();
    for category in [
        Category::Static,
        Category::Skinned,
        Category::Headpart,
        Category::Facegen,
        Category::Lod,
        Category::Animation,
    ] {
        assert!(
            list.assets.iter().any(|asset| asset.category == category),
            "no {category:?} entry"
        );
    }
    for asset in &list.assets {
        let prefix = format!("{}-", asset.edition.folder());
        assert!(asset.id.starts_with(&prefix), "{}", asset.id);
    }
}

/// #502: the LE Animations `hkxcmd` converts, about 20 as the research (#473)
/// asked, all from LE's `Skyrim - Animations.bsa`.
#[test]
fn the_committed_pinned_list_holds_le_animations_to_convert() {
    let list = PinnedList::committed().unwrap();
    let le_animations: Vec<_> = list
        .assets
        .iter()
        .filter(|asset| asset.edition == Edition::Le && asset.category == Category::Animation)
        .collect();
    assert!(le_animations.len() >= 20, "{}", le_animations.len());
    for asset in le_animations {
        assert_eq!(asset.archive, "Skyrim - Animations.bsa", "{}", asset.id);
        assert!(asset.path.ends_with(".hkx"), "{}", asset.id);
    }
}

/// The pool is gitignored, so a maintainer's BSAs can never be committed,
/// while the list pinning it is not.
#[test]
fn the_pool_is_gitignored_and_the_pinned_list_is_not() {
    let ignored = |path: &Path| {
        let status = Command::new("git")
            .arg("check-ignore")
            .arg("-q")
            .arg(path)
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .status()
            .expect("git runs");
        // 0: ignored; 1: not ignored; anything else is git failing.
        match status.code() {
            Some(0) => true,
            Some(1) => false,
            code => panic!("git check-ignore failed with {code:?}"),
        }
    };
    let pool = default_pool_dir();
    for edition in [Edition::Le, Edition::Sse] {
        assert!(ignored(
            &pool.join(edition.folder()).join("Skyrim - Meshes0.bsa")
        ));
    }
    assert!(!ignored(&pinned_list_path()));
}

#[test]
fn a_valid_entry_parses() {
    let list = PinnedList::parse(&list_with(&[("note", "\"why\"")])).unwrap();
    let asset = list.get("sse-static-a.nif").unwrap();
    assert_eq!(asset.edition, Edition::Sse);
    assert_eq!(asset.category, Category::Static);
    assert_eq!(asset.note.as_deref(), Some("why"));
}

#[test]
fn an_invalid_entry_is_rejected_with_its_id() {
    for (field, value, expected) in [
        ("id", "\"Sse-Upper\"", "an id is lowercase"),
        ("id", "\"\"", "an id is lowercase"),
        (
            "archive",
            "\"le/Skyrim - Meshes.bsa\"",
            "is not a `.bsa` file name",
        ),
        (
            "archive",
            "\"Skyrim - Meshes.ba2\"",
            "is not a `.bsa` file name",
        ),
        (
            "path",
            "\"meshes\\\\a.nif\"",
            "is not a `/`-separated ASCII",
        ),
        (
            "path",
            "\"meshes/../a.nif\"",
            "is not a `/`-separated ASCII",
        ),
        ("path", "\"meshes/é.nif\"", "is not a `/`-separated ASCII"),
        ("sha256", "\"E3B0\"", "64 lowercase hex"),
        (
            "sha256",
            "\"E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855\"",
            "64 lowercase hex",
        ),
    ] {
        let error = PinnedList::parse(&list_with(&[(field, value)])).unwrap_err();
        assert!(error.contains(expected), "{field} = {value}: {error}");
    }
}

#[test]
fn an_unknown_field_edition_or_category_is_rejected() {
    for (field, value) in [
        ("archvie", "\"x.bsa\""),
        ("edition", "\"fo4\""),
        ("category", "\"texture\""),
    ] {
        assert!(
            PinnedList::parse(&list_with(&[(field, value)])).is_err(),
            "{field} = {value}"
        );
    }
}

#[test]
fn an_id_used_twice_is_rejected() {
    let twice = format!("{}\n{}", list_with(&[]), list_with(&[]));
    let error = PinnedList::parse(&twice).unwrap_err();
    assert!(error.contains("used twice"), "{error}");
}
