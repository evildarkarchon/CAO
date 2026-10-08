//! Asset Routing scenarios, ported from `tests/AssetRoutingTests.cpp`.
//!
//! Each test names its C++ origin. Routing is filename-only, so most fixtures
//! are bare paths; only the filesystem-independence scenario touches disk.

use std::path::{Path, PathBuf};

use cao_core::routing::{
    AssetIdentity, AssetKind, AssetOperation, AssetRouter, AssetVariant, ExecutionMode,
    MalformedArchiveExtensionReason, MeshVariant, OptimizerTarget, PolicyValidationError,
    ProfileCapabilities, ProfileCapability, RequestedWork, RoutedAsset, RoutedAssetPhase,
    RoutingDecision, RoutingPolicy, RoutingPolicyRequest, SkipReason, TextureVariant,
};

/// Every capability except Archive creation, under the `.ba2` Archive extension.
const ALL_ROUTING_CAPABILITIES: &[ProfileCapability] = &[
    ProfileCapability::NativeTextureOptimization,
    ProfileCapability::ConvertibleTextureConversion,
    ProfileCapability::StandardMeshOptimization,
    ProfileCapability::TerrainMeshOptimization,
    ProfileCapability::AnimationOptimization,
    ProfileCapability::ArchiveExtraction,
    ProfileCapability::MeshReferenceMaintenance,
];

/// Compiles a policy the test knows is valid.
fn policy(
    mode: ExecutionMode,
    work: &[RequestedWork],
    capabilities: &[ProfileCapability],
) -> RoutingPolicy {
    RoutingPolicy::compile(
        RoutingPolicyRequest::for_work(mode, work),
        ProfileCapabilities::define(".ba2", capabilities),
    )
    .expect("the scenario's policy is valid")
}

/// The shared all-work Apply router used by the recognition and carried-fact matrices.
fn fully_enabled_router() -> AssetRouter {
    AssetRouter::new(policy(
        ExecutionMode::Apply,
        &[
            RequestedWork::NativeTextureOptimization,
            RequestedWork::ConvertibleTextureConversion,
            RequestedWork::StandardMeshOptimization,
            RequestedWork::TerrainMeshOptimization,
            RequestedWork::AnimationOptimization,
            RequestedWork::ArchiveExtraction,
        ],
        ALL_ROUTING_CAPABILITIES,
    ))
}

fn routed(decision: RoutingDecision) -> RoutedAsset {
    match decision {
        RoutingDecision::Routed(asset) => asset,
        other => panic!("expected a Routed Asset, got {other:?}"),
    }
}

/// Origin: AssetRoutingTests::compilesCompleteRoutingPolicy.
#[test]
fn valid_request_and_capabilities_compile_one_immutable_policy() {
    let work = [
        RequestedWork::NativeTextureOptimization,
        RequestedWork::ConvertibleTextureConversion,
        RequestedWork::StandardMeshOptimization,
        RequestedWork::TerrainMeshOptimization,
        RequestedWork::AnimationOptimization,
        RequestedWork::ArchiveExtraction,
    ];
    let compiled = RoutingPolicy::compile(
        RoutingPolicyRequest::for_work(ExecutionMode::DryRun, &work),
        ProfileCapabilities::define(".Ba2", ALL_ROUTING_CAPABILITIES),
    )
    .expect("complete request compiles");

    assert_eq!(compiled.execution_mode(), ExecutionMode::DryRun);
    for choice in work {
        assert!(compiled.requests(choice), "{choice:?} is requested");
    }
    assert!(!compiled.requests(RequestedWork::ArchiveCreation));
    assert!(compiled.maintains_mesh_references());
    assert_eq!(compiled.archive_extension(), ".ba2");
}

