#include "Run/RunSetup.h"

#include <QTest>

#include <algorithm>
#include <variant>

using cao::routing::ExecutionMode;
using cao::routing::AmbiguousArchiveExtension;
using cao::routing::AssetKind;
using cao::routing::AssetOperation;
using cao::routing::AssetRouter;
using cao::routing::MalformedArchiveExtension;
using cao::routing::MalformedArchiveExtensionReason;
using cao::routing::MissingArchiveExtension;
using cao::routing::RequestedWork;
using cao::routing::RoutedAsset;
using cao::routing::UnsupportedDerivedOperation;
using cao::routing::UnsupportedRequestedAssetKind;
using cao::routing::UnsupportedRequestedAssetVariant;
using cao::run::ApplicationRunChoices;
using cao::run::policyValidationErrorMessage;
using cao::run::RunSetup;
using cao::run::SelectedProfileFacts;

namespace
{
/// Reports whether an aggregate setup failure contains one typed conflict alternative.
template<typename Error>
bool containsError(const std::span<const cao::routing::PolicyValidationError> errors)
{
    return std::ranges::any_of(errors, [](const auto &error) {
        return std::holds_alternative<Error>(error);
    });
}
}

class RunSetupTests final : public QObject
{
    Q_OBJECT

private slots:
    /// Verifies a complete valid setup carries every choice and the derived maintenance operation.
    void validSetupCarriesEveryRequestedRoutingFact();
    /// Verifies each named application choice maps to exactly one closed Requested Work value.
    void eachApplicationChoiceMapsToOnlyItsRequestedWork();
    /// Verifies setup returns every extension, Kind, Variant, and derived-operation conflict together.
    void invalidSetupReturnsEveryStructuredConflict();
    /// Verifies GUI and CLI presentation can visit every structured conflict alternative directly.
    void everyConflictVariantHasStructuredPresentation();
    /// Verifies the successfully compiled immutable policy can be copied into the later routing cutover.
    void validPolicyRemainsAvailableForLaterRouting();
    /// Verifies requesting Archive creation under a profile without Archive support is rejected.
    void unsupportedArchiveCreationIsRejected();
};

void RunSetupTests::validSetupCarriesEveryRequestedRoutingFact()
{
    const ApplicationRunChoices choices{
        .executionMode = ExecutionMode::DryRun,
        .optimizeNativeTextures = true,
        .convertTextures = true,
        .optimizeStandardMeshes = true,
        .optimizeTerrainMeshes = true,
        .optimizeAnimations = true,
        .extractArchives = true,
        .createArchives = true,
    };
    const SelectedProfileFacts profile{
        .archiveExtension = ".bsa",
        .supportsNativeTextureOptimization = true,
        .supportsTextureConversion = true,
        .supportsStandardMeshOptimization = true,
        .supportsTerrainMeshOptimization = true,
        .supportsAnimationOptimization = true,
        .supportsArchiveExtraction = true,
        .supportsMeshReferenceMaintenance = true,
        .supportsArchiveCreation = true,
    };

    const auto result = RunSetup::prepare(choices, profile);

    QVERIFY(result.hasPolicy());
    const auto *policy = result.policy();
    QVERIFY(policy != nullptr);
    QCOMPARE(policy->executionMode(), ExecutionMode::DryRun);
    QVERIFY(policy->requests(RequestedWork::NativeTextureOptimization));
    QVERIFY(policy->requests(RequestedWork::ConvertibleTextureConversion));
    QVERIFY(policy->requests(RequestedWork::StandardMeshOptimization));
    QVERIFY(policy->requests(RequestedWork::TerrainMeshOptimization));
    QVERIFY(policy->requests(RequestedWork::AnimationOptimization));
    QVERIFY(policy->requests(RequestedWork::ArchiveExtraction));
    QVERIFY(policy->requests(RequestedWork::ArchiveCreation));
    QVERIFY(policy->maintainsMeshReferences());
}

void RunSetupTests::eachApplicationChoiceMapsToOnlyItsRequestedWork()
{
    constexpr std::array requestedWork{
        RequestedWork::NativeTextureOptimization,
        RequestedWork::ConvertibleTextureConversion,
        RequestedWork::StandardMeshOptimization,
        RequestedWork::TerrainMeshOptimization,
        RequestedWork::AnimationOptimization,
        RequestedWork::ArchiveExtraction,
        RequestedWork::ArchiveCreation,
    };
    const SelectedProfileFacts profile{
        .archiveExtension = ".bsa",
        .supportsNativeTextureOptimization = true,
        .supportsTextureConversion = true,
        .supportsStandardMeshOptimization = true,
        .supportsTerrainMeshOptimization = true,
        .supportsAnimationOptimization = true,
        .supportsArchiveExtraction = true,
        .supportsMeshReferenceMaintenance = true,
        .supportsArchiveCreation = true,
    };

    for (std::size_t selected = 0; selected < requestedWork.size(); ++selected) {
        auto choices = ApplicationRunChoices{};
        bool *choiceFlags[] = {
            &choices.optimizeNativeTextures,
            &choices.convertTextures,
            &choices.optimizeStandardMeshes,
            &choices.optimizeTerrainMeshes,
            &choices.optimizeAnimations,
            &choices.extractArchives,
            &choices.createArchives,
        };
        *choiceFlags[selected] = true;

        const auto result = RunSetup::prepare(choices, profile);

        QVERIFY(result.hasPolicy());
        for (std::size_t work = 0; work < requestedWork.size(); ++work)
            QCOMPARE(result.policy()->requests(requestedWork[work]), work == selected);
    }
}

