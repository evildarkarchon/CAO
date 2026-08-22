#include "Run/ArchiveFirstAssetDiscovery.h"

#include <QtTest>

#include <btu/bsa/archive_data.hpp>
#include <btu/bsa/pack.hpp>
#include <btu/bsa/settings.hpp>

#include <algorithm>
#include <array>
#include <filesystem>
#include <span>
#include <utility>
#include <vector>

using cao::routing::AssetKind;
using cao::routing::AssetOperation;
using cao::routing::ExecutionMode;
using cao::routing::ProfileCapability;
using cao::routing::ProfileCapabilities;
using cao::routing::RequestedWork;
using cao::routing::RoutedAsset;
using cao::routing::RoutingPolicy;
using cao::routing::RunPhase;
using cao::routing::RunRequest;
using cao::run::ArchiveFirstAssetDiscovery;
using cao::run::extractArchiveNoOverwrite;

namespace
{
/// Compiles the Archive-enabled policy shared by discovery integration tests.
RoutingPolicy archiveEnabledPolicy()
{
    const auto request = RunRequest::forWork(
        ExecutionMode::Apply,
        {RequestedWork::NativeTextureOptimization, RequestedWork::ArchiveExtraction});
    const auto capabilities = ProfileCapabilities::define(
        ".bsa",
        {ProfileCapability::NativeTextureOptimization,
         ProfileCapability::ArchiveExtraction});
    const auto result = RoutingPolicy::compile(request, capabilities);
    if (!result.hasPolicy())
        qFatal("The known-valid Archive discovery policy failed to compile");
    return *result.policy();
}

/// Compiles a policy that recognizes Archives but does not request their extraction.
RoutingPolicy archiveDisabledPolicy()
{
    const auto request = RunRequest::forWork(
        ExecutionMode::Apply,
        {RequestedWork::NativeTextureOptimization});
    const auto capabilities = ProfileCapabilities::define(
        ".bsa",
        {ProfileCapability::NativeTextureOptimization,
         ProfileCapability::ArchiveExtraction});
    const auto result = RoutingPolicy::compile(request, capabilities);
    if (!result.hasPolicy())
        qFatal("The known-valid Archive-disabled discovery policy failed to compile");
    return *result.policy();
}

/// Writes one file, including any missing parent directories, and fails the test on I/O errors.
void writeFile(const std::filesystem::path &path, const QByteArray &contents)
{
    QVERIFY(QDir().mkpath(QString::fromStdWString(path.parent_path().wstring())));
    QFile file(QString::fromStdWString(path.wstring()));
    QVERIFY(file.open(QIODevice::WriteOnly));
    QCOMPARE(file.write(contents), contents.size());
}

/// Reads one complete fixture file and fails the test on I/O errors.
QByteArray readFile(const std::filesystem::path &path)
{
    QFile file(QString::fromStdWString(path.wstring()));
    if (!file.open(QIODevice::ReadOnly))
        qFatal("Could not read an Archive discovery fixture");
    return file.readAll();
}

/// Builds one real SSE Texture Archive from a staging tree outside the scanned mod root.
void createTextureArchive(const std::filesystem::path &archivePath,
                          const std::filesystem::path &stagingRoot,
                          const std::span<const std::filesystem::path> files)
{
    auto archive = btu::bsa::ArchiveData(btu::bsa::Settings::get(btu::Game::SSE),
                                         btu::bsa::ArchiveType::Textures);
    for (const auto &file : files)
        QVERIFY(archive.add_file(file));
    archive.set_out_path(archivePath);

    const auto errors = btu::bsa::write(false, std::move(archive), stagingRoot);
    QVERIFY(errors.empty());
    QVERIFY(std::filesystem::is_regular_file(archivePath));
}

/// Counts exact path occurrences without relying on unspecified directory traversal order.
std::size_t pathCount(const std::span<const std::filesystem::path> paths,
                      const std::filesystem::path &expected)
{
    return static_cast<std::size_t>(std::count(paths.begin(), paths.end(), expected));
}
}

class ArchiveFirstAssetDiscoveryTests final : public QObject
{
    Q_OBJECT

private slots:
    /// Verifies enabled Archives extract before one definitive non-Archive tree is returned.
    void extractsEnabledArchivesBeforeDefinitiveDiscovery();

    /// Verifies real extraction adds Archived Assets without replacing a colliding Loose Asset.
    void realExtractionPreservesLooseAssetPrecedence();

    /// Verifies recognized Archives excluded by policy never reach the extraction operation.
    void excludedArchivesAreNotExtracted();
};