/// Origin: AssetRoutingTests::returnsAllPolicyValidationErrors.
#[test]
fn compilation_reports_every_request_capability_and_derived_conflict_together() {
    let errors = RoutingPolicy::compile(
        RoutingPolicyRequest::for_work(
            ExecutionMode::Apply,
            &[
                RequestedWork::ConvertibleTextureConversion,
                RequestedWork::TerrainMeshOptimization,
                RequestedWork::AnimationOptimization,
            ],
        ),
        ProfileCapabilities::without_archive_extension(&[
            ProfileCapability::NativeTextureOptimization,
            ProfileCapability::StandardMeshOptimization,
        ]),
    )
    .expect_err("the request conflicts with the profile");

    assert_eq!(
        errors,
        vec![
            PolicyValidationError::MissingArchiveExtension,
            PolicyValidationError::UnsupportedRequestedAssetVariant {
                request: RequestedWork::ConvertibleTextureConversion,
                variant: AssetVariant::Texture(TextureVariant::Convertible),
            },
            PolicyValidationError::UnsupportedRequestedAssetVariant {
                request: RequestedWork::TerrainMeshOptimization,
                variant: AssetVariant::Mesh(MeshVariant::Terrain),
            },
            PolicyValidationError::UnsupportedRequestedAssetKind {
                request: RequestedWork::AnimationOptimization,
                kind: AssetKind::Animation,
            },
            PolicyValidationError::UnsupportedDerivedOperation {
                cause: RequestedWork::ConvertibleTextureConversion,
                operation: AssetOperation::MeshReferenceMaintenance,
            },
        ]
    );
}

/// Origin: AssetRoutingTests::profileArchiveExtensionValidation (all six rows).
#[test]
fn invalid_profile_archive_extensions_are_structured_errors() {
    use MalformedArchiveExtensionReason::*;
    let malformed = |extension: &str, reason| PolicyValidationError::MalformedArchiveExtension {
        extension: extension.to_owned(),
        reason,
    };
    let cases = [
        ("", PolicyValidationError::MissingArchiveExtension),
        ("bsa", malformed("bsa", MissingLeadingPeriod)),
        (".", malformed(".", EmptySuffix)),
        (".bs/a", malformed(".bs/a", InvalidCharacter)),
        (".bsa.backup", malformed(".bsa.backup", InvalidCharacter)),
        (
            ".DdS",
            PolicyValidationError::AmbiguousArchiveExtension {
                extension: ".DdS".to_owned(),
                conflicting_extension: ".dds".to_owned(),
                conflicting_kind: AssetKind::Texture,
            },
        ),
    ];
    for (extension, expected) in cases {
        let errors = RoutingPolicy::compile(
            RoutingPolicyRequest::optimize_native_textures(),
            ProfileCapabilities::define(extension, &[ProfileCapability::NativeTextureOptimization]),
        )
        .expect_err("the extension is invalid");
        assert_eq!(errors, vec![expected], "extension {extension:?}");
    }
}

/// Origin: AssetRoutingTests::nativeTextureTracer (all six rows).
#[test]
fn only_a_terminal_dds_extension_routes_a_native_texture_with_the_callers_path() {
    let router = AssetRouter::new(
        RoutingPolicy::compile(
            RoutingPolicyRequest::optimize_native_textures(),
            ProfileCapabilities::define(".bsa", &[ProfileCapability::NativeTextureOptimization]),
        )
        .unwrap(),
    );
    let cases = [
        ("mods/Textures/Armor.DdS", true),
        ("mods/Textures/../Textures/Armor.DdS", true),
        ("mods/textures.dds/readme", false),
        ("mods/Textures/Armor.dds.backup", false),
        ("mods/Textures/DDS", false),
        ("mods/Textures/readme.txt", false),
    ];
    for (path, should_route) in cases {
        let decision = router.route(Path::new(path));
        if !should_route {
            assert_eq!(decision, RoutingDecision::Unsupported, "{path}");
            continue;
        }
        let asset = routed(decision);
        assert_eq!(
            asset.execution_path(),
            Path::new(path),
            "the caller's path is kept unnormalized"
        );
        assert_eq!(
            asset.identity(),
            AssetIdentity::Texture(TextureVariant::Native)
        );
    }
}

