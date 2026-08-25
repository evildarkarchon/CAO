#include "Run/ArchiveFirstAssetDiscovery.h"

#include <QtTest>

#include <btu/bsa/archive_data.hpp>
#include <btu/bsa/pack.hpp>
#include <btu/bsa/settings.hpp>

#include <algorithm>
#include <array>
#include <filesystem>
#include <optional>
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
using cao::routing::RoutedAssetPhase;
using cao::routing::RoutingPolicyRequest;
using cao::run::ArchiveFirstAssetDiscovery;
using cao::run::extractArchiveNoOverwrite;

namespace
{
/// Compiles the Archive-enabled policy shared by discovery integration tests.
RoutingPolicy archiveEnabledPolicy()
{
    const auto request = RoutingPolicyRequest::forWork(
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
    const auto request = RoutingPolicyRequest::forWork(
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

    /// Verifies a cancelled extraction batch does not trigger definitive filesystem discovery.
    void cancelledExtractionSkipsDefinitiveTraversal();

    /// Verifies an explicitly supplied Archive may disappear before pass two without escaping.
    void removedExplicitArchiveRootDoesNotThrow();

    /// Verifies an Archive named directly as a root yields the Assets extraction produced beside
    /// it, without adopting the unrelated Assets that directory already held.
    void explicitArchiveRootDiscoversExtractedSiblings();

    /// Verifies a directory root may disappear before pass two without escaping.
    void removedDirectoryRootDoesNotThrow();

    /// Verifies an Archive that extraction itself produced never enters the Effective Asset Tree.
    void archivesProducedByExtractionStayOutOfTheTree();
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
    std::size_t selectedArchiveCount = 0;
    bool selectedArchiveWasRoutedForExtraction = false;
    const ArchiveFirstAssetDiscovery discovery(archiveEnabledPolicy());
    const std::array roots{root};
    const auto effectiveTree = discovery.discover(
        roots,
        [&](const std::span<const RoutedAsset> selectedArchives) {
            selectedArchiveCount = selectedArchives.size();
            if (!selectedArchives.empty()) {
                const auto &selectedArchive = selectedArchives.front();
                selectedArchiveWasRoutedForExtraction = selectedArchive.kind() == AssetKind::Archive
                                                        && selectedArchive.phase()
                                                               == RoutedAssetPhase::ArchiveExtraction
                                                        && selectedArchive.operations().contains(
                                                            AssetOperation::Extraction);
                extractedArchives.push_back(selectedArchive.executionPath());
            }
            writeFile(extractedAsset, "extracted");
            return true;
        });

    QCOMPARE(selectedArchiveCount, std::size_t{1});
    QVERIFY(selectedArchiveWasRoutedForExtraction);
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
            return true;
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
        [&](const std::span<const RoutedAsset>) {
            extractionAttempted = true;
            return true;
        });

    QVERIFY(!extractionAttempted);
    QCOMPARE(pathCount(effectiveTree.effectiveAssetTree().paths(), looseAsset), std::size_t{1});
    QCOMPARE(pathCount(effectiveTree.effectiveAssetTree().paths(), archive), std::size_t{0});
    QCOMPARE(effectiveTree.effectiveAssetTree().paths().size(), std::size_t{1});
    // Policy excluded this Archive, which the Archive pass already accounted for as a Skip Reason.
    // Counting it as malformed nesting as well would warn about every Archive in a run that simply
    // was not asked to extract them.
    QCOMPARE(effectiveTree.nestedArchiveCount(), std::size_t{0});
}

void ArchiveFirstAssetDiscoveryTests::cancelledExtractionSkipsDefinitiveTraversal()
{
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());

    const auto root = std::filesystem::path(temporaryDirectory.path().toStdWString());
    const auto archive = root / "content.bsa";
    const auto looseAsset = root / "textures" / "loose.dds";
    writeFile(archive, "archive placeholder");
    writeFile(looseAsset, "loose");

    const ArchiveFirstAssetDiscovery discovery(archiveEnabledPolicy());
    const std::array roots{root};
    const auto result = discovery.discover(
        roots,
        [](const std::span<const RoutedAsset>) { return false; });

    QVERIFY(result.effectiveAssetTree().paths().empty());
}

void ArchiveFirstAssetDiscoveryTests::removedExplicitArchiveRootDoesNotThrow()
{
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());

    const auto archive = std::filesystem::path(temporaryDirectory.path().toStdWString())
                         / "content.bsa";
    writeFile(archive, "archive placeholder");

    std::optional<cao::run::ArchiveFirstAssetDiscoveryResult> result;
    const ArchiveFirstAssetDiscovery discovery(archiveEnabledPolicy());
    const std::array roots{archive};
    try {
        result.emplace(discovery.discover(
            roots,
            [&](const std::span<const RoutedAsset>) {
                return std::filesystem::remove(archive);
            }));
    } catch (const std::filesystem::filesystem_error &) {
        // The assertion below reports the filesystem error as a test failure.
    }

    QVERIFY(result.has_value());
    QVERIFY(result->effectiveAssetTree().paths().empty());
}

