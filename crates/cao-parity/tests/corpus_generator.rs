//! The corpus generator (#500): the generated cases, drawn from the pairwise
//! covering array over options, profile overrides and tree features.

mod common;

use std::collections::BTreeSet;

use cao_parity::case::{CaseFile, ModSelection};
use cao_parity::cases::fixtures_dir;
use cao_parity::generate::{GENERATOR_VERSION, generated_case, generated_cases};
use cao_parity::guard::{GuardInput, rejections};
use cao_parity::materialise::write_input;
use cao_parity::recipe::{ContentEntry, OutputFormat};
use common::{TempDir, shipped_profiles};

fn profile(case: &CaseFile) -> &str {
    &case.spec.profile
}

#[test]
fn the_same_generator_version_gives_the_same_cases() {
    let first = generated_cases();
    assert!(!first.is_empty());
    assert_eq!(generated_cases(), first);
    for (id, case) in &first {
        assert_eq!(case.generator_version, Some(GENERATOR_VERSION), "{id}");
        assert_eq!(
            generated_case(id).as_ref(),
            Some(case),
            "{id} is found by id"
        );
    }
    assert_eq!(generated_case("pw-999-sse-om-apply"), None);
}

#[test]
fn case_ids_are_unique_descriptive_names() {
    let cases = generated_cases();
    let ids: BTreeSet<String> = cases
        .iter()
        .map(|(id, _)| id.to_ascii_lowercase())
        .collect();
    assert_eq!(ids.len(), cases.len(), "ids are unique ignoring case");
    for (id, case) in &cases {
        let mode = match case.spec.mod_selection {
            ModSelection::OneMod { .. } => "om",
            ModSelection::SeveralMods { .. } => "sm",
        };
        let run = if case.spec.dry_run { "dry" } else { "apply" };
        let suffix = format!("-{}-{mode}-{run}", profile(case).to_lowercase());
        assert!(
            (id.starts_with("pw-") || id.starts_with("se-")) && id.ends_with(&suffix),
            "{id} does not describe {suffix}"
        );
    }
}

#[test]
fn generated_specs_keep_the_spec_constraints() {
    for (id, case) in generated_cases() {
        let spec = &case.spec;
        if spec.profile == "FO4" {
            assert!(
                spec.meshes.level == 0 && !spec.meshes.resave,
                "{id}: FO4 Mesh work"
            );
            assert!(!spec.archives.merge_textures, "{id}: FO4 merges textures");
        }
        if spec.animations {
            assert_eq!(spec.profile, "SSE", "{id}: only SSE has Animations");
        }
        if spec.dry_run {
            let archives = &spec.archives;
            assert!(
                !(archives.extract || archives.create || archives.delete_backup),
                "{id}: a Dry Run with Archive work"
            );
        }
        if matches!(spec.mod_selection, ModSelection::SeveralMods { .. }) {
            assert!(spec.meshes.level <= 1, "{id}: Several Mods above Necessary");
        }
    }
}

#[test]
fn pairs_of_options_overrides_and_tree_features_are_covered() {
    let cases: Vec<CaseFile> = generated_cases()
        .into_iter()
        .map(|(_, case)| case)
        .collect();
    let has = |test: &dyn Fn(&CaseFile) -> bool| cases.iter().any(test);
    for profile_name in ["SSE", "TES5", "FO4"] {
        for dry_run in [false, true] {
            for several in [false, true] {
                assert!(
                    has(&|case| {
                        case.spec.profile == profile_name
                            && case.spec.dry_run == dry_run
                            && matches!(case.spec.mod_selection, ModSelection::SeveralMods { .. })
                                == several
                    }),
                    "{profile_name} dry={dry_run} several={several}"
                );
            }
        }
        for format in [
            None,
            Some(OutputFormat::Bc7),
            Some(OutputFormat::Bc5),
            Some(OutputFormat::Bc3),
            Some(OutputFormat::Bc1),
            Some(OutputFormat::R8G8B8A8),
        ] {
            assert!(
                has(&|case| case.spec.profile == profile_name
                    && case.profile_overrides.output_format == format),
                "{profile_name} with output format {format:?}"
            );
        }
        // A tree feature: an input Archive, in each profile's own game.
        assert!(
            has(&|case| case.spec.profile == profile_name
                && case
                    .tree
                    .content
                    .iter()
                    .any(|entry| matches!(entry, ContentEntry::Archive(_)))),
            "{profile_name} with an input Archive"
        );
    }
    assert!(has(&|case| case.spec.animations));
    assert!(has(&|case| case.spec.meshes.level == 3));
}

#[test]
fn a_handful_of_start_error_cases_select_a_missing_folder() {
    let start_errors: Vec<(String, CaseFile)> = generated_cases()
        .into_iter()
        .filter(|(id, _)| id.starts_with("se-"))
        .collect();
    assert!(!start_errors.is_empty() && start_errors.len() <= 5);
    for (id, case) in start_errors {
        let folder = case.spec.mod_selection.folder().to_owned();
        assert!(
            case.tree.content.iter().all(|entry| !entry
                .path()
                .to_lowercase()
                .starts_with(&folder.to_lowercase())),
            "{id}: `{folder}` must be missing from the tree"
        );
        assert!(
            !case.tree.content.is_empty(),
            "{id}: a tree is still written"
        );
    }
}

#[test]
fn every_generated_case_passes_the_deviation_guard() {
    let temp = TempDir::new("generated-guard");
    let profiles = shipped_profiles();
    for (id, case) in generated_cases() {
        let case_root = temp.path().join("parity").join(&id);
        let rejected = rejections(&GuardInput {
            case: &case,
            case_root: &case_root,
            profiles: &profiles,
            fixtures: &fixtures_dir(),
        })
        .unwrap();
        assert!(rejected.is_empty(), "{id}: {rejected:?}");
    }
}

/// Counts the files and bytes under `root`.
fn tree_size(root: &std::path::Path) -> (usize, u64) {
    let mut totals = (0, 0);
    for entry in std::fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            let (files, bytes) = tree_size(&entry.path());
            totals = (totals.0 + files, totals.1 + bytes);
        } else {
            totals = (totals.0 + 1, totals.1 + entry.metadata().unwrap().len());
        }
    }
    totals
}

#[test]
fn every_generated_case_builds_within_the_per_case_budget() {
    let temp = TempDir::new("generated-budget");
    for (id, case) in generated_cases() {
        let input = temp.path().join(&id).join("input");
        write_input(&case, &id, &input, &fixtures_dir())
            .unwrap_or_else(|error| panic!("{id}: {error}"));
        let (files, bytes) = tree_size(&input);
        assert!(files <= 64, "{id}: {files} files");
        assert!(bytes <= 32 * 1024 * 1024, "{id}: {bytes} bytes");
        std::fs::remove_dir_all(temp.path().join(&id)).unwrap();
    }
}
