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
    /// Verifies an otherwise contained file alias cannot admit staging into either pass.
    void linksIntoStagingAreExcluded();
    /// Verifies normalized relative Archive ordering, Unicode folding, and caller root precedence.
    void archivesAreOrderedWithinEachModRoot();
    /// Verifies staging and unknown staging-like trees never enter either discovery pass.
    void stagingIsExcludedFromDiscovery();
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

    /// Verifies linked files cannot make discovery extract or optimize outside the Mod Root.
    void escapingFileLinksAreExcluded();

    /// Covers contained and escaping directory aliases for each available link type.
    void directoryLinksAreExcluded_data();

    /// Verifies directory links, including Windows junctions, are never traversed.
    void directoryLinksAreExcluded();

    /// Verifies a contained file link remains eligible for ordinary Loose Asset work.
    void containedFileLinkIsDiscovered();

    /// Verifies unresolved links cannot silently disappear from the run's diagnostics.
    void danglingFileLinkIsDiagnosed();

    /// Verifies the definitive pass applies containment to links produced by extraction.
    void extractionProducedLinksAreExcluded();

    /// Verifies a selected directory alias is resolved once, even if extraction retargets it.
    void selectedDirectoryAliasKeepsItsOriginalTarget();
};

void ArchiveFirstAssetDiscoveryTests::linksIntoStagingAreExcluded() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    const auto staging = root / ".cao-staging" / "owned";
    writeFile(staging / "content.bsa", "uncommitted archive");
    writeFile(staging / "texture.dds", "uncommitted asset");
    std::error_code error;
    std::filesystem::create_symlink(staging / "content.bsa", root / "linked.bsa", error);
    if (error) QSKIP("File symlink creation is unavailable on this host");
    std::filesystem::create_symlink(staging / "texture.dds", root / "linked.dds", error);
    QVERIFY2(!error, error.message().c_str());
    std::size_t extractions{};
    const auto result = ArchiveFirstAssetDiscovery(archiveEnabledPolicy()).discover(
        std::vector{root}, [&](const auto& archives) {
            extractions += archives.size();
            return true;
        });
    QVERIFY(std::filesystem::remove(root / "linked.bsa", error));
    QVERIFY(std::filesystem::remove(root / "linked.dds", error));
    QCOMPARE(extractions, std::size_t{0});
    QVERIFY(result.effectiveAssetTree().paths().empty());
    QCOMPARE(result.diagnostics().size(), std::size_t{2});
}

void ArchiveFirstAssetDiscoveryTests::archivesAreOrderedWithinEachModRoot() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto base = std::filesystem::path(directory.path().toStdWString());
    const std::array roots{base / "z-mod", base / "a-mod"};
    const std::vector<std::filesystem::path> names{
        "alpha.bsa", "alpha/z.bsa", "Beta.bsa", "STRASSE.bsa",
        std::filesystem::path(u8"Straße.bsa"), "z.bsa"};
    for (const auto& root : roots)
        for (auto name = names.rbegin(); name != names.rend(); ++name)
            writeFile(root / *name, "archive placeholder");

    std::vector<std::filesystem::path> observed;
    const ArchiveFirstAssetDiscovery discovery(archiveEnabledPolicy());
    const auto result = discovery.discover(roots, [&](std::span<const RoutedAsset> archives) {
        for (const auto& archive : archives) observed.push_back(archive.executionPath());
        return true;
    });
    std::vector<std::filesystem::path> expected;
    for (const auto& root : roots)
        for (const auto& name : names) expected.push_back(root / name);
    QVERIFY(!result.cancelled());
    QCOMPARE(observed, expected);
}

void ArchiveFirstAssetDiscoveryTests::stagingIsExcludedFromDiscovery() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    writeFile(root / "loose.dds", "loose");
    for (const auto* name : {".cao-staging", ".CAO-Staging-abandoned"}) {
        writeFile(root / name / "temporary.dds", "temporary");
        writeFile(root / name / "archive.bsa", "uncommitted archive");
    }
    std::size_t extractions{};
    const auto result = ArchiveFirstAssetDiscovery(archiveEnabledPolicy()).discover(
        std::vector{root}, [&](const auto& archives) {
            extractions += archives.size();
            return true;
        });
    QCOMPARE(extractions, std::size_t{0});
    QCOMPARE(result.effectiveAssetTree().paths().size(), std::size_t{1});
    QCOMPARE(result.effectiveAssetTree().paths().front(), root / "loose.dds");
    QCOMPARE(readFile(root / ".cao-staging" / "temporary.dds"), QByteArray("temporary"));
}