/// Origin: AssetRoutingTests::supportedAssetRecognition (all seven rows).
#[test]
fn supported_extensions_map_case_insensitively_to_their_identity() {
    let router = fully_enabled_router();
    let cases = [
        (
            "Root/Textures/Native.DdS",
            AssetIdentity::Texture(TextureVariant::Native),
        ),
        (
            "Root/Textures/Convertible.TgA",
            AssetIdentity::Texture(TextureVariant::Convertible),
        ),
        (
            "Root/Meshes/Standard.NiF",
            AssetIdentity::Mesh(MeshVariant::Standard),
        ),
        (
            "Root/Meshes/Terrain.BtR",
            AssetIdentity::Mesh(MeshVariant::Terrain),
        ),
        (
            "Root/Meshes/Terrain.BtO",
            AssetIdentity::Mesh(MeshVariant::Terrain),
        ),
        ("Root/Animations/Behavior.HkX", AssetIdentity::Animation),
        ("Root/Archives/Assets.Ba2", AssetIdentity::Archive),
    ];
    for (path, identity) in cases {
        let asset = routed(router.route(Path::new(path)));
        assert_eq!(asset.execution_path(), Path::new(path));
        assert_eq!(asset.identity(), identity, "{path}");
        assert_eq!(asset.kind(), identity.kind(), "{path}");
    }
}

/// Origin: AssetRoutingTests::routedAssetFacts (all six rows).
#[test]
fn routed_assets_carry_phase_target_mode_and_closed_operations() {
    use AssetOperation::*;
    let router = fully_enabled_router();
    let cases: [(&str, RoutedAssetPhase, OptimizerTarget, &[AssetOperation]); 6] = [
        (
            "Textures/Native.dds",
            RoutedAssetPhase::LooseAssetProcessing,
            OptimizerTarget::Texture,
            &[Optimization],
        ),
        (
            "Textures/Convertible.tga",
            RoutedAssetPhase::LooseAssetProcessing,
            OptimizerTarget::Texture,
            &[Conversion],
        ),
        (
            "Meshes/Standard.nif",
            RoutedAssetPhase::LooseAssetProcessing,
            OptimizerTarget::Mesh,
            &[Optimization, MeshReferenceMaintenance],
        ),
        (
            "Meshes/Terrain.btr",
            RoutedAssetPhase::LooseAssetProcessing,
            OptimizerTarget::Mesh,
            &[Optimization, MeshReferenceMaintenance],
        ),
        (
            "Animations/Behavior.hkx",
            RoutedAssetPhase::LooseAssetProcessing,
            OptimizerTarget::Animation,
            &[Optimization],
        ),
        (
            "Archives/Assets.ba2",
            RoutedAssetPhase::ArchiveExtraction,
            OptimizerTarget::Archive,
            &[Extraction],
        ),
    ];
    for (path, phase, target, operations) in cases {
        let asset = routed(router.route(Path::new(path)));
        assert_eq!(asset.phase(), phase, "{path}");
        assert_eq!(asset.target(), target, "{path}");
        assert_eq!(asset.execution_mode(), ExecutionMode::Apply, "{path}");
        for operation in [
            Extraction,
            Optimization,
            Conversion,
            MeshReferenceMaintenance,
        ] {
            assert_eq!(
                asset.operations().contains(operation),
                operations.contains(&operation),
                "{path}: {operation:?}"
            );
        }
    }
}

