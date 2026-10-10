//! The real archive reader across every container C++ extracted: TES3, TES4
//! (stored and compressed) and FO4 `GNRL`.
//!
//! These are the per-format rows the `cao-core` ports left to the real reader,
//! from C++ `tests/ArchiveFirstAssetDiscoveryTests.cpp`; each names its
//! origin. Archives are built with `ba2` directly, so a manifest can hold raw
//! names, unsafe ones included, that `cao-archive` would never write.

mod common;

use std::path::Path;

use ba2::prelude::*;
use ba2::{fo4, tes3, tes4};
use cao_core::execution::MutationState;
use cao_core::run::{
    ArchiveExtractionPlan, ArchiveExtractor, RunFailureCode, RunOutcome, RunPhase,
    TemporaryArtifactRegistry, create_run_id,
};
use cao_optimizers::archives::{ArchiveFileReader, VolumeProbes};
use cao_optimizers::composition::ApplicationRun;
use cao_winfs::msvc_canonical;
use common::{app_dir, profile_options, scratch_dir, serial};

/// A raw Archive container, as C++ `createRawArchive` numbered them.
#[derive(Debug, Clone, Copy)]
enum Format {
    Tes3,
    Tes4,
    Fo4,
    CompressedTes4,
}

/// Writes an Archive at `path` holding one entry named exactly `name`, whose
/// payload is the single byte `B`.
///
/// `ba2` stores names normalized (lowercase), so, as C++ did, each stored name
/// is then overwritten in place with its original spelling. Lookups go by
/// hash, which ignores case, so the Archive still reads.
fn raw_archive(path: &Path, format: Format, name: &str) {
    const PAYLOAD: &[u8] = b"B";
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut out = std::fs::File::create_new(path).unwrap();
    // Each stored name with the spelling to restore over it.
    let mut names: Vec<(Vec<u8>, &str)> = Vec::new();
    match format {
        Format::Tes3 => {
            let key = tes3::ArchiveKey::from(name);
            names.push((key.name().to_vec(), name));
            let archive: tes3::Archive = [(key, tes3::File::from(PAYLOAD))].into_iter().collect();
            archive.write(&mut out).unwrap();
        }
        Format::Tes4 | Format::CompressedTes4 => {
            let (directory, file_name) = name.rsplit_once(['/', '\\']).unwrap_or(("", name));
            let directory_key = tes4::ArchiveKey::from(directory);
            let file_key = tes4::DirectoryKey::from(file_name);
            names.push((directory_key.name().to_vec(), directory));
            names.push((file_key.name().to_vec(), file_name));
            let mut file = tes4::File::from_decompressed(PAYLOAD);
            if matches!(format, Format::CompressedTes4) {
                let options = tes4::FileCompressionOptions::builder()
                    .version(tes4::Version::SSE)
                    .build();
                file = file.compress(&options).unwrap();
            }
            let directory_entries: tes4::Directory = [(file_key, file)].into_iter().collect();
            let archive: tes4::Archive = [(directory_key, directory_entries)].into_iter().collect();
            let mut flags =
                tes4::ArchiveFlags::DIRECTORY_STRINGS | tes4::ArchiveFlags::FILE_STRINGS;
            if matches!(format, Format::CompressedTes4) {
                flags |= tes4::ArchiveFlags::COMPRESSED;
            }
            let options = tes4::ArchiveOptions::builder()
                .version(tes4::Version::SSE)
                .flags(flags)
                .build();
            archive.write(&mut out, &options).unwrap();
        }
        Format::Fo4 => {
            let file: fo4::File = [fo4::Chunk::from_decompressed(PAYLOAD)]
                .into_iter()
                .collect();
            let key = fo4::ArchiveKey::from(name);
            names.push((key.name().to_vec(), name));
            let archive: fo4::Archive = [(key, file)].into_iter().collect();
            let options = fo4::ArchiveOptions::builder()
                .format(fo4::Format::GNRL)
                .strings(true)
                .build();
            archive.write(&mut out, &options).unwrap();
        }
    }
    drop(out);
    let mut bytes = std::fs::read(path).unwrap();
    for (stored, original) in names {
        assert_eq!(stored.len(), original.len(), "{original}");
        let position = bytes
            .windows(stored.len())
            .position(|window| window == stored.as_slice())
            .unwrap_or_else(|| panic!("{original} is not stored in the Archive"));
        bytes[position..position + stored.len()].copy_from_slice(original.as_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

/// Origin: stagedExtractionCommitsPayload. Every supported container's
/// payload commits from registered staging, and the source Archive is kept
/// byte for byte.
#[test]
fn every_container_commits_its_staged_payload() {
    for format in [
        Format::Tes3,
        Format::Tes4,
        Format::Fo4,
        Format::CompressedTes4,
    ] {
        let root = scratch_dir(&format!("formats-staged-{format:?}"));
        let root = msvc_canonical(&root).unwrap();
        let archive = root.join("source.bsa");
        raw_archive(&archive, format, "textures/a.dds");
        let original = std::fs::read(&archive).unwrap();
        let reader = ArchiveFileReader::default();
        let mut artifacts = TemporaryArtifactRegistry::new(create_run_id());
        let entries = vec!["textures/a.dds".to_owned()];
        let plan = ArchiveExtractionPlan {
            archive_path: archive.clone(),
            mod_root: root.clone(),
            merge_entries: entries.clone(),
            entries,
            estimated_capacity_bytes: 0,
        };

        let result = ArchiveExtractor::new(&reader, &VolumeProbes).extract(&plan, &mut artifacts);

        assert!(result.succeeded(), "{format:?}: {}", result.detail);
        assert_eq!(result.mutation, MutationState::Committed, "{format:?}");
        assert_eq!(std::fs::read(root.join("textures/a.dds")).unwrap(), b"B");
        assert_eq!(std::fs::read(&archive).unwrap(), original, "{format:?}");
        assert!(artifacts.cleanup().is_empty(), "{format:?}");
    }
}

/// Origin: rawManifestPaths. Through a whole SSE Apply run, each container's
/// raw manifest names are normalized: two spellings of one game path collide
/// and the winner keeps its own spelling, while a name escaping the Archive's
/// directory fails the run before any extraction. The Archives are untouched
/// either way.
#[test]
fn raw_manifest_paths_collide_or_fail_before_extraction() {
    let _serial = serial();
    for format in [Format::Tes3, Format::Tes4, Format::Fo4] {
        for escaping in [false, true] {
            let app = app_dir(&format!("formats-raw-{format:?}-{escaping}"));
            let mod_root = app.join("mods").join("Mod");
            let first = mod_root.join("a.bsa");
            let second = mod_root.join("b.bsa");
            raw_archive(&first, format, "Textures/Shared.dds");
            let second_name = if escaping {
                r"..\escaped.dds"
            } else {
                r"TEXTURES\folder\..\.\SHARED.DDS"
            };
            raw_archive(&second, format, second_name);
            let before = (
                std::fs::read(&first).unwrap(),
                std::fs::read(&second).unwrap(),
            );

            let mut options = profile_options(&app, "SSE");
            options.dry_run = false;
            options.user_path = mod_root.to_string_lossy().into_owned();
            options.textures_necessary = false;
            options.textures_compress = false;
            options.textures_mipmaps = false;
            options.textures_resize_ratio = false;
            options.textures_resize_size = false;
            options.meshes_optimization_level = 0;
            options.meshes_resave = false;
            options.animations_optimization = false;
            options.bsa_extract = true;
            options.bsa_create = false;
            // Keep the sources, so their bytes can be compared afterwards.
            options.bsa_delete_backup = false;
            let run = ApplicationRun::new(&app, "SSE", &options).unwrap();
            let result = run.start(None).unwrap().wait();

            let case = format!("{format:?}, escaping: {escaping}");
            let canonical_root = msvc_canonical(&mod_root).unwrap();
            if escaping {
                assert_eq!(result.outcome(), RunOutcome::Failed, "{case}");
                let failure = &result.failures()[0];
                assert_eq!(failure.code, RunFailureCode::ArchiveEntryInvalid, "{case}");
                assert_eq!(failure.phase, RunPhase::DiscoveringArchives, "{case}");
                assert_eq!(failure.path, canonical_root.join("b.bsa"), "{case}");
                assert!(result.archive_extraction_attempts().is_empty(), "{case}");
                assert_eq!(std::fs::read(&first).unwrap(), before.0, "{case}");
                assert_eq!(std::fs::read(&second).unwrap(), before.1, "{case}");
            } else {
                assert_eq!(
                    result.outcome(),
                    RunOutcome::Succeeded,
                    "{case}: {:?}",
                    result.failures()
                );
                let collisions = result.archive_collisions();
                assert_eq!(collisions.len(), 1, "{case}");
                assert_eq!(collisions[0].game_path, Path::new("Textures/Shared.dds"));
                assert_eq!(collisions[0].winning_archive, canonical_root.join("a.bsa"));
                assert_eq!(
                    collisions[0].shadowed_archives,
                    [canonical_root.join("b.bsa")]
                );
                assert_eq!(
                    std::fs::read(mod_root.join("Textures/Shared.dds")).unwrap(),
                    b"B",
                    "{case}"
                );
                // Backed up, never rewritten.
                assert_eq!(std::fs::read(mod_root.join("a.bsa.bak")).unwrap(), before.0);
                assert_eq!(std::fs::read(mod_root.join("b.bsa.bak")).unwrap(), before.1);
            }
            assert!(
                !mod_root.parent().unwrap().join("escaped.dds").exists(),
                "{case}"
            );
        }
    }
}