void ArchiveFirstAssetDiscoveryTests::extractsEnabledArchivesBeforeDefinitiveDiscovery()
{
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());

    const auto root = std::filesystem::path(temporaryDirectory.path().toStdWString());
    const auto archive = root / "content.bsa";
    const auto looseAsset = root / "textures" / "loose.dds";
    const auto extractedAsset = root / "textures" / "extracted.dds";
    writeFile(archive, "archive placeholder");
    writeFile(looseAsset, "loose");

    std::vector<std::filesystem::path> extractedArchives;
    const ArchiveFirstAssetDiscovery discovery(archiveEnabledPolicy());
    const std::array roots{root};
    const auto effectiveTree = discovery.discover(
        roots,
        [&](const std::span<const RoutedAsset> selectedArchives) {
            QCOMPARE(selectedArchives.size(), std::size_t{1});
            const auto &selectedArchive = selectedArchives.front();
            QCOMPARE(selectedArchive.kind(), AssetKind::Archive);
            QCOMPARE(selectedArchive.phase(), RunPhase::ArchiveExtraction);
            QVERIFY(selectedArchive.operations().contains(AssetOperation::Extraction));
            extractedArchives.push_back(selectedArchive.executionPath());
            writeFile(extractedAsset, "extracted");
        });

    QCOMPARE(extractedArchives, std::vector<std::filesystem::path>{archive});
    QCOMPARE(pathCount(effectiveTree.effectiveAssetTree().paths(), looseAsset), std::size_t{1});
    QCOMPARE(pathCount(effectiveTree.effectiveAssetTree().paths(), extractedAsset), std::size_t{1});
    QCOMPARE(pathCount(effectiveTree.effectiveAssetTree().paths(), archive), std::size_t{0});
    QCOMPARE(effectiveTree.effectiveAssetTree().paths().size(), std::size_t{2});
}

void ArchiveFirstAssetDiscoveryTests::realExtractionPreservesLooseAssetPrecedence()
{
    QTemporaryDir modDirectory;
    QTemporaryDir stagingDirectory;
    QVERIFY(modDirectory.isValid());
    QVERIFY(stagingDirectory.isValid());

    const auto root = std::filesystem::path(modDirectory.path().toStdWString());
    const auto stagingRoot = std::filesystem::path(stagingDirectory.path().toStdWString());
    const auto archive = root / "content.bsa";
    const auto collision = root / "textures" / "collision.dds";
    const auto archivedOnly = root / "textures" / "archived-only.dds";
    const auto stagedCollision = stagingRoot / "textures" / "collision.dds";
    const auto stagedArchivedOnly = stagingRoot / "textures" / "archived-only.dds";
    writeFile(stagedCollision, "archived collision");
    writeFile(stagedArchivedOnly, "archived only");
    const std::array archivedFiles{stagedCollision, stagedArchivedOnly};
    createTextureArchive(archive, stagingRoot, archivedFiles);
    writeFile(collision, "loose collision");

    const ArchiveFirstAssetDiscovery discovery(archiveEnabledPolicy());
    const std::array roots{root};
    const auto effectiveTree = discovery.discover(
        roots,
        [](const std::span<const RoutedAsset> selectedArchives) {
            for (const auto &selectedArchive : selectedArchives)
                extractArchiveNoOverwrite(selectedArchive.executionPath(), false);
        });

    QCOMPARE(readFile(collision), QByteArray("loose collision"));
    QCOMPARE(readFile(archivedOnly), QByteArray("archived only"));
    QCOMPARE(pathCount(effectiveTree.effectiveAssetTree().paths(), collision), std::size_t{1});
    QCOMPARE(pathCount(effectiveTree.effectiveAssetTree().paths(), archivedOnly), std::size_t{1});
    QCOMPARE(effectiveTree.effectiveAssetTree().paths().size(), std::size_t{2});
}

void ArchiveFirstAssetDiscoveryTests::excludedArchivesAreNotExtracted()
{
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());

    const auto root = std::filesystem::path(temporaryDirectory.path().toStdWString());
    const auto archive = root / "disabled.bsa";
    const auto looseAsset = root / "textures" / "loose.dds";
    writeFile(archive, "archive placeholder");
    writeFile(looseAsset, "loose");

    bool extractionAttempted = false;
    const ArchiveFirstAssetDiscovery discovery(archiveDisabledPolicy());
    const std::array roots{root};
    const auto effectiveTree = discovery.discover(
        roots,
        [&](const std::span<const RoutedAsset>) { extractionAttempted = true; });

    QVERIFY(!extractionAttempted);
    QCOMPARE(pathCount(effectiveTree.effectiveAssetTree().paths(), looseAsset), std::size_t{1});
    QCOMPARE(pathCount(effectiveTree.effectiveAssetTree().paths(), archive), std::size_t{0});
    QCOMPARE(effectiveTree.effectiveAssetTree().paths().size(), std::size_t{1});
}

QTEST_MAIN(ArchiveFirstAssetDiscoveryTests)
#include "ArchiveFirstAssetDiscoveryTests.moc"
