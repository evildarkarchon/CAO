//! Archive extraction over real Archives (#497): the `cao-archive` reader, the
//! `cao-winfs` probes, source backup or removal, and whole Apply runs through
//! the production wiring.
//!
//! The source-cleanup scenarios port C++ `tests/MainOptimizerTests.cpp`,
//! which drove `BSAOptimizer::extract` against real BSAs; each names its
//! origin. The rest are Rust-only.

mod common;

use std::path::{Path, PathBuf};

use cao_archive::{ArchiveData, ArchiveType, Game, Settings, write_archive};
use cao_core::execution::MutationState;
use cao_core::run::{
    ArchiveExtractionFailure, ArchiveExtractionPlan, ArchiveExtractor, ArchiveReader, RunOutcome,
    SourceCleanup, SourceFilePin, TemporaryArtifactRegistry, create_run_id,
};
use cao_optimizers::archives::{ArchiveFileReader, VolumeProbes};
use cao_optimizers::composition::ApplicationRun;
use cao_profiles::Options;
use cao_winfs::{Access, Open, Share, msvc_canonical};
use common::{app_dir, profile_options, scratch_dir, serial, write};
use directxtex::{
    CP_FLAGS_NONE, DDS_FLAGS_NONE, DXGI_FORMAT, DXGI_FORMAT_BC1_UNORM_SRGB, DXGI_FORMAT_BC6H_UF16,
    DXGI_FORMAT_BC7_UNORM, ScratchImage, TexMetadata,
};

/// The game path every single-entry fixture Archive holds, as plans spell it.
const FIXTURE: &str = "textures/fixture.dds";

/// Writes an Archive of `game`'s `archive_type` at `out` holding `files`,
/// each a game path with its bytes, packed from a scratch folder beside
/// `out`'s directory, as Archive creation packs a Mod Root.
fn write_test_archive(
    out: &Path,
    game: Game,
    archive_type: ArchiveType,
    compress: bool,
    files: &[(&str, Vec<u8>)],
) {
    let input = out
        .parent()
        .unwrap()
        .with_file_name(format!("input-{}", out.file_name().unwrap().display()));
    // A missing folder is the expected case.
    let _ = std::fs::remove_dir_all(&input);
    let mut data = ArchiveData::new(&Settings::get(game), archive_type);
    for (game_path, bytes) in files {
        write(&input, game_path, bytes);
        assert!(data.add_file(input.join(game_path), bytes.len() as u64));
    }
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    write_archive(compress, &data, &input, out).unwrap();
}

/// A canonical Mod Root `mod` under a fresh scratch folder.
fn mod_root(name: &str) -> PathBuf {
    let root = scratch_dir(name).join("mod");
    std::fs::create_dir_all(&root).unwrap();
    msvc_canonical(&root).unwrap()
}

/// A plan that publishes every one of `entries` from `archive`.
fn plan(archive: &Path, root: &Path, entries: &[&str]) -> ArchiveExtractionPlan {
    let entries: Vec<String> = entries.iter().map(|entry| (*entry).to_owned()).collect();
    ArchiveExtractionPlan {
        archive_path: archive.to_path_buf(),
        mod_root: root.to_path_buf(),
        merge_entries: entries.clone(),
        entries,
        estimated_capacity_bytes: 0,
    }
}

/// One row of `archiveSourceCleanupRequiresSuccessfulMerge`.
#[derive(Debug, Clone, Copy)]
struct CleanupRow {
    name: &'static str,
    valid_archive: bool,
    delete_backup: bool,
    block_source_cleanup: bool,
    linked_backup: bool,
}

/// Origin: archiveSourceCleanupRequiresSuccessfulMerge. The source is backed
/// up or removed only after a successful merge; an existing backup, even a
/// dangling link at a backup name, is never replaced; and a cleanup blocked
/// by another handle keeps the committed entries and, since the source is
/// still the same readable Archive, stays safe to continue.
#[test]
fn source_cleanup_requires_a_successful_merge() {
    let row =
        |name, valid_archive, delete_backup, block_source_cleanup, linked_backup| CleanupRow {
            name,
            valid_archive,
            delete_backup,
            block_source_cleanup,
            linked_backup,
        };
    let rows = [
        row("failed-backup", false, false, false, false),
        row("failed-delete", false, true, false, false),
        row("successful-backup", true, false, false, false),
        row("successful-delete", true, true, false, false),
        row("dangling-backup", true, false, false, true),
        row("blocked-backup", true, false, true, false),
        row("blocked-delete", true, true, true, false),
    ];
    for row in rows {
        source_cleanup_row(row);
    }
}