void ArchiveFirstAssetDiscoveryTests::escapingFileLinksAreExcluded()
{
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());
    const auto base = std::filesystem::path(temporaryDirectory.path().toStdWString());
    const auto root = base / "mod";
    const auto outside = base / "outside";
    writeFile(root / "inside.dds", "inside");
    writeFile(outside / "outside.dds", "outside");
    writeFile(outside / "outside.bsa", "outside archive");
    std::error_code error;
    std::filesystem::create_symlink(outside / "outside.dds", root / "linked.dds", error);
    if (error) QSKIP("File symlink creation is unavailable on this host");
    std::filesystem::create_symlink(outside / "outside.bsa", root / "linked.bsa", error);
    QVERIFY2(!error, error.message().c_str());

    std::size_t selectedArchiveCount = 0;
    const ArchiveFirstAssetDiscovery discovery(archiveEnabledPolicy());
    const std::array roots{root};
    const auto result = discovery.discover(roots, [&](const std::span<const RoutedAsset> archives) {
        selectedArchiveCount += archives.size();
        return true;
    });

    QVERIFY(std::filesystem::remove(root / "linked.dds", error));
    QVERIFY(std::filesystem::remove(root / "linked.bsa", error));
    QCOMPARE(selectedArchiveCount, std::size_t{0});
    QCOMPARE(result.effectiveAssetTree().paths().size(), std::size_t{1});
    QCOMPARE(result.effectiveAssetTree().paths().front(), root / "inside.dds");
    QCOMPARE(result.diagnostics().size(), std::size_t{2});
    for (const auto& diagnostic : result.diagnostics()) {
        QCOMPARE(diagnostic.code(), cao::run::RunDiagnosticCode::LinkedEntryExcluded);
        QCOMPARE(diagnostic.phase(), cao::run::RunPhase::DiscoveringArchives);
        QVERIFY(diagnostic.path() == root / "linked.dds" || diagnostic.path() == root / "linked.bsa");
        QVERIFY(!diagnostic.detail().empty());
    }
}

void ArchiveFirstAssetDiscoveryTests::directoryLinksAreExcluded_data()
{
    QTest::addColumn<bool>("junction");
    QTest::addColumn<bool>("contained");
    QTest::newRow("escaping symlink") << false << false;
    QTest::newRow("contained symlink") << false << true;
#ifdef _WIN32
    QTest::newRow("escaping junction") << true << false;
    QTest::newRow("contained junction") << true << true;
#endif
}

void ArchiveFirstAssetDiscoveryTests::directoryLinksAreExcluded()
{
    QFETCH(bool, junction);
    QFETCH(bool, contained);
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());
    const auto base = std::filesystem::path(temporaryDirectory.path().toStdWString());
    const auto root = base / "mod";
    const auto target = (contained ? root : base) / "target";
    const auto link = root / "linked";
    writeFile(root / "inside.dds", "inside");
    writeFile(target / "target.dds", "linked content");
    std::error_code error;
    if (junction) {
        // Junction creation needs no symlink privilege, so Windows CI exercises reparse traversal
        // even when its account cannot create symbolic links.
        QProcess process;
        auto quotedPath = [](const std::filesystem::path& path) {
            auto value = QString::fromStdWString(path.wstring());
            value.replace("'", "''");
            return "'" + value + "'";
        };
        process.start("powershell.exe", {"-NoProfile", "-NonInteractive", "-Command",
            "New-Item -ItemType Junction -Path " + quotedPath(link) + " -Value " +
            quotedPath(target) + " -ErrorAction Stop | Out-Null"});
        QVERIFY(process.waitForFinished());
        QCOMPARE(process.exitCode(), 0);
    } else {
        std::filesystem::create_directory_symlink(target, link, error);
        if (error) QSKIP("Directory symlink creation is unavailable on this host");
    }

    const ArchiveFirstAssetDiscovery discovery(archiveDisabledPolicy());
    const std::array roots{root};
    const auto result = discovery.discover(roots, [](auto) { return true; });

    // Remove only the link before QTemporaryDir cleanup; its target is separate fixture content.
    QVERIFY(std::filesystem::remove(link, error));
    QVERIFY2(!error, error.message().c_str());
    QCOMPARE(pathCount(result.effectiveAssetTree().paths(), link / "target.dds"), std::size_t{0});
    QCOMPARE(result.effectiveAssetTree().paths().size(), contained ? std::size_t{2} : std::size_t{1});
    QCOMPARE(result.diagnostics().size(), std::size_t{1});
    QCOMPARE(result.diagnostics().front().code(), cao::run::RunDiagnosticCode::LinkedEntryExcluded);
    QCOMPARE(result.diagnostics().front().path(), link);
}