/// Origin: AssetRoutingTests::skipReasonPrecedence (all seven rows).
#[test]
fn recognized_assets_are_skipped_with_the_highest_precedence_reason() {
    let cases: [(ExecutionMode, &[RequestedWork], &str, SkipReason); 7] = [
        (
            ExecutionMode::DryRun,
            &[],
            "Archives/Assets.ba2",
            SkipReason::DisabledPhase,
        ),
        (
            ExecutionMode::Apply,
            &[],
            "Textures/Native.dds",
            SkipReason::DisabledAssetKind,
        ),
        (
            ExecutionMode::Apply,
            &[RequestedWork::ConvertibleTextureConversion],
            "Textures/Native.dds",
            SkipReason::ExcludedAssetVariant,
        ),
        (
            ExecutionMode::Apply,
            &[RequestedWork::NativeTextureOptimization],
            "Textures/Convertible.tga",
            SkipReason::ExcludedAssetVariant,
        ),
        (
            ExecutionMode::Apply,
            &[RequestedWork::StandardMeshOptimization],
            "Meshes/Terrain.bto",
            SkipReason::ExcludedAssetVariant,
        ),
        (
            ExecutionMode::Apply,
            &[],
            "Animations/Behavior.hkx",
            SkipReason::DisabledAssetKind,
        ),
        (
            ExecutionMode::Apply,
            &[],
            "Archives/Assets.ba2",
            SkipReason::DisabledAssetKind,
        ),
    ];
    for (mode, work, path, reason) in cases {
        let router = AssetRouter::new(policy(mode, work, ALL_ROUTING_CAPABILITIES));
        match router.route(Path::new(path)) {
            RoutingDecision::Skipped(skipped) => {
                assert_eq!(skipped.execution_path(), Path::new(path));
                assert_eq!(skipped.reason(), reason, "{path} under {mode:?} {work:?}");
            }
            other => panic!("{path} should be skipped, got {other:?}"),
        }
    }
}

/// Origin: AssetRoutingTests::conversionOnlyRoutesTextureAndMaintainsMeshes.
#[test]
fn conversion_only_work_routes_tga_and_maintains_both_mesh_variants() {
    let router = AssetRouter::new(policy(
        ExecutionMode::Apply,
        &[RequestedWork::ConvertibleTextureConversion],
        &[
            ProfileCapability::ConvertibleTextureConversion,
            ProfileCapability::MeshReferenceMaintenance,
        ],
    ));
    for path in [
        "Textures/Convertible.tga",
        "Meshes/Standard.nif",
        "Meshes/Terrain.btr",
        "Meshes/Terrain.bto",
    ] {
        let asset = routed(router.route(Path::new(path)));
        assert_eq!(asset.execution_path(), Path::new(path));
        let operations = asset.operations();
        if asset.kind() == AssetKind::Texture {
            assert!(operations.contains(AssetOperation::Conversion));
            assert!(!operations.contains(AssetOperation::Optimization));
            assert!(!operations.contains(AssetOperation::MeshReferenceMaintenance));
        } else {
            assert_eq!(asset.kind(), AssetKind::Mesh);
            assert!(operations.contains(AssetOperation::MeshReferenceMaintenance));
            assert!(!operations.contains(AssetOperation::Optimization));
        }
    }
}

/// Origin: AssetRoutingTests::dryRunPreservesLooseAssetOperations.
#[test]
fn dry_run_keeps_loose_asset_operations_but_disables_archive_extraction() {
    let router = AssetRouter::new(policy(
        ExecutionMode::DryRun,
        &[
            RequestedWork::ConvertibleTextureConversion,
            RequestedWork::StandardMeshOptimization,
            RequestedWork::ArchiveExtraction,
        ],
        &[
            ProfileCapability::ConvertibleTextureConversion,
            ProfileCapability::StandardMeshOptimization,
            ProfileCapability::ArchiveExtraction,
            ProfileCapability::MeshReferenceMaintenance,
        ],
    ));

    let texture = routed(router.route(Path::new("Textures/Source.tga")));
    assert_eq!(texture.execution_mode(), ExecutionMode::DryRun);
    assert!(texture.operations().contains(AssetOperation::Conversion));

    let mesh = routed(router.route(Path::new("Meshes/Model.nif")));
    assert_eq!(mesh.execution_mode(), ExecutionMode::DryRun);
    assert!(mesh.operations().contains(AssetOperation::Optimization));
    assert!(
        mesh.operations()
            .contains(AssetOperation::MeshReferenceMaintenance)
    );

    match router.route(Path::new("Archives/Assets.ba2")) {
        RoutingDecision::Skipped(archive) => {
            assert_eq!(archive.reason(), SkipReason::DisabledPhase)
        }
        other => panic!("a Dry Run never extracts Archives, got {other:?}"),
    }
}