fn source_cleanup_row(row: CleanupRow) {
    let name = row.name;
    let root = mod_root(&format!("cleanup-{name}"));
    let source = root.join("assets.bsa");
    let backup = root.join("assets.bsa.bak");
    std::fs::write(&backup, b"existing backup bytes").unwrap();
    let occupied = root.join("assets.bsa.bak.bak");
    if row.linked_backup
        && std::os::windows::fs::symlink_file(root.join("absent"), &occupied).is_err()
    {
        eprintln!("{name}: not run, the process cannot create symlinks");
        return;
    }
    if row.valid_archive {
        write_test_archive(
            &source,
            Game::Sse,
            ArchiveType::Textures,
            false,
            &[(FIXTURE, b"archived bytes".to_vec())],
        );
    } else {
        std::fs::write(&source, b"corrupt archive bytes").unwrap();
    }
    let original = std::fs::read(&source).unwrap();
    // Permits extraction's reads while denying rename and delete, so the
    // failure comes after the merge.
    let lock = row
        .block_source_cleanup
        .then(|| Open::new(Access::READ, Share::READ).open(&source).unwrap());
    let reader = ArchiveFileReader::default();
    let mut artifacts = TemporaryArtifactRegistry::new(create_run_id());
    let cleanup = if row.delete_backup {
        SourceCleanup::Remove
    } else {
        SourceCleanup::Backup
    };

    let result = ArchiveExtractor::new(&reader, &VolumeProbes).extract_with_source_cleanup(
        &plan(&source, &root, &[FIXTURE]),
        cleanup,
        &mut artifacts,
    );
    drop(lock);

    let extracted = row.valid_archive;
    let cleaned = extracted && !row.block_source_cleanup;
    assert_eq!(result.succeeded(), cleaned, "{name}: {}", result.detail);
    assert_eq!(source.exists(), !cleaned, "{name}");
    assert_eq!(root.join(FIXTURE).exists(), extracted, "{name}");
    if row.block_source_cleanup {
        assert_eq!(
            result.failure,
            Some(ArchiveExtractionFailure::SourceCleanupFailed),
            "{name}"
        );
        assert_eq!(result.mutation, MutationState::Committed, "{name}");
        assert!(result.safe_to_continue, "{name}");
    }
    assert_eq!(std::fs::read(&backup).unwrap(), b"existing backup bytes");
    if !row.valid_archive || !row.delete_backup || row.block_source_cleanup {
        let retained = if cleaned {
            root.join(if row.linked_backup {
                "assets.bsa.bak.bak.bak"
            } else {
                "assets.bsa.bak.bak"
            })
        } else {
            source.clone()
        };
        assert_eq!(std::fs::read(&retained).unwrap(), original, "{name}");
    }
    if row.linked_backup {
        assert_eq!(std::fs::read_link(&occupied).unwrap(), root.join("absent"));
    }
    assert!(artifacts.cleanup().is_empty(), "{name}");
}

/// Origin: archiveSourceLinkIsNotCleaned. A source that is a link is refused
/// by the pin before anything is read, so neither the link nor its target is
/// cleaned and nothing is extracted.
#[test]
fn a_linked_source_is_not_extracted_or_cleaned() {
    for cleanup in [SourceCleanup::Backup, SourceCleanup::Remove] {
        let root = mod_root(&format!("linked-source-{cleanup:?}"));
        let target = root.join("original.bsa");
        let source = root.join("assets.bsa");
        write_test_archive(
            &target,
            Game::Sse,
            ArchiveType::Textures,
            false,
            &[(FIXTURE, b"archived bytes".to_vec())],
        );
        if std::os::windows::fs::symlink_file(&target, &source).is_err() {
            eprintln!("not run: the process cannot create symlinks");
            return;
        }
        let reader = ArchiveFileReader::default();
        let mut artifacts = TemporaryArtifactRegistry::new(create_run_id());

        let result = ArchiveExtractor::new(&reader, &VolumeProbes).extract_with_source_cleanup(
            &plan(&source, &root, &[FIXTURE]),
            cleanup,
            &mut artifacts,
        );

        assert!(result.failure.is_some());
        assert!(source.symlink_metadata().unwrap().is_symlink());
        assert!(target.exists());
        assert!(!root.join(FIXTURE).exists());
        assert!(artifacts.cleanup().is_empty());
    }
}

