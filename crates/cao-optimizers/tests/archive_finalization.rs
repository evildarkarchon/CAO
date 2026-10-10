//! Archive Finalization against real Archives: core's phase over
//! [`GameArchivePacker`], [`ArchiveFileReader`] and each game's real rules,
//! and the production wiring that runs it.
//!
//! These are the real-archive cases of `tests/ArchiveFinalizationTests.cpp`,
//! each naming its origin: Loading Plugin names per game, Dummy Plugin bytes,
//! the capacity estimate against what is really published, and FO4's DX10
//! Textures BA2 (deviation 21). Every other scenario is ported in `cao-core`
//! against a fake packer.

mod common;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use cao_archive::{ArchiveFormat, Fo4Container, Game, ReadArchive, Settings};
use cao_core::execution::MutationState;
use cao_core::routing::{ExecutionMode, RequestedWork, RoutingPolicyRequest};
use cao_core::run::{
    ArchiveFinalization, ArchiveFinalizationMutationKind, ArchiveFinalizationResult,
    ArchiveFinalizationSettings, ArchiveNameKind, ArchivePacker, ArchivePrecedence, CapacityProbe,
    MutableRunEvidence, MutationKind, RunConfiguration, RunOutcome, RunPhase, RunPhaseRecord,
    RunPreparation, RunWorkEvidence, SelectedProfileFacts, TemporaryArtifactRegistry,
    create_run_id,
};
use cao_optimizers::archives::{ArchiveFileReader, GameArchivePacker, VolumeProbes};
use cao_optimizers::composition::ApplicationRun;
use common::{app_dir, profile_options, scratch_dir, serial, write, write_dds};
use directxtex::DXGI_FORMAT_R8G8B8A8_UNORM;

/// A capacity probe answering the same for every Mod Root.
struct Capacity(Option<u64>);

impl CapacityProbe for Capacity {
    fn available_bytes(&self, _: &Path) -> Option<u64> {
        self.0
    }
}

/// The canonical form of a directory, as Preparing resolves it.
fn canonical(path: &Path) -> PathBuf {
    cao_winfs::msvc_canonical(path).unwrap()
}