void ArchiveFirstAssetDiscoveryTests::removedDirectoryRootDoesNotThrow()
{
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());

    const auto root = std::filesystem::path(temporaryDirectory.path().toStdWString()) / "mod";
    const auto archive = root / "content.bsa";
    writeFile(archive, "archive placeholder");

    std::optional<cao::run::ArchiveFirstAssetDiscoveryResult> result;
    const ArchiveFirstAssetDiscovery discovery(archiveEnabledPolicy());
    const std::array roots{root};
    try {
        result.emplace(discovery.discover(
            roots,
            [&](const std::span<const RoutedAsset>) {
                return std::filesystem::remove_all(root) != 0;
            }));
    } catch (const std::filesystem::filesystem_error &) {
        // The assertion below reports the filesystem error as a test failure.
    }

    QVERIFY(result.has_value());
    QVERIFY(result->effectiveAssetTree().paths().empty());
}

void ArchiveFirstAssetDiscoveryTests::explicitArchiveRootDiscoversExtractedSiblings()
{
    QTemporaryDir modDirectory;
    QTemporaryDir stagingDirectory;
    QVERIFY(modDirectory.isValid());
    QVERIFY(stagingDirectory.isValid());

    const auto root = std::filesystem::path(modDirectory.path().toStdWString());
    const auto stagingRoot = std::filesystem::path(stagingDirectory.path().toStdWString());
    const auto archive = root / "content.bsa";
    const auto extracted = root / "textures" / "archived-only.dds";
    const auto preExisting = root / "textures" / "pre-existing.dds";
    const auto staged = stagingRoot / "textures" / "archived-only.dds";
    writeFile(staged, "archived only");
    const std::array archivedFiles{staged};
    createTextureArchive(archive, stagingRoot, archivedFiles);
    writeFile(preExisting, "pre-existing");

    const ArchiveFirstAssetDiscovery discovery(archiveEnabledPolicy());
    const std::array roots{archive};
    const auto result = discovery.discover(
        roots,
        [](const std::span<const RoutedAsset> selectedArchives) {
            for (const auto &selectedArchive : selectedArchives)
                extractArchiveNoOverwrite(selectedArchive.executionPath(), false);
            return true;
        });

    // Traversing the Archive file again would find only the excluded Archive, so the extracted
    // Assets are reachable solely through the destination the extraction wrote into.
    QVERIFY(std::filesystem::is_regular_file(extracted));
    QCOMPARE(pathCount(result.effectiveAssetTree().paths(), extracted), std::size_t{1});
    // The caller named an Archive rather than the directory, so Assets that were already there
    // were never requested and stay out of the Effective Asset Tree.
    QCOMPARE(pathCount(result.effectiveAssetTree().paths(), preExisting), std::size_t{0});
    QCOMPARE(pathCount(result.effectiveAssetTree().paths(), archive), std::size_t{0});
    QCOMPARE(result.effectiveAssetTree().paths().size(), std::size_t{1});
}

void ArchiveFirstAssetDiscoveryTests::archivesProducedByExtractionStayOutOfTheTree()
{
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());

    const auto root = std::filesystem::path(temporaryDirectory.path().toStdWString());
    const auto archive = root / "content.bsa";
    const auto nestedArchive = root / "textures" / "nested.bsa";
    const auto extractedAsset = root / "textures" / "extracted.dds";
    writeFile(archive, "archive placeholder");

    std::size_t selectedArchiveCount = 0;
    const ArchiveFirstAssetDiscovery discovery(archiveEnabledPolicy());
    const std::array roots{root};
    const auto result = discovery.discover(
        roots,
        [&](const std::span<const RoutedAsset> selectedArchives) {
            selectedArchiveCount = selectedArchives.size();
            // Extraction produces an Archive of its own, which the Archive pass could not have
            // offered for extraction because it did not exist while Archives were being selected.
            // The game reads no Archive nested inside another, so this is malformed mod content
            // that a run must ignore rather than work a later round should pick up.
            writeFile(nestedArchive, "nested archive placeholder");
            writeFile(extractedAsset, "extracted");
            return true;
        });

    QCOMPARE(selectedArchiveCount, std::size_t{1});
    // Admitting the nested Archive would route it as Archive work that no post-extraction target
    // performs, inflating the run's work total with an Asset nothing can execute.
    QCOMPARE(pathCount(result.effectiveAssetTree().paths(), nestedArchive), std::size_t{0});
    QCOMPARE(pathCount(result.effectiveAssetTree().paths(), archive), std::size_t{0});
    QCOMPARE(pathCount(result.effectiveAssetTree().paths(), extractedAsset), std::size_t{1});
    QCOMPARE(result.effectiveAssetTree().paths().size(), std::size_t{1});

    // Ignoring it silently would leave the author believing its contents were processed. The
    // count is one, not two: the Archive the run did extract is accounted for by the extraction it
    // received, so counting it as malformed nesting too would be a false alarm.
    QCOMPARE(result.nestedArchiveCount(), std::size_t{1});
}

QTEST_MAIN(ArchiveFirstAssetDiscoveryTests)
#include "ArchiveFirstAssetDiscoveryTests.moc"