/// Origin: extractedSourceReplacementIsNotCleaned. Once an extracted source
/// is moved away and a new Archive put at its name, neither cleanup nor the
/// recovery pin touches the replacement or the displaced original, and the
/// extracted entry stays.
#[test]
fn a_replaced_source_is_not_cleaned() {
    for cleanup in [SourceCleanup::Backup, SourceCleanup::Remove] {
        let root = mod_root(&format!("replaced-source-{cleanup:?}"));
        let source = root.join("assets.bsa");
        let displaced = root.join("earlier.bsa");
        write_test_archive(
            &source,
            Game::Sse,
            ArchiveType::Textures,
            false,
            &[(FIXTURE, b"archived bytes".to_vec())],
        );
        let mut pin = SourceFilePin::new(&source, &root).unwrap();
        let reader = ArchiveFileReader::default();
        let mut artifacts = TemporaryArtifactRegistry::new(create_run_id());
        let result = ArchiveExtractor::new(&reader, &VolumeProbes)
            .extract(&plan(&source, &root, &[FIXTURE]), &mut artifacts);
        assert!(result.succeeded(), "{}", result.detail);

        pin.release_for_cleanup();
        std::fs::rename(&source, &displaced).unwrap();
        write_test_archive(
            &source,
            Game::Sse,
            ArchiveType::Textures,
            false,
            &[(FIXTURE, b"replacement bytes".to_vec())],
        );

        let cleaned = match cleanup {
            SourceCleanup::Remove => pin.remove_if_unchanged(),
            SourceCleanup::Backup => pin.backup_if_unchanged().map(|_| ()),
        };
        assert!(cleaned.is_err());
        assert!(pin.pin_unchanged_for_recovery().is_err());
        drop(pin);
        assert!(source.exists());
        assert!(displaced.exists());
        assert!(!root.join("assets.bsa.bak").exists());
        assert_eq!(
            std::fs::read(root.join(FIXTURE)).unwrap(),
            b"archived bytes"
        );
        assert!(artifacts.cleanup().is_empty());
    }
}

/// Rust-only, for the #497 risk "drop a memory-mapped Archive before
/// deleting or renaming its source". The reader keeps an Archive mapped
/// between entries, and that mapping really does stop source cleanup's
/// identity-bound delete (Win32 delete disposition, as C++ used; std's
/// `remove_file` uses POSIX semantics, which a mapping does not stop, so it
/// would prove nothing). Once released, the source can be deleted, or
/// renamed to its backup, straight after extraction.
#[test]
fn a_released_source_can_be_deleted_and_renamed_straight_after_extraction() {
    let root = mod_root("released-source");
    for cleanup in [SourceCleanup::Remove, SourceCleanup::Backup] {
        let source = root.join(format!("{cleanup:?}.bsa"));
        write_test_archive(
            &source,
            Game::Sse,
            ArchiveType::Textures,
            true,
            &[(FIXTURE, b"archived bytes".repeat(100))],
        );
        let reader = ArchiveFileReader::default();
        let listed = reader.list_entries(&source).unwrap();
        assert_eq!(listed.len(), 1);
        let mut pin = SourceFilePin::new(&source, &root).unwrap();
        let out = root.join(format!("{cleanup:?}.dds"));
        // The reader writes over a file staging already created.
        std::fs::write(&out, b"").unwrap();
        reader
            .extract_entry(&source, &listed[0].name, &out)
            .unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"archived bytes".repeat(100));
        if cleanup == SourceCleanup::Remove {
            assert!(
                pin.remove_if_unchanged().is_err(),
                "a mapped Archive cannot be deleted, so the reader must be released first"
            );
            assert!(source.exists());
        }

        reader.release();
        match cleanup {
            SourceCleanup::Remove => pin.remove_if_unchanged().unwrap(),
            SourceCleanup::Backup => {
                let backup = pin.backup_if_unchanged().unwrap();
                assert_eq!(backup, root.join("Backup.bsa.bak"));
                drop(pin);
                // Nothing holds the renamed source either.
                std::fs::remove_file(&backup).unwrap();
            }
        }
        assert!(!source.exists());
    }
}

/// Apply options over `mod_root` under `profile` with Archive extraction as
/// the only work.
fn apply_extraction(app: &Path, profile: &str, mod_root: &Path, delete_backup: bool) -> Options {
    let mut options = profile_options(app, profile);
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
    options.bsa_delete_backup = delete_backup;
    options
}