/// A fresh, canonical Mod Root named `name` for one scenario.
fn mod_root(scenario: &str, name: &str) -> PathBuf {
    let root = canonical(&scratch_dir(&format!("finalization-{scenario}"))).join(name);
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// An Apply preparation over `roots` that requests only Archive creation.
fn preparation(roots: &[PathBuf], extension: &str) -> RunPreparation {
    let configuration = RunConfiguration {
        profile: SelectedProfileFacts {
            archive_extension: Some(extension.to_owned()),
            supports_archive_creation: true,
            ..SelectedProfileFacts::default()
        },
        ignored_mods: Vec::new(),
        separator_suffixes: Vec::new(),
    };
    let policy = configuration
        .profile
        .compile_policy(RoutingPolicyRequest::for_work(
            ExecutionMode::Apply,
            &[RequestedWork::ArchiveCreation],
        ))
        .unwrap();
    RunPreparation::new(
        roots.to_vec(),
        configuration,
        policy,
        ArchivePrecedence::DeterministicDiscovery,
    )
}

/// Runs Archive Finalization once over `root` with `game`'s real rules and
/// returns its recorded result. Safety Cleanup must find nothing left.
fn finalize(
    game: Game,
    root: &Path,
    settings: ArchiveFinalizationSettings,
    capacity: Option<u64>,
) -> ArchiveFinalizationResult {
    finalize_probing(game, root, settings, &Capacity(capacity))
}

/// [`finalize`] with any capacity probe.
fn finalize_probing(
    game: Game,
    root: &Path,
    settings: ArchiveFinalizationSettings,
    capacity: &dyn CapacityProbe,
) -> ArchiveFinalizationResult {
    let packer = GameArchivePacker::new(Settings::get(game));
    let reader = ArchiveFileReader::default();
    let mut evidence = MutableRunEvidence::new(None);
    for phase in [RunPhase::Preparing, RunPhase::ArchiveFinalization] {
        evidence
            .record_phase(RunPhaseRecord::executed(phase, None))
            .unwrap();
    }
    let evidence = RefCell::new(evidence);
    let mut artifacts = TemporaryArtifactRegistry::new(create_run_id());
    ArchiveFinalization::new(&packer, &reader, capacity, &VolumeProbes, settings, &[])
        .run(
            &preparation(&[root.to_path_buf()], packer.rules().extension.as_str()),
            &RunWorkEvidence::new(&evidence),
            &mut artifacts,
            &|| false,
        )
        .unwrap();
    assert!(artifacts.cleanup().is_empty());
    evidence
        .into_inner()
        .archive_finalization()
        .unwrap()
        .clone()
}

/// The canonical Dummy Plugin of `game`.
fn dummy(game: Game) -> Vec<u8> {
    Settings::get(game).dummy_plugin.to_vec()
}

/// The size of the file at `path`.
fn size(path: &Path) -> u64 {
    std::fs::metadata(path).unwrap().len()
}

/// Opens a real Archive this phase wrote.
fn open(archive: &Path) -> ReadArchive {
    ReadArchive::open(archive)
        .unwrap()
        .expect("a known Archive format")
}

/// Sets `path`'s modification time a day back and returns it.
fn age(path: &Path) -> SystemTime {
    let earlier = SystemTime::now() - Duration::from_secs(24 * 60 * 60);
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(earlier)
        .unwrap();
    std::fs::metadata(path).unwrap().modified().unwrap()
}

/// Origin: finalizationCapacityEstimateCoversPublishedOutput, for each game.
/// The estimate a shortage reports is never less than the Archive and Dummy
/// Plugin really published.
#[test]
fn the_capacity_estimate_covers_the_published_archive_and_plugin() {
    for (game, archive) in [
        (Game::Tes5, "mod.bsa"),
        (Game::Sse, "mod.bsa"),
        (Game::Fo4, "mod - Main.ba2"),
    ] {
        let root = mod_root(&format!("estimate-{game:?}"), "mod");
        write(&root, "meshes/asset.nif", &[b'x'; 8192]);
        let settings = ArchiveFinalizationSettings::default();

        let rejected = finalize(game, &root, settings, Some(0));
        let detail = &rejected.attempts[0].detail;
        let estimate: u64 = detail
            .split_once("estimated ")
            .unwrap()
            .1
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .unwrap();

        let result = finalize(game, &root, settings, None);
        assert_eq!(result.attempts.len(), 1);
        let attempt = &result.attempts[0];
        assert!(attempt.succeeded(), "{game:?}: {}", attempt.detail);
        assert_eq!(attempt.archive_path, root.join(archive));
        let published = size(&attempt.archive_path) + size(&root.join("mod.esp"));
        assert!(estimate >= published, "{game:?}: {estimate} < {published}");
    }
}

/// Origin: existingArchivePluginNamesFollowProfile (all four rows). An
/// existing Archive gets the selected game's canonical Dummy Plugin at its
/// suffix-free name.
#[test]
fn existing_archive_plugin_names_follow_the_profile() {
    for (game, archive) in [
        (Game::Tes5, "bundle.bsa"),
        (Game::Sse, "bundle - Textures.bsa"),
        (Game::Fo4, "bundle - Main.ba2"),
        (Game::Fo4, "bundle - Textures.ba2"),
    ] {
        let root = mod_root(&format!("existing-names-{archive}"), "mod");
        write(&root, archive, b"retained archive");

        let result = finalize(game, &root, ArchiveFinalizationSettings::default(), None);

        assert!(result.attempts.is_empty());
        assert_eq!(result.failure, None, "{archive}: {}", result.detail);
        assert_eq!(result.mutations.len(), 1, "{archive}");
        let plugin = root.join("bundle.esp");
        let mutation = &result.mutations[0];
        assert_eq!(mutation.mod_root, root);
        assert_eq!(mutation.path, plugin);
        assert_eq!(
            mutation.kind,
            ArchiveFinalizationMutationKind::PluginCreation
        );
        assert_eq!(mutation.mutation, MutationState::Committed);
        assert_eq!(std::fs::read(&plugin).unwrap(), dummy(game), "{archive}");
        // Only the durable ownership controls stay; Safety Cleanup removed
        // the run's staging child.
        for entry in std::fs::read_dir(root.join(".cao-staging")).unwrap() {
            assert!(!entry.unwrap().file_type().unwrap().is_dir());
        }
    }
}

/// Origin: existingArchivesShareLoadingPlugin (all three rows). FO4's Main
/// and Textures Archives share one Loading Plugin: it is created once, and
/// an existing full plugin or exact dummy is kept as it is.
#[test]
fn existing_fo4_archives_share_one_loading_plugin() {
    for preexisting in ["none", "full-plugin", "exact-dummy"] {
        let root = mod_root(&format!("shared-plugin-{preexisting}"), "mod");
        write(&root, "bundle - Main.ba2", b"retained main archive");
        write(&root, "bundle - Textures.ba2", b"retained texture archive");
        let dummy_path = root.join("bundle.esp");
        let full = root.join("bundle.esm");
        let (original, bytes) = match preexisting {
            "full-plugin" => (full.clone(), b"full loading plugin".to_vec()),
            _ => (dummy_path.clone(), dummy(Game::Fo4)),
        };
        if preexisting != "none" {
            std::fs::write(&original, &bytes).unwrap();
        }

        let result = finalize(
            Game::Fo4,
            &root,
            ArchiveFinalizationSettings::default(),
            None,
        );

        assert!(result.attempts.is_empty());
        assert_eq!(result.failure, None, "{preexisting}: {}", result.detail);
        assert!(result.safe_to_continue);
        if preexisting == "none" {
            assert_eq!(result.mutations.len(), 1);
            assert_eq!(result.mutations[0].path, dummy_path);
            assert_eq!(result.mutations[0].mutation, MutationState::Committed);
        } else {
            assert!(result.mutations.is_empty(), "{preexisting}");
        }
        assert_eq!(std::fs::read(&original).unwrap(), bytes, "{preexisting}");
        if preexisting == "full-plugin" {
            assert!(!dummy_path.exists());
        }
    }
}

/// Origin: plannedOutputNamesUseExactDummyBytes (all three rows). Output
/// names come from a same-size full plugin, never from an exact Dummy
/// Plugin, and that full plugin loads the output, so no dummy is published.
#[test]
fn output_names_come_from_full_plugins_not_exact_dummies() {
    for (game, archive) in [
        (Game::Tes5, "loader.bsa"),
        (Game::Sse, "loader - Textures.bsa"),
        (Game::Fo4, "loader - Textures.ba2"),
    ] {
        let root = mod_root(&format!("exact-dummy-names-{game:?}"), "mod");
        // An FO4 Textures BA2 parses its DDS sources, so every game gets a
        // real Texture.
        write_dds(
            &root.join("textures/asset.dds"),
            DXGI_FORMAT_R8G8B8A8_UNORM,
            4,
        );
        write(&root, "dummy.esp", &dummy(game));
        write(&root, "loader.esm", &vec![0u8; dummy(game).len()]);
        let settings = ArchiveFinalizationSettings {
            compress: false,
            ..ArchiveFinalizationSettings::default()
        };

        let result = finalize(game, &root, settings, None);

        assert_eq!(result.attempts.len(), 1);
        let attempt = &result.attempts[0];
        assert!(attempt.succeeded(), "{game:?}: {}", attempt.detail);
        assert_eq!(attempt.archive_path, root.join(archive));
        assert!(
            result
                .mutations
                .iter()
                .all(|mutation| mutation.kind != ArchiveFinalizationMutationKind::PluginCreation)
        );
        assert!(!root.join("loader.esp").exists());
    }
}

/// Origin: plannedOutputRechecksLoadingPluginNames (all three rows). A plugin
/// that appears after planning at another recognized name loads the output,
/// so no Dummy Plugin is published and the plugin is left untouched.
#[test]
fn a_plugin_appearing_at_another_recognized_name_is_reused() {
    for (game, plugin_name) in [
        (Game::Tes5, "mod.esm"),
        (Game::Sse, "mod - Textures.esl"),
        (Game::Fo4, "mod.esm"),
    ] {
        let root = mod_root(&format!("recheck-names-{game:?}"), "mod");
        let source = if game == Game::Fo4 {
            "meshes/asset.nif"
        } else {
            "textures/asset.dds"
        };
        write(&root, source, b"source bytes");
        let alternate = root.join(plugin_name);
        let appeared = std::sync::Mutex::new(None);
        struct Appearing<'a> {
            plugin: &'a Path,
            appeared: &'a std::sync::Mutex<Option<SystemTime>>,
        }
        impl CapacityProbe for Appearing<'_> {
            fn available_bytes(&self, _: &Path) -> Option<u64> {
                // Planning found no Loading Plugin; one appears now.
                let mut appeared = self.appeared.lock().unwrap();
                if appeared.is_none() {
                    std::fs::write(self.plugin, b"existing loading plugin").unwrap();
                    *appeared = Some(age(self.plugin));
                }
                None
            }
        }
        let settings = ArchiveFinalizationSettings {
            compress: false,
            ..ArchiveFinalizationSettings::default()
        };

        let result = finalize_probing(
            game,
            &root,
            settings,
            &Appearing {
                plugin: &alternate,
                appeared: &appeared,
            },
        );

        let original = appeared.lock().unwrap().expect("the plugin appeared");
        assert_eq!(result.attempts.len(), 1);
        let attempt = &result.attempts[0];
        assert!(attempt.succeeded(), "{game:?}: {}", attempt.detail);
        assert_eq!(attempt.mutation, MutationState::Committed);
        assert!(
            result
                .mutations
                .iter()
                .all(|mutation| mutation.kind != ArchiveFinalizationMutationKind::PluginCreation)
        );
        assert!(open(&attempt.archive_path).archived_assets().is_ok());
        assert!(!root.join(source).exists());
        // The planned fallback name stays free.
        assert!(!root.join("mod.esp").exists());
        assert_eq!(
            std::fs::metadata(&alternate).unwrap().modified().unwrap(),
            original
        );
    }
}

