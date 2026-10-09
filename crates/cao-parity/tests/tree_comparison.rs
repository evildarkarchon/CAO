//! The output-tree comparator over real directories.

mod common;

use std::cell::RefCell;
use std::path::Path;
use std::time::{Duration, SystemTime};

use cao_parity::HarnessError;
use cao_parity::compare::Verdict;
use cao_parity::leftovers::ParityRules;
use cao_parity::tree::{
    ArtifactRule, ArtifactVerdict, DefaultRules, RuleOutcome, TreeRules, TreeSide, compare_trees,
};
use common::{TempDir, write};

/// Two side directories under one temp dir.
struct Sides {
    _temp: TempDir,
    oracle: std::path::PathBuf,
    rust: std::path::PathBuf,
}

fn sides(name: &str) -> Sides {
    let temp = TempDir::new(name);
    let oracle = temp.path().join("oracle");
    let rust = temp.path().join("rust");
    std::fs::create_dir(&oracle).unwrap();
    std::fs::create_dir(&rust).unwrap();
    Sides {
        _temp: temp,
        oracle,
        rust,
    }
}

/// Writes the same file on both sides.
fn both(sides: &Sides, relative: &str, bytes: &[u8]) {
    write(&sides.oracle, relative, bytes);
    write(&sides.rust, relative, bytes);
}

fn side(root: &Path) -> TreeSide<'_> {
    TreeSide { root, run_id: None }
}

fn compare(sides: &Sides, rules: &dyn TreeRules) -> cao_parity::tree::TreeComparison {
    compare_trees(side(&sides.oracle), side(&sides.rust), rules).unwrap()
}