/// Origin: AssetRoutingTests::routingIgnoresFilesystemState.
#[test]
fn routing_neither_depends_on_nor_mutates_the_filesystem() {
    let router = AssetRouter::new(
        RoutingPolicy::compile(
            RoutingPolicyRequest::optimize_native_textures(),
            ProfileCapabilities::define(".ba2", &[ProfileCapability::NativeTextureOptimization]),
        )
        .unwrap(),
    );
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("cao-core")
        .join("routing-filesystem-state");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("NotReallyATexture.dds");
    // A leftover from an earlier run would make the "missing" decision vacuous;
    // a missing file is the expected case.
    let _ = std::fs::remove_file(&path);

    let missing = routed(router.route(&path));
    std::fs::write(&path, b"arbitrary non-DDS contents").unwrap();
    let existing = routed(router.route(&path));

    assert_eq!(missing, existing);
    assert!(existing.operations().contains(AssetOperation::Optimization));
    assert_eq!(std::fs::read(&path).unwrap(), b"arbitrary non-DDS contents");
    std::fs::remove_dir_all(&directory).unwrap();
}

/// Origin: AssetRoutingTests::batchRoutingOwnsRoutedAssetsInInputOrder.
#[test]
fn batch_routing_owns_routed_assets_in_input_order_with_duplicates() {
    let router = fully_enabled_router();
    let ledger = {
        let borrowed = vec![
            PathBuf::from("Textures/First.dds"),
            PathBuf::from("Meshes/Second.nif"),
            PathBuf::from("Textures/First.dds"),
        ];
        router.route_all(&borrowed)
    };

    let paths: Vec<_> = ledger
        .routed_assets()
        .iter()
        .map(RoutedAsset::execution_path)
        .collect();
    assert_eq!(
        paths,
        [
            Path::new("Textures/First.dds"),
            Path::new("Meshes/Second.nif"),
            Path::new("Textures/First.dds")
        ]
    );
}

/// Origin: AssetRoutingTests::batchRoutingOmitsUnsupportedPathsAndCountsSkips.
#[test]
fn batch_routing_omits_unsupported_paths_and_counts_each_skip_reason() {
    let router = AssetRouter::new(policy(
        ExecutionMode::DryRun,
        &[
            RequestedWork::ConvertibleTextureConversion,
            RequestedWork::ArchiveExtraction,
        ],
        &[
            ProfileCapability::ConvertibleTextureConversion,
            ProfileCapability::ArchiveExtraction,
            ProfileCapability::MeshReferenceMaintenance,
        ],
    ));
    let paths = [
        "Textures/Routed.tga",
        "Docs/Unsupported.txt",
        "Textures/Excluded.dds",
        "Animations/Disabled.hkx",
        "Archives/Disabled.ba2",
        "Textures/Routed.tga",
    ];

    let ledger = router.route_all(paths);

    let routed: Vec<_> = ledger
        .routed_assets()
        .iter()
        .map(RoutedAsset::execution_path)
        .collect();
    assert_eq!(routed, [Path::new(paths[0]), Path::new(paths[5])]);
    assert_eq!(ledger.skipped_asset_count(SkipReason::DisabledPhase), 1);
    assert_eq!(ledger.skipped_asset_count(SkipReason::DisabledAssetKind), 1);
    assert_eq!(
        ledger.skipped_asset_count(SkipReason::ExcludedAssetVariant),
        1
    );
}