/// **Deviation 21**, with the real FO4 rules. Textures always go into their
/// own compressed DX10 Textures BA2, even with merging asked for and
/// compression off; the Main BA2 holds only the other files, uncompressed.
#[test]
fn deviation_21_fo4_textures_always_get_a_compressed_dx10_ba2() {
    let root = mod_root("deviation-21", "mod");
    write_dds(
        &root.join("textures/asset.dds"),
        DXGI_FORMAT_R8G8B8A8_UNORM,
        64,
    );
    write(&root, "meshes/asset.nif", &b"NiTriShape ".repeat(200));
    let settings = ArchiveFinalizationSettings {
        compress: false,
        merge_textures: true,
        ..ArchiveFinalizationSettings::default()
    };

    let result = finalize(Game::Fo4, &root, settings, None);

    assert_eq!(result.attempts.len(), 2);
    assert!(result.attempts.iter().all(|attempt| attempt.succeeded()));
    let main = open(&root.join("mod - Main.ba2"));
    assert_eq!(main.header().format, ArchiveFormat::Fo4);
    assert_eq!(main.header().container, Some(Fo4Container::General));
    let main_assets = main.archived_assets().unwrap();
    assert_eq!(main_assets.len(), 1);
    assert!(main_assets[0].name.to_lowercase().ends_with(".nif"));
    assert!(!main_assets[0].compressed);
    let textures = open(&root.join("mod - Textures.ba2"));
    assert_eq!(textures.header().container, Some(Fo4Container::Dx10));
    let texture_assets = textures.archived_assets().unwrap();
    assert_eq!(texture_assets.len(), 1);
    assert!(texture_assets[0].compressed);
    assert_eq!(
        std::fs::read(root.join("mod.esp")).unwrap(),
        dummy(Game::Fo4)
    );
}

