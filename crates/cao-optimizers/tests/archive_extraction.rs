//! Archive extraction over real Archives (#497): the `cao-archive` reader, the
//! `cao-winfs` probes, source backup or removal, and whole Apply runs through
//! the production wiring.
//!
//! The source-cleanup scenarios port C++ `tests/MainOptimizerTests.cpp`,
//! which drove `BSAOptimizer::extract` against real BSAs; each names its
//! origin. The rest are Rust-only.
//!
//! The extraction scenarios are whole Apply runs that request only Archive
//! extraction, over a Toolset built from the production reader, packer and
//! volume probes (#597). Every plan comes from the run's own preflight, so a
//! scenario that needs a source the preflight would refuse changes it after
//! preflight, on the run's first Archive extraction phase record.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cao_archive::{ArchiveData, ArchiveType, Game, Settings, write_archive};
use cao_core::execution::MutationState;
use cao_core::routing::RequestedWork;
use cao_core::run::{
    ArchiveExtractionFailure, ArchiveExtractionResult, ArchiveReader, ArchiveSeams,
    ArchiveWorkSettings, CancellationToken, OptimizationRunResult, RunOutcome, RunPhase,
    SourceCleanup, SourceFilePin,
};
use cao_core::test_support::{ArchiveRunRequest, RecordingSink};
use cao_optimizers::archives::{ArchiveFileReader, GameArchivePacker, VolumeProbes};
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

/// Runs one Apply run over `root` that requests only Archive extraction,
/// with `cleanup` for each extracted source, over the production reader, the
/// SSE packer and the real volume probes. `after_preflight` runs on the
/// run's first Archive extraction phase record: after preflight planned
/// every Archive, before the first attempt.
fn extract(
    root: &Path,
    cleanup: SourceCleanup,
    after_preflight: impl Fn() + 'static,
) -> OptimizationRunResult {
    let archive_run = ArchiveRunRequest::new(root)
        .with_work([RequestedWork::ArchiveExtraction])
        .with_archive_work(ArchiveWorkSettings {
            source_cleanup: cleanup,
            ..ArchiveWorkSettings::default()
        });
    let configuration = archive_run.seams_configuration(|| ArchiveSeams {
        reader: Arc::new(ArchiveFileReader::default()),
        volume_probe: Arc::new(VolumeProbes),
        packer: Arc::new(GameArchivePacker::new(Settings::get(Game::Sse))),
    });
    let sink = RecordingSink::on_first_record_of(RunPhase::ExtractingArchives, after_preflight);
    archive_run.execute(&configuration, Some(&sink), &CancellationToken::new())
}

/// The run's one Archive extraction attempt.
///
/// # Panics
///
/// When the run made no attempt or several, such as when its preflight
/// failed.
fn only_attempt(result: &OptimizationRunResult) -> &ArchiveExtractionResult {
    let attempts = result.archive_extraction_attempts();
    assert_eq!(attempts.len(), 1, "{:?}", result.failures());
    &attempts[0]
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
///
/// A run's preflight refuses an Archive it cannot read, so the failed rows
/// corrupt the source after preflight.
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
    write_test_archive(
        &source,
        Game::Sse,
        ArchiveType::Textures,
        false,
        &[(FIXTURE, b"archived bytes".to_vec())],
    );
    const CORRUPT: &[u8] = b"corrupt archive bytes";
    let original = if row.valid_archive {
        std::fs::read(&source).unwrap()
    } else {
        CORRUPT.to_vec()
    };
    // Permits extraction's reads while denying rename and delete, so the
    // failure comes after the merge.
    let lock = row
        .block_source_cleanup
        .then(|| Open::new(Access::READ, Share::READ).open(&source).unwrap());
    let cleanup = if row.delete_backup {
        SourceCleanup::Remove
    } else {
        SourceCleanup::Backup
    };
    let (corrupted, valid) = (source.clone(), row.valid_archive);

    let run = extract(&root, cleanup, move || {
        if !valid {
            std::fs::write(&corrupted, CORRUPT).unwrap();
        }
    });
    drop(lock);
    let result = only_attempt(&run);

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
    assert!(run.cleanup_failures().is_empty(), "{name}");
}