void ArchiveFirstAssetDiscoveryTests::containedFileLinkIsDiscovered()
{
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());
    const auto root = std::filesystem::path(temporaryDirectory.path().toStdWString());
    writeFile(root / "original.dds", "inside");
    std::error_code error;
    std::filesystem::create_symlink(root / "original.dds", root / "linked.dds", error);
    if (error) QSKIP("File symlink creation is unavailable on this host");

    const ArchiveFirstAssetDiscovery discovery(archiveDisabledPolicy());
    const std::array roots{root};
    const auto result = discovery.discover(roots, [](auto) { return true; });
    QVERIFY(std::filesystem::remove(root / "linked.dds", error));
    QCOMPARE(pathCount(result.effectiveAssetTree().paths(), root / "linked.dds"), std::size_t{1});
    QCOMPARE(result.effectiveAssetTree().paths().size(), std::size_t{2});
    QVERIFY(result.diagnostics().empty());
}

void ArchiveFirstAssetDiscoveryTests::danglingFileLinkIsDiagnosed()
{
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());
    const auto root = std::filesystem::path(temporaryDirectory.path().toStdWString());
    const auto link = root / "dangling.dds";
    std::error_code error;
    std::filesystem::create_symlink(root / "missing.dds", link, error);
    if (error) QSKIP("File symlink creation is unavailable on this host");

    const ArchiveFirstAssetDiscovery discovery(archiveDisabledPolicy());
    const std::array roots{root};
    const auto result = discovery.discover(roots, [](auto) { return true; });
    QVERIFY(std::filesystem::remove(link, error));
    QVERIFY(result.effectiveAssetTree().paths().empty());
    QCOMPARE(result.diagnostics().size(), std::size_t{1});
    QCOMPARE(result.diagnostics().front().code(), cao::run::RunDiagnosticCode::LinkedEntryExcluded);
    QCOMPARE(result.diagnostics().front().path(), link);
}

void ArchiveFirstAssetDiscoveryTests::extractionProducedLinksAreExcluded()
{
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());
    const auto base = std::filesystem::path(temporaryDirectory.path().toStdWString());
    const auto root = base / "mod";
    const auto outside = base / "outside.dds";
    const auto link = root / "extracted.dds";
    writeFile(root / "content.bsa", "archive");
    writeFile(outside, "outside");
    std::error_code error;
    std::filesystem::create_symlink(outside, link, error);
    if (error) QSKIP("File symlink creation is unavailable on this host");
    QVERIFY(std::filesystem::remove(link, error));

    const ArchiveFirstAssetDiscovery discovery(archiveEnabledPolicy());
    const std::array roots{root};
    const auto result = discovery.discover(roots, [&](auto) {
        std::filesystem::create_symlink(outside, link, error);
        return !error;
    });
    QVERIFY2(!error, error.message().c_str());
    QVERIFY(std::filesystem::remove(link, error));
    QVERIFY(result.effectiveAssetTree().paths().empty());
    QCOMPARE(result.diagnostics().size(), std::size_t{1});
    QCOMPARE(result.diagnostics().front().phase(), cao::run::RunPhase::BuildingEffectiveAssetTree);
    QCOMPARE(result.diagnostics().front().path(), link);
}

void ArchiveFirstAssetDiscoveryTests::selectedDirectoryAliasKeepsItsOriginalTarget()
{
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());
    const auto base = std::filesystem::path(temporaryDirectory.path().toStdWString());
    const auto original = base / "original";
    const auto replacement = base / "replacement";
    const auto alias = base / "selected";
    writeFile(original / "content.bsa", "archive");
    writeFile(original / "original.dds", "original");
    writeFile(replacement / "replacement.dds", "replacement");
    std::error_code error;
    std::filesystem::create_directory_symlink(original, alias, error);
    if (error) QSKIP("Directory symlink creation is unavailable on this host");

    const ArchiveFirstAssetDiscovery discovery(archiveEnabledPolicy());
    const std::array roots{alias};
    const auto result = discovery.discover(roots, [&](auto) {
        if (!std::filesystem::remove(alias, error)) return false;
        std::filesystem::create_directory_symlink(replacement, alias, error);
        return !error;
    });
    QVERIFY2(!error, error.message().c_str());
    QVERIFY(std::filesystem::remove(alias, error));
    QVERIFY(!result.cancelled());
    QCOMPARE(result.effectiveAssetTree().paths().size(), std::size_t{1});
    QCOMPARE(result.effectiveAssetTree().paths().front(), original / "original.dds");
}

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