/// Quarantined files are never packed: `.caobad` and `.caobad.N` match no
/// game's classification rule, so they stay loose beside the new Archive and
/// are never deleted as packed sources.
#[test]
fn quarantined_files_are_never_packed() {
    for game in [Game::Tes5, Game::Sse, Game::Fo4] {
        let root = mod_root(&format!("quarantine-{game:?}"), "mod");
        write(&root, "meshes/good.nif", b"packed mesh bytes");
        write(&root, "meshes/broken.nif.caobad", b"quarantined mesh bytes");
        write(
            &root,
            "textures/broken.dds.caobad.1",
            b"quarantined texture bytes",
        );

        let result = finalize(game, &root, ArchiveFinalizationSettings::default(), None);

        assert_eq!(result.attempts.len(), 1, "{game:?}");
        let attempt = &result.attempts[0];
        assert!(attempt.succeeded(), "{game:?}: {}", attempt.detail);
        let names: Vec<_> = open(&attempt.archive_path)
            .archived_assets()
            .unwrap()
            .into_iter()
            .map(|asset| asset.name.to_lowercase().replace('/', "\\"))
            .collect();
        assert_eq!(names, [r"meshes\good.nif"], "{game:?}");
        assert!(!root.join("meshes/good.nif").exists());
        assert!(root.join("meshes/broken.nif.caobad").exists());
        assert!(root.join("textures/broken.dds.caobad.1").exists());
    }
}