void RunSetupTests::invalidSetupReturnsEveryStructuredConflict()
{
    const ApplicationRunChoices choices{
        .executionMode = ExecutionMode::Apply,
        .optimizeNativeTextures = true,
        .convertTextures = true,
        .optimizeStandardMeshes = true,
        .optimizeTerrainMeshes = true,
        .optimizeAnimations = true,
        .extractArchives = true,
    };
    const SelectedProfileFacts profile{
        .archiveExtension = std::nullopt,
        .supportsNativeTextureOptimization = true,
        .supportsTextureConversion = false,
        .supportsStandardMeshOptimization = true,
        .supportsTerrainMeshOptimization = false,
        .supportsAnimationOptimization = false,
        .supportsArchiveExtraction = false,
        .supportsMeshReferenceMaintenance = false,
    };

    const auto result = RunSetup::prepare(choices, profile);

    QVERIFY(!result.hasPolicy());
    QCOMPARE(result.errors().size(), std::size_t(6));
    QVERIFY(containsError<MissingArchiveExtension>(result.errors()));
    QVERIFY(containsError<UnsupportedRequestedAssetVariant>(result.errors()));
    QVERIFY(containsError<UnsupportedRequestedAssetKind>(result.errors()));
    QVERIFY(containsError<UnsupportedDerivedOperation>(result.errors()));
    for (const auto &error : result.errors())
        QVERIFY(!policyValidationErrorMessage(error).empty());
}

void RunSetupTests::everyConflictVariantHasStructuredPresentation()
{
    const std::array<cao::routing::PolicyValidationError, 6> errors{
        MissingArchiveExtension{},
        MalformedArchiveExtension{"bsa", MalformedArchiveExtensionReason::MissingLeadingPeriod},
        AmbiguousArchiveExtension{".dds", ".dds", AssetKind::Texture},
        UnsupportedRequestedAssetKind{RequestedWork::AnimationOptimization,
                                      AssetKind::Animation},
        UnsupportedRequestedAssetVariant{RequestedWork::TerrainMeshOptimization,
                                         cao::routing::MeshVariant::Terrain},
        UnsupportedDerivedOperation{RequestedWork::ConvertibleTextureConversion,
                                    AssetOperation::MeshReferenceMaintenance},
    };

    for (const auto &error : errors)
        QVERIFY(!policyValidationErrorMessage(error).empty());
}

void RunSetupTests::validPolicyRemainsAvailableForLaterRouting()
{
    const ApplicationRunChoices choices{
        .executionMode = ExecutionMode::DryRun,
        .optimizeNativeTextures = true,
    };
    const SelectedProfileFacts profile{
        .archiveExtension = ".bsa",
        .supportsNativeTextureOptimization = true,
    };
    const auto setup = RunSetup::prepare(choices, profile);
    QVERIFY(setup.hasPolicy());

    const AssetRouter laterRouter(*setup.policy());
    const auto decision = laterRouter.route("textures/example.dds");

    const auto *asset = std::get_if<RoutedAsset>(&decision);
    QVERIFY(asset != nullptr);
    QCOMPARE(asset->executionMode(), ExecutionMode::DryRun);
    QVERIFY(setup.policy()->requests(RequestedWork::NativeTextureOptimization));
}

void RunSetupTests::unsupportedArchiveCreationIsRejected()
{
    const ApplicationRunChoices choices{
        .executionMode = ExecutionMode::Apply,
        .optimizeNativeTextures = true,
        .createArchives = true,
    };
    // Mirrors a custom profile with bsaEnabled=false driven by a CLI run that passed --bc.
    const SelectedProfileFacts profile{
        .archiveExtension = ".bsa",
        .supportsNativeTextureOptimization = true,
        .supportsArchiveExtraction = false,
        .supportsArchiveCreation = false,
    };

    const auto result = RunSetup::prepare(choices, profile);

    QVERIFY(!result.hasPolicy());
    QCOMPARE(result.errors().size(), std::size_t(1));
    const auto *unsupported = std::get_if<UnsupportedRequestedAssetKind>(&result.errors().front());
    QVERIFY(unsupported != nullptr);
    QCOMPARE(unsupported->request, RequestedWork::ArchiveCreation);
    QCOMPARE(unsupported->kind, AssetKind::Archive);
    QVERIFY(!policyValidationErrorMessage(result.errors().front()).empty());
}

QTEST_MAIN(RunSetupTests)
#include "RunSetupTests.moc"