/// Rust-only, through the production wiring: an SSE Apply run extracts an
/// enabled BSA into the Effective Asset Tree, keeps the Loose Asset that
/// shadows one entry, then backs the source up as `.bak` or deletes it,
/// straight after extraction.
#[test]
fn an_sse_apply_extracts_a_bsa_then_backs_up_or_deletes_its_source() {
    let _serial = serial();
    for delete_backup in [false, true] {
        let app = app_dir(&format!("extract-sse-{delete_backup}"));
        let mod_root = app.join("mods").join("Mod");
        write(&mod_root, "textures/shared.dds", b"loose bytes");
        let archive = mod_root.join("Mod.bsa");
        write_test_archive(
            &archive,
            Game::Sse,
            ArchiveType::Standard,
            true,
            &[
                ("textures/shared.dds", b"archived texture".to_vec()),
                ("meshes/armor/cuirass.nif", b"archived mesh".repeat(40)),
            ],
        );

        let options = apply_extraction(&app, "SSE", &mod_root, delete_backup);
        let run = ApplicationRun::new(&app, "SSE", &options).unwrap();
        let result = run.start(None).unwrap().wait();

        assert_eq!(
            result.outcome(),
            RunOutcome::Succeeded,
            "{:?}",
            result.failures()
        );
        let attempts = result.archive_extraction_attempts();
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].mutation, MutationState::Committed);
        assert_eq!(
            std::fs::read(mod_root.join("textures/shared.dds")).unwrap(),
            b"loose bytes",
            "a Loose Asset outranks the Archive"
        );
        assert_eq!(
            std::fs::read(mod_root.join("meshes/armor/cuirass.nif")).unwrap(),
            b"archived mesh".repeat(40)
        );
        assert!(!archive.exists());
        assert_eq!(mod_root.join("Mod.bsa.bak").exists(), !delete_backup);
    }
}

/// A cubemap of 16×16 faces with one mip, in `format`, filled with seeded
/// bytes: extraction copies blocks and never decodes them.
fn cubemap_dds(format: DXGI_FORMAT) -> Vec<u8> {
    let mut scratch = ScratchImage::default();
    scratch
        .initialize_cube(format, 16, 16, 1, 1, CP_FLAGS_NONE)
        .unwrap();
    for (index, byte) in scratch.pixels_mut().iter_mut().enumerate() {
        *byte = (index * 13 + 5) as u8;
    }
    scratch.save_dds(DDS_FLAGS_NONE).unwrap().buffer().to_vec()
}

/// The metadata and pixel bytes of a DDS file.
fn load_dds(path: &Path) -> (TexMetadata, Vec<u8>) {
    let bytes = std::fs::read(path).unwrap();
    let mut info = TexMetadata::default();
    let image = ScratchImage::load_dds(&bytes, DDS_FLAGS_NONE, Some(&mut info), None).unwrap();
    (info, image.pixels().to_vec())
}

/// **Deviation 7:** FO4 DX10 cubemaps in BC7, BC6H and sRGB formats, which
/// only the DX10 DDS header can describe, extract successfully, every face
/// intact. C++ asked DirectXTex for one array item and failed them. A Main
/// BA2 extracts beside the Textures BA2 in the same run.
#[test]
fn deviation_7_fo4_dx10_cubemaps_in_bc7_bc6h_and_srgb_extract() {
    let _serial = serial();
    let app = app_dir("extract-fo4-cubemaps");
    let mod_root = app.join("mods").join("Mod");
    let cubemaps = [
        ("textures/sky/bc7.dds", DXGI_FORMAT_BC7_UNORM),
        ("textures/sky/bc6h.dds", DXGI_FORMAT_BC6H_UF16),
        ("textures/sky/srgb.dds", DXGI_FORMAT_BC1_UNORM_SRGB),
    ];
    let sources: Vec<(&str, Vec<u8>)> = cubemaps
        .iter()
        .map(|(game_path, format)| (*game_path, cubemap_dds(*format)))
        .collect();
    write_test_archive(
        &mod_root.join("Mod - Textures.ba2"),
        Game::Fo4,
        ArchiveType::Textures,
        true,
        &sources,
    );
    write_test_archive(
        &mod_root.join("Mod - Main.ba2"),
        Game::Fo4,
        ArchiveType::Standard,
        true,
        &[("scripts/quest.pex", b"compiled script".repeat(20))],
    );

    let options = apply_extraction(&app, "FO4", &mod_root, true);
    let run = ApplicationRun::new(&app, "FO4", &options).unwrap();
    let result = run.start(None).unwrap().wait();

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    assert_eq!(result.archive_extraction_attempts().len(), 2);
    for ((game_path, format), (_, source)) in cubemaps.iter().zip(&sources) {
        let (info, pixels) = load_dds(&mod_root.join(game_path));
        assert!(info.is_cubemap(), "{game_path}");
        assert_eq!((info.format, info.array_size), (*format, 6), "{game_path}");
        let expected = {
            let image = ScratchImage::load_dds(source, DDS_FLAGS_NONE, None, None).unwrap();
            image.pixels().to_vec()
        };
        assert!(pixels == expected, "{game_path}: faces differ");
    }
    assert_eq!(
        std::fs::read(mod_root.join("scripts/quest.pex")).unwrap(),
        b"compiled script".repeat(20)
    );
    assert!(!mod_root.join("Mod - Textures.ba2").exists());
    assert!(!mod_root.join("Mod - Main.ba2").exists());
}