/// Origin: AssetRoutingTests::ledgerQueriesRoutedAssetsByPhaseAndTarget.
#[test]
fn ledger_queries_by_phase_and_target_keep_relative_order() {
    let router = fully_enabled_router();
    let paths = [
        "Archives/First.ba2",
        "Textures/Repeated.dds",
        "Meshes/Middle.nif",
        "Textures/Repeated.dds",
        "Archives/Last.ba2",
    ];
    let ledger = router.route_all(paths);
    let names = |assets: Vec<&RoutedAsset>| -> Vec<PathBuf> {
        assets
            .into_iter()
            .map(|asset| asset.execution_path().to_path_buf())
            .collect()
    };
    let expect = |indices: &[usize]| -> Vec<PathBuf> {
        indices.iter().map(|i| PathBuf::from(paths[*i])).collect()
    };

    assert_eq!(
        names(ledger.routed_assets_in_phase(RoutedAssetPhase::LooseAssetProcessing)),
        expect(&[1, 2, 3])
    );
    assert_eq!(
        names(ledger.routed_assets_in_phase(RoutedAssetPhase::ArchiveExtraction)),
        expect(&[0, 4])
    );
    assert_eq!(
        names(ledger.routed_assets_for(OptimizerTarget::Texture)),
        expect(&[1, 3])
    );
    assert_eq!(
        names(ledger.routed_assets_for(OptimizerTarget::Mesh)),
        expect(&[2])
    );
    assert!(
        ledger
            .routed_assets_for(OptimizerTarget::Animation)
            .is_empty()
    );
}

/// Origin: AssetRoutingTests::ledgerWorkTotalCountsRoutedAssetsNotOperations.
#[test]
fn one_routed_asset_is_one_work_entry_whatever_its_operations() {
    let router = AssetRouter::new(policy(
        ExecutionMode::Apply,
        &[
            RequestedWork::ConvertibleTextureConversion,
            RequestedWork::StandardMeshOptimization,
        ],
        &[
            ProfileCapability::ConvertibleTextureConversion,
            ProfileCapability::StandardMeshOptimization,
            ProfileCapability::MeshReferenceMaintenance,
        ],
    ));

    let ledger = router.route_all(["Meshes/BothOperations.nif"]);

    assert_eq!(ledger.routed_assets().len(), 1);
    let operations = ledger.routed_assets()[0].operations();
    assert!(operations.contains(AssetOperation::Optimization));
    assert!(operations.contains(AssetOperation::MeshReferenceMaintenance));
}

/// Origin: AssetRoutingTests::batchRoutingMatchesSingleAssetDecisions.
#[test]
fn batch_routing_matches_single_path_decisions_for_every_disposition() {
    let router = AssetRouter::new(policy(
        ExecutionMode::Apply,
        &[RequestedWork::ConvertibleTextureConversion],
        &[
            ProfileCapability::ConvertibleTextureConversion,
            ProfileCapability::MeshReferenceMaintenance,
        ],
    ));
    let paths = [
        "Textures/Routed.tga",
        "Textures/Skipped.dds",
        "Docs/Unsupported.txt",
    ];

    let single_routed = routed(router.route(Path::new(paths[0])));
    let RoutingDecision::Skipped(single_skipped) = router.route(Path::new(paths[1])) else {
        panic!("the native Texture is excluded");
    };
    assert_eq!(
        router.route(Path::new(paths[2])),
        RoutingDecision::Unsupported
    );
    let ledger = router.route_all(paths);

    assert_eq!(ledger.routed_assets(), [single_routed]);
    assert_eq!(ledger.skipped_asset_count(single_skipped.reason()), 1);
}

/// Origin: AssetRoutingTests::emptyBatchProducesEmptyLedger.
#[test]
fn an_empty_batch_produces_an_empty_ledger() {
    let ledger = fully_enabled_router().route_all(Vec::<PathBuf>::new());

    assert!(ledger.routed_assets().is_empty());
    for reason in SkipReason::ALL {
        assert_eq!(ledger.skipped_asset_count(reason), 0);
    }
}