/// The names core builds render exactly as `cao-archive`'s `FilePath` does,
/// counters, suffixes and all-digit stems included.
#[test]
fn core_names_render_as_file_path_does() {
    let root = mod_root("name-rendering", "mod");
    for name in [
        "Plain.esp",
        "Counted12.esm",
        "Suffixed - Textures.esl",
        "Suffixed - Textures3.esp",
        "Main7 - Main.esp",
        "2.esp",
        "007.esm",
        "Other - Words.esp",
    ] {
        write(&root, name, b"plugin");
    }
    for game in [Game::Tes5, Game::Sse, Game::Fo4] {
        let settings = Settings::get(game);
        let packer = GameArchivePacker::new(settings);
        let mut core: Vec<_> = packer
            .list_names(&root, ArchiveNameKind::Plugin)
            .unwrap()
            .into_iter()
            .map(|name| name.full_path())
            .collect();
        let mut archive: Vec<_> = cao_archive::list_plugins(&root, &settings)
            .unwrap()
            .into_iter()
            .map(|name| name.full_path())
            .collect();
        core.sort();
        archive.sort();
        assert_eq!(core, archive, "{game:?}");
        assert!(!core.is_empty());
    }
}

/// Rust-only, through the production wiring: an SSE Apply run that requests
/// Archive creation packs the Loose Assets into new Archives with a Dummy
/// Plugin, keeps the profile's Packing Exclusions loose, deletes the packed
/// sources and prunes the emptied folders.
#[test]
fn an_sse_apply_packs_loose_assets_into_loadable_archives() {
    let _serial = serial();
    let app = app_dir("finalization-sse-apply");
    let root = canonical(&{
        let root = app.join("mods").join("Mod");
        std::fs::create_dir_all(&root).unwrap();
        root
    });
    write(
        &root,
        "meshes/armor/cuirass.nif",
        &b"NiTriShape ".repeat(300),
    );
    write(
        &root,
        "textures/armor/cuirass.dds",
        &b"texture bytes".repeat(50),
    );
    write(&root, "sound/fx/hit.wav", &b"RIFF wave ".repeat(30));
    // The shipped SSE `FilesToNotPack.txt` keeps behaviour files loose for
    // FNIS and Nemesis.
    let excluded = root.join("meshes/actors/character/behaviors/0_master.hkx");
    write(
        &root,
        "meshes/actors/character/behaviors/0_master.hkx",
        b"behavior",
    );

    let mut options = profile_options(&app, "SSE");
    options.dry_run = false;
    options.user_path = root.to_string_lossy().into_owned();
    options.textures_necessary = false;
    options.textures_compress = false;
    options.textures_mipmaps = false;
    options.textures_resize_ratio = false;
    options.textures_resize_size = false;
    options.meshes_optimization_level = 0;
    options.meshes_resave = false;
    options.animations_optimization = false;
    options.bsa_extract = false;
    options.bsa_create = true;
    // The options model's default; the shipped `settings.ini` lacks the key.
    options.bsa_merge_incompressible = true;
    let run = ApplicationRun::new(&app, "SSE", &options).unwrap();
    let result = run.start(None).unwrap().wait();

    assert_eq!(result.outcome(), RunOutcome::Succeeded, "{result:?}");
    let finalization = result.archive_finalization().unwrap();
    let archives: Vec<_> = finalization
        .attempts
        .iter()
        .map(|attempt| attempt.archive_path.clone())
        .collect();
    // SSE merges Incompressible files into the Standard Archive by default and
    // keeps Textures apart.
    assert_eq!(
        archives,
        [root.join("Mod.bsa"), root.join("Mod - Textures.bsa")]
    );
    let mut packed: Vec<_> = archives
        .iter()
        .flat_map(|archive| open(archive).archived_assets().unwrap())
        .map(|asset| asset.name.to_lowercase())
        .collect();
    packed.sort();
    assert_eq!(
        packed,
        [
            r"meshes\armor\cuirass.nif",
            r"sound\fx\hit.wav",
            r"textures\armor\cuirass.dds",
        ]
    );
    assert_eq!(
        std::fs::read(root.join("Mod.esp")).unwrap(),
        dummy(Game::Sse)
    );
    assert!(excluded.exists());
    for packed_dir in ["textures", "sound", "meshes/armor"] {
        assert!(
            !root.join(packed_dir).exists(),
            "{packed_dir} was not pruned"
        );
    }
    let pruned = finalization
        .mutations
        .iter()
        .find(|mutation| mutation.kind == ArchiveFinalizationMutationKind::EmptyDirectoryPruning)
        .unwrap();
    // textures/armor, textures, sound/fx, sound and meshes/armor.
    assert_eq!(pruned.count, 5);
    let summary = result
        .mutation_summaries()
        .iter()
        .find(|summary| summary.kind == MutationKind::ArchiveFinalization)
        .unwrap();
    // Two Archives, one Dummy Plugin and five pruned directories.
    assert_eq!(summary.committed, 2 + 1 + 5);
}