/// The `(path, rule)` of every Different artifact.
fn broken(comparison: &cao_parity::tree::TreeComparison) -> Vec<(String, &'static str)> {
    comparison
        .artifacts
        .iter()
        .filter_map(|artifact| match &artifact.verdict {
            ArtifactVerdict::Different(difference) => {
                Some((artifact.path.clone(), difference.rule))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn identical_trees_are_identical_whatever_their_timestamps_and_attributes() {
    let sides = sides("identical");
    both(&sides, "mods/A/textures/a.dds", b"dds bytes");
    both(&sides, "mods/A/A.esp", b"plugin");
    std::fs::create_dir_all(sides.oracle.join("mods/A/empty")).unwrap();
    std::fs::create_dir_all(sides.rust.join("mods/A/empty")).unwrap();

    let file = std::fs::File::options()
        .write(true)
        .open(sides.rust.join("mods/A/A.esp"))
        .unwrap();
    file.set_modified(SystemTime::now() - Duration::from_secs(86_400))
        .unwrap();
    drop(file);
    let mut permissions = std::fs::metadata(sides.rust.join("mods/A/A.esp"))
        .unwrap()
        .permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(sides.rust.join("mods/A/A.esp"), permissions.clone()).unwrap();

    let comparison = compare(&sides, &DefaultRules);
    assert_eq!(comparison.verdict(), Verdict::Identical);
    let paths: Vec<_> = comparison
        .artifacts
        .iter()
        .map(|artifact| artifact.path.as_str())
        .collect();
    assert_eq!(
        paths,
        vec![
            "mods",
            "mods/A",
            "mods/A/A.esp",
            "mods/A/empty",
            "mods/A/textures",
            "mods/A/textures/a.dds"
        ]
    );

    // Let the temp dir be removed. On Windows this only clears the read-only
    // attribute; the lint's world-writable concern is a Unix one.
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    std::fs::set_permissions(sides.rust.join("mods/A/A.esp"), permissions).unwrap();
}

#[test]
fn harness_owned_folders_are_not_output() {
    let sides = sides("harness-owned");
    both(&sides, "mods/A/a.txt", b"same");
    write(&sides.oracle, "profiles/SSE/profile.ini", b"oracle");
    write(&sides.rust, "profiles/SSE/profile.ini", b"rust");
    write(&sides.oracle, "logs/SSE/run.html", b"log");
    write(&sides.rust, "bin/hkxcmd.exe", b"exe");
    assert_eq!(compare(&sides, &DefaultRules).verdict(), Verdict::Identical);
}

#[test]
fn relative_paths_must_match_case_sensitively() {
    let sides = sides("paths");
    both(&sides, "mods/A/kept.txt", b"same");
    write(&sides.oracle, "mods/A/only-oracle.txt", b"x");
    write(&sides.rust, "mods/A/Only-Rust.txt", b"x");
    write(&sides.oracle, "mods/A/Case.txt", b"x");
    write(&sides.rust, "mods/A/case.txt", b"x");

    let comparison = compare(&sides, &DefaultRules);
    assert!(!comparison.verdict().passed());
    let mut broken = broken(&comparison);
    broken.sort();
    assert_eq!(
        broken,
        vec![
            ("mods/A/Case.txt".to_owned(), "Same Relative Paths"),
            ("mods/A/Only-Rust.txt".to_owned(), "Same Relative Paths"),
            ("mods/A/case.txt".to_owned(), "Same Relative Paths"),
            ("mods/A/only-oracle.txt".to_owned(), "Same Relative Paths"),
        ]
    );
}

#[test]
fn a_file_and_a_directory_at_one_path_differ_by_kind() {
    let sides = sides("kind");
    write(&sides.oracle, "mods/A/thing", b"file");
    std::fs::create_dir_all(sides.rust.join("mods/A/thing")).unwrap();
    assert_eq!(
        broken(&compare(&sides, &DefaultRules)),
        vec![("mods/A/thing".to_owned(), "Same Entry Kind")]
    );
}

#[test]
fn files_without_a_rule_must_be_byte_identical() {
    let sides = sides("bytes");
    write(&sides.oracle, "mods/A/a.txt", b"abcdef");
    write(&sides.rust, "mods/A/a.txt", b"abcXef");
    let comparison = compare(&sides, &DefaultRules);
    let Verdict::Different(differences) = comparison.verdict() else {
        panic!("expected Different");
    };
    assert_eq!(differences.len(), 1);
    assert_eq!(differences[0].path, "mods/A/a.txt");
    assert_eq!(differences[0].rule, "Byte Equality");
    assert!(
        differences[0].detail.contains("byte 3"),
        "{}",
        differences[0].detail
    );
}

/// A rule that records each path it is asked about and returns a fixed outcome.
struct Recording {
    name: &'static str,
    outcome: fn() -> RuleOutcome,
    calls: RefCell<Vec<String>>,
}

impl Recording {
    fn new(name: &'static str, outcome: fn() -> RuleOutcome) -> Self {
        Self {
            name,
            outcome,
            calls: RefCell::new(Vec::new()),
        }
    }
}

impl ArtifactRule for Recording {
    fn name(&self) -> &'static str {
        self.name
    }

    fn compare(&self, oracle: &Path, rust: &Path) -> Result<RuleOutcome, HarnessError> {
        assert_eq!(oracle.file_name(), rust.file_name());
        self.calls
            .borrow_mut()
            .push(oracle.file_name().unwrap().to_string_lossy().into_owned());
        Ok((self.outcome)())
    }
}

/// Textures go to `texture`; `.caobad` files are leftovers and go to `leftover`.
struct Hooks {
    texture: Recording,
    leftover: Recording,
}

impl TreeRules for Hooks {
    fn leftover_rule(&self, path: &str) -> Option<&dyn ArtifactRule> {
        path.ends_with(".caobad")
            .then_some(&self.leftover as &dyn ArtifactRule)
    }

    fn asset_rule(&self, path: &str) -> Option<&dyn ArtifactRule> {
        path.contains(".dds")
            .then_some(&self.texture as &dyn ArtifactRule)
    }
}

#[test]
fn the_asset_kind_hook_decides_files_whose_bytes_differ() {
    let sides = sides("asset-hook");
    write(&sides.oracle, "mods/A/textures/re-encoded.dds", b"gpu");
    write(&sides.rust, "mods/A/textures/re-encoded.dds", b"cpu");
    both(&sides, "mods/A/textures/same.dds", b"same");
    let hooks = Hooks {
        texture: Recording::new("Texture PSNR", || RuleOutcome::Equivalent),
        leftover: Recording::new("Leftover", || RuleOutcome::Different("unused".into())),
    };

    let comparison = compare(&sides, &hooks);
    assert_eq!(comparison.verdict(), Verdict::Equivalent);
    let verdict = &comparison
        .artifacts
        .iter()
        .find(|artifact| artifact.path == "mods/A/textures/re-encoded.dds")
        .unwrap()
        .verdict;
    assert_eq!(
        verdict,
        &ArtifactVerdict::Equivalent {
            rule: "Texture PSNR"
        }
    );
    assert_eq!(
        *hooks.texture.calls.borrow(),
        vec!["re-encoded.dds"],
        "identical bytes need no rule"
    );

    // A rule's Different names that rule.
    let hooks = Hooks {
        texture: Recording::new("Texture PSNR", || {
            RuleOutcome::Different("PSNR 31 dB on mip 0".into())
        }),
        leftover: Recording::new("Leftover", || RuleOutcome::Equivalent),
    };
    let Verdict::Different(differences) = compare(&sides, &hooks).verdict() else {
        panic!("expected Different");
    };
    assert_eq!(differences[0].rule, "Texture PSNR");
    assert_eq!(differences[0].detail, "PSNR 31 dB on mip 0");
}

#[test]
fn the_leftovers_hook_is_consulted_before_the_asset_kind_hook() {
    let sides = sides("leftover-hook");
    write(&sides.oracle, "mods/A/textures/bad.dds.caobad", b"one");
    write(&sides.rust, "mods/A/textures/bad.dds.caobad", b"two");
    let hooks = Hooks {
        texture: Recording::new("Texture PSNR", || RuleOutcome::Equivalent),
        leftover: Recording::new("Quarantine", || {
            RuleOutcome::Different("renamed original changed".into())
        }),
    };
    assert_eq!(
        broken(&compare(&sides, &hooks)),
        vec![("mods/A/textures/bad.dds.caobad".to_owned(), "Quarantine")]
    );
    assert!(hooks.texture.calls.borrow().is_empty());
}

#[test]
fn staging_leftovers_match_after_placeholder_normalisation() {
    let sides = sides("staging");
    let oracle_nonce = "0123456789abcdef0123456789abcdef";
    let rust_nonce = "fedcba9876543210fedcba9876543210";
    write(
        &sides.oracle,
        &format!("mods/A/.cao-staging/run-12-34-0-{oracle_nonce}/archive-entry-{oracle_nonce}"),
        b"partial",
    );
    write(
        &sides.rust,
        &format!("mods/A/.cao-staging/run-rust-1-{rust_nonce}/archive-entry-{rust_nonce}"),
        b"partial",
    );
    both(&sides, "mods/A/.cao-staging/owner.lock", b"");

    let comparison = compare_trees(
        TreeSide {
            root: &sides.oracle,
            run_id: Some("12-34-0"),
        },
        TreeSide {
            root: &sides.rust,
            run_id: Some("rust-1"),
        },
        &DefaultRules,
    )
    .unwrap();
    assert_eq!(comparison.verdict(), Verdict::Identical);
    assert!(
        comparison.artifacts.iter().any(|artifact| artifact.path
            == "mods/A/.cao-staging/run-{run-id}-{nonce}/archive-entry-{nonce}"),
        "artifacts are reported under their normalised paths"
    );
}

/// The oracle's staging names carry a random 32-hex token where the Rust
/// side's carry its Run ID; both normalise to the same placeholders by shape.
#[test]
fn cpp_staging_tokens_normalise_like_rust_run_ids() {
    let sides = sides("staging-shape");
    let token = "00112233445566778899aabbccddeeff";
    let (oracle_nonce, rust_nonce) = (
        "0123456789abcdef0123456789abcdef",
        "fedcba9876543210fedcba9876543210",
    );
    write(
        &sides.oracle,
        &format!("mods/A/textures/.cao-staging-texture-{token}-{oracle_nonce}.dds"),
        b"partial",
    );
    write(
        &sides.rust,
        &format!("mods/A/textures/.cao-staging-texture-rust-1-{rust_nonce}.dds"),
        b"partial",
    );
    write(
        &sides.oracle,
        &format!("mods/A/.cao-staging/run-{token}-{oracle_nonce}/archive-entry-{oracle_nonce}"),
        b"entry",
    );
    write(
        &sides.rust,
        &format!("mods/A/.cao-staging/run-rust-1-{rust_nonce}/archive-entry-{rust_nonce}"),
        b"entry",
    );
    // A mod's own `run-…` folder outside staging keeps its name.
    both(
        &sides,
        &format!("mods/A/run-{token}-{oracle_nonce}/a.txt"),
        b"mod",
    );

    let comparison = compare_trees(
        TreeSide {
            root: &sides.oracle,
            run_id: Some("12-34-0"),
        },
        TreeSide {
            root: &sides.rust,
            run_id: Some("rust-1"),
        },
        &ParityRules,
    )
    .unwrap();

    assert_eq!(
        comparison.verdict(),
        Verdict::Identical,
        "{:?}",
        broken(&comparison)
    );
    let paths: Vec<&str> = comparison
        .artifacts
        .iter()
        .map(|a| a.path.as_str())
        .collect();
    assert!(paths.contains(&"mods/A/textures/.cao-staging-texture-{run-id}-{nonce}.dds"));
    assert!(paths.contains(&"mods/A/.cao-staging/run-{run-id}-{nonce}"));
    assert!(paths.contains(&format!("mods/A/run-{token}-{oracle_nonce}/a.txt").as_str()));
}

/// Writes a v3 manifest for the Mod Root `mods/A` of `side`.
fn write_manifest(side: &Path, run_id: &str, nonce: &str, records: &[(char, String)]) {
    let root = side.join("mods/A").to_string_lossy().replace('\\', "/");
    let mut text = format!(
        "CAO-STAGING 3\n\"{root}\"\n\"{run_id}\" \"run-{run_id}-{nonce}\"\n{}\n",
        records.len()
    );
    for (kind, name) in records {
        text.push_str(&format!("{kind} \"{name}\"\n"));
    }
    write(
        side,
        "mods/A/.cao-staging/ownership.manifest",
        text.as_bytes(),
    );
}

/// Both builds' manifests are Equivalent when they record the same things,
/// whatever their roots, Run IDs and nonces; a different record set is not.
#[test]
fn ownership_manifests_compare_semantically() {
    let token = "00112233445566778899aabbccddeeff";
    let (oracle_nonce, rust_nonce) = (
        "0123456789abcdef0123456789abcdef",
        "fedcba9876543210fedcba9876543210",
    );
    let records = |run_id: &str, nonce: &str| {
        vec![
            ('D', format!("run-{run_id}-{nonce}")),
            (
                'S',
                format!("textures/.cao-staging-texture-{run_id}-{nonce}.dds"),
            ),
        ]
    };

    let same = sides("manifest-same");
    write_manifest(
        &same.oracle,
        token,
        oracle_nonce,
        &records(token, oracle_nonce),
    );
    write_manifest(
        &same.rust,
        "rust-1",
        rust_nonce,
        &records("rust-1", rust_nonce),
    );
    both(&same, "mods/A/.cao-staging/owner.lock", b"");
    let comparison = compare(&same, &ParityRules);
    assert_eq!(
        comparison.verdict(),
        Verdict::Equivalent,
        "{:?}",
        broken(&comparison)
    );

    let differing = sides("manifest-different");
    write_manifest(
        &differing.oracle,
        token,
        oracle_nonce,
        &records(token, oracle_nonce),
    );
    write_manifest(
        &differing.rust,
        "rust-1",
        rust_nonce,
        &records("rust-1", rust_nonce)[..1],
    );
    assert_eq!(
        broken(&compare(&differing, &ParityRules)),
        vec![(
            "mods/A/.cao-staging/ownership.manifest".to_owned(),
            "Ownership Manifest Semantics"
        )]
    );

    // A manifest naming another Mod Root is never Equivalent.
    let moved = sides("manifest-moved");
    write_manifest(
        &moved.oracle,
        token,
        oracle_nonce,
        &records(token, oracle_nonce),
    );
    write_manifest(
        &moved.rust,
        "rust-1",
        rust_nonce,
        &records("rust-1", rust_nonce),
    );
    let text = std::fs::read_to_string(moved.rust.join("mods/A/.cao-staging/ownership.manifest"))
        .unwrap()
        .replace("/mods/A\"", "/mods/B\"");
    write(
        &moved.rust,
        "mods/A/.cao-staging/ownership.manifest",
        text.as_bytes(),
    );
    assert_eq!(broken(&compare(&moved, &ParityRules)).len(), 1);
}

/// `owner.lock` is compared for presence only.
#[test]
fn owner_locks_compare_for_presence_only() {
    let sides = sides("owner-lock");
    write(&sides.oracle, "mods/A/.cao-staging/owner.lock", b"");
    write(&sides.rust, "mods/A/.cao-staging/owner.lock", b"anything");
    assert_eq!(compare(&sides, &ParityRules).verdict(), Verdict::Equivalent);

    // Presence is the rule: a lock on one side only is Different (and so are
    // the folders that exist only on that side).
    let missing = self::sides("owner-lock-missing");
    write(&missing.oracle, "mods/A/.cao-staging/owner.lock", b"");
    assert!(broken(&compare(&missing, &ParityRules)).contains(&(
        "mods/A/.cao-staging/owner.lock".to_owned(),
        "Same Relative Paths"
    )));
}