/// Origin: archiveSourceLinkIsNotCleaned. A source that is a link is refused
/// by the pin before anything is read, so neither the link nor its target is
/// cleaned and nothing is extracted.
///
/// Discovery already excludes a linked entry, so the run's source becomes a
/// link only after preflight planned it: the Archive moves to a name no
/// discovery would take for an Archive, and a link to it takes its place.
#[test]
fn a_linked_source_is_not_extracted_or_cleaned() {
    for cleanup in [SourceCleanup::Backup, SourceCleanup::Remove] {
        let root = mod_root(&format!("linked-source-{cleanup:?}"));
        let target = root.join("original.dat");
        let source = root.join("assets.bsa");
        let probe = root.join("symlink-probe");
        if std::os::windows::fs::symlink_file(&target, &probe).is_err() {
            eprintln!("not run: the process cannot create symlinks");
            return;
        }
        std::fs::remove_file(&probe).unwrap();
        write_test_archive(
            &source,
            Game::Sse,
            ArchiveType::Textures,
            false,
            &[(FIXTURE, b"archived bytes".to_vec())],
        );
        let (planned, moved) = (source.clone(), target.clone());

        let run = extract(&root, cleanup, move || {
            std::fs::rename(&planned, &moved).unwrap();
            std::os::windows::fs::symlink_file(&moved, &planned).unwrap();
        });

        let result = only_attempt(&run);
        assert!(result.failure.is_some());
        assert!(source.symlink_metadata().unwrap().is_symlink());
        assert!(target.exists());
        assert!(!root.join(FIXTURE).exists());
        assert!(run.cleanup_failures().is_empty());
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
        // The extracted entry, as extraction would have published it. No run
        // can replace a source its extraction pinned, so this drives the pin
        // alone; whether the scenario is reachable is raised on #595 (#597).
        write(&root, FIXTURE, b"archived bytes");

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

/// Rust-only, through the production wiring: an SSE Apply run extracts a BSA,
/// deletes its source, then packs what extraction published, with the Loose
/// Assets, into a new Archive at the source's own name.
///
/// Extraction and packing see the same Archive state: the reader that held
/// the source mapped while extracting had released it, so the source could be
/// deleted and its name reused, and Archive Finalization's check that the new
/// Archive is listable read the new Archive rather than the old one. One
/// reader served the whole run.
#[test]
fn an_sse_apply_packs_what_it_extracted_into_an_archive_at_the_sources_name() {
    let _serial = serial();
    let app = app_dir("extract-then-pack-sse");
    let mod_root = app.join("mods").join("Mod");
    write(&mod_root, "meshes/loose.nif", &b"loose mesh".repeat(30));
    let mod_root = msvc_canonical(&mod_root).unwrap();
    let archive = mod_root.join("Mod.bsa");
    let archived_mesh = b"archived mesh".repeat(40);
    write_test_archive(
        &archive,
        Game::Sse,
        ArchiveType::Standard,
        true,
        &[("meshes/armor/cuirass.nif", archived_mesh.clone())],
    );

    let mut options = apply_extraction(&app, "SSE", &mod_root, true);
    options.bsa_create = true;
    let run = ApplicationRun::new(&app, "SSE", &options).unwrap();
    let result = run.start(None).unwrap().wait();

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    let [extraction] = result.archive_extraction_attempts() else {
        panic!("one extraction attempt");
    };
    assert_eq!(extraction.mutation, MutationState::Committed);
    let finalization = result.archive_finalization().unwrap();
    assert_eq!(finalization.failure, None, "{}", finalization.detail);
    let [packing] = finalization.attempts.as_slice() else {
        panic!("one packed Archive: {finalization:?}");
    };
    assert!(packing.succeeded(), "{}", packing.detail);
    assert_eq!(packing.archive_path, archive, "the source's name is reused");
    let read = cao_archive::ReadArchive::open(&archive).unwrap().unwrap();
    let mut packed: Vec<_> = read
        .archived_assets()
        .unwrap()
        .into_iter()
        .map(|asset| (asset.name.to_lowercase(), asset.size))
        .collect();
    packed.sort();
    assert_eq!(
        packed,
        [
            (
                r"meshes\armor\cuirass.nif".to_owned(),
                archived_mesh.len() as u64
            ),
            (r"meshes\loose.nif".to_owned(), 300),
        ]
    );
    assert!(
        !mod_root.join("meshes").exists(),
        "packed sources are pruned"
    );
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
