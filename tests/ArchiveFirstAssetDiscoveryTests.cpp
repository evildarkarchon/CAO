#include "Run/ArchiveFirstAssetDiscovery.h"
#include "Run/ArchiveExtraction.h"

#include <QtTest>

#include <bsa/bsa.hpp>
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

/// Builds a valid small Archive in private staging without adding files to the scanned tree.
void createFixtureArchive(const std::filesystem::path& path) {
    QTemporaryDir stagingDirectory;
    QVERIFY(stagingDirectory.isValid());
    const auto staging = std::filesystem::path(stagingDirectory.path().toStdWString());
    const auto entry = staging / "textures" / "fixture.dds";
    writeFile(entry, "fixture");
    QVERIFY(QDir().mkpath(QString::fromStdWString(path.parent_path().wstring())));
    createTextureArchive(path, staging, std::array{entry});
}

/// Writes a raw manifest with tiny payload, then restores case/separators normalized by key
/// hashing. Replacement names retain their byte lengths, so every serialized offset remains
/// unchanged.
void createRawArchive(const std::filesystem::path& path, const int format,
                      const std::string& name) {
    QVERIFY(QDir().mkpath(QString::fromStdWString(path.parent_path().wstring())));
    const std::array payload{std::byte{0x42}};
    std::vector<std::pair<std::string, std::string>> names;
    if (format == 0) {
        bsa::tes3::archive archive;
        bsa::tes3::file file;
        file.read(payload);
        bsa::tes3::file::key key(name);
        names.emplace_back(key.name(), name);
        archive.insert(std::move(key), std::move(file));
        archive.write(path);
    } else if (format == 1 || format == 3) {
        bsa::tes4::archive archive;
        archive.archive_flags(bsa::tes4::archive_flag::directory_strings |
                              bsa::tes4::archive_flag::file_strings);
        const auto slash = name.find_last_of("/\\");
        const auto directoryName = name.substr(0, slash);
        const auto fileName = name.substr(slash + 1);
        bsa::tes4::directory::key directoryKey(directoryName);
        bsa::tes4::file::key fileKey(fileName);
        names.emplace_back(directoryKey.name(), directoryName);
        names.emplace_back(fileKey.name(), fileName);
        bsa::tes4::file file;
        file.read(payload, bsa::tes4::version::sse);
        if (format == 3) file.compress(bsa::tes4::version::sse);
        bsa::tes4::directory directory;
        directory.insert(std::move(fileKey), std::move(file));
        archive.insert(std::move(directoryKey), std::move(directory));
        archive.write(path, bsa::tes4::version::sse);
    } else {
        bsa::fo4::archive archive;
        bsa::fo4::file file;
        file.read(payload, bsa::fo4::format::general);
        bsa::fo4::file::key key(name);
        names.emplace_back(key.name(), name);
        archive.insert(std::move(key), std::move(file));
        archive.write(path, bsa::fo4::format::general);
    }
    auto bytes = readFile(path);
    for (const auto& [stored, original] : names) {
        QCOMPARE(stored.size(), original.size());
        const auto needle = QByteArray::fromStdString(stored);
        const auto position = bytes.indexOf(needle);
        QVERIFY(position >= 0);
        bytes.replace(position, needle.size(), QByteArray::fromStdString(original));
    }
    writeFile(path, bytes);
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
    /// A shortage in a later root prevents every extraction and preserves original Archives.
    void insufficientCapacityBlocksEntireBatch();
    /// Compressed and shadowed bytes still need full decompressed staging capacity.
    void compressedShadowedEntryRequiresCapacity();
    /// Rechecks fresh payload sizes before staging and treats unavailable capacity as unknown.
    void extractionCapacityRecheck_data();
    /// A late shortage leaves source bytes intact without creating run-owned staging.
    void extractionCapacityRecheck();
    /// A lost source after preflight commits no output and preserves safe continuation.
    void extractionFailureBeforeMergeCommitsNothing();
    /// Real payloads commit from registered staging and retain their original Archive.
    void stagedExtractionCommitsPayload_data();
    /// Exercises the staged writer for every supported Archive container.
    void stagedExtractionCommitsPayload();
    /// A source whose manifest changes after preflight must fail before any live merge.
    void changedManifestFailsBeforeMerge();
    /// A link inserted after preflight cannot redirect a staged commit outside the Mod Root.
    void mergeRejectsLinkedParent();
    /// A failure after the first merge retains that output and exposes unsafe mutation.
    void partialMergeRetainsCommittedOutput();
    /// Frozen winner decisions survive a failed Archive or a Loose Asset disappearing.
    void frozenPrecedenceSurvivesFailures_data();
    /// Executes real staging against a preflight plan after external tree changes.
    void frozenPrecedenceSurvivesFailures();
    /// Covers all supported raw Archive formats with and without unsafe path traversal.
    void rawManifestPaths_data();
    /// Verifies canonical collision paths and structured rejection before extraction.
    void rawManifestPaths();
    /// Verifies collisions remain scoped to independent Mod Roots.
    void collisionsDoNotCrossModRoots();
    /// Verifies a late unreadable Mod Root prevents extraction in all preceding roots.
    void lateUnreadableRootBlocksEveryExtraction();

    /// Verifies a corrupt required Archive prevents every extraction mutation.
    void unreadableArchiveStopsExtraction();
    /// Verifies Dry Run ignores invalid ordering intent and never inspects corrupt manifests.
    void dryRunIgnoresPrecedenceAndManifests();
    /// Verifies cancellation from collision reporting retains evidence without extraction.
    void collisionObserverCanCancelBeforeExtraction();
    /// Verifies direct Archive inputs through directory aliases use their resolved Mod Root.
    void explicitArchiveAliasUsesResolvedScope();
    /// Covers missing, extra, duplicate, and outside-root ordering intent.
    void invalidExplicitOrder_data();
    /// Verifies explicit precedence failures are structured and block the extraction batch.
    void invalidExplicitOrder();
    /// Exercises explicit and deterministic precedence with and without an authoritative Loose
    /// Asset.
    void collisionsAreReportedBeforeExtraction_data();
    /// Verifies all shadowed Archives and canonical game paths are reported before real extraction.
    void collisionsAreReportedBeforeExtraction();

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

void ArchiveFirstAssetDiscoveryTests::insufficientCapacityBlocksEntireBatch() {
    QTemporaryDir directory;
    const auto root = std::filesystem::path(directory.path().toStdWString());
    const auto first = root / "first";
    const auto last = root / "last";
    createFixtureArchive(first / "source.bsa");
    createFixtureArchive(last / "source.bsa");
    const ArchiveFirstAssetDiscovery discovery(archiveEnabledPolicy(),
        [&](const auto& path) -> std::optional<std::uintmax_t> {
            return path == last ? 180000 : std::numeric_limits<std::uintmax_t>::max();
        });
    bool extracted = false;
    const auto result = discovery.discover(std::array{first, last}, [&](auto) {
        extracted = true;
        return true;
    });
    QVERIFY(!extracted);
    QCOMPARE(result.failures().size(), std::size_t{1});
    QCOMPARE(result.failures().front().code(), cao::run::RunFailureCode::ArchiveInsufficientCapacity);
    QVERIFY(std::filesystem::exists(first / "source.bsa"));
    QVERIFY(std::filesystem::exists(last / "source.bsa"));
    QVERIFY(!std::filesystem::exists(first / "textures"));
}

void ArchiveFirstAssetDiscoveryTests::compressedShadowedEntryRequiresCapacity() {
    QTemporaryDir directory;
    const auto root = std::filesystem::path(directory.path().toStdWString());
    bsa::tes4::archive archive;
    archive.archive_flags(bsa::tes4::archive_flag::directory_strings |
                          bsa::tes4::archive_flag::file_strings);
    const std::vector<std::byte> payload(512 * 1024, std::byte{0x42});
    bsa::tes4::file file;
    file.read(payload, bsa::tes4::version::sse);
    file.compress(bsa::tes4::version::sse);
    bsa::tes4::directory entries;
    entries.insert(bsa::tes4::file::key("large.dds"), std::move(file));
    archive.insert(bsa::tes4::directory::key("textures"), std::move(entries));
    archive.write(root / "source.bsa", bsa::tes4::version::sse);
    writeFile(root / "textures/large.dds", "loose winner");
    QVERIFY(std::filesystem::file_size(root / "source.bsa") < 10000);
    const ArchiveFirstAssetDiscovery discovery(archiveEnabledPolicy(),
        [](const auto&) -> std::optional<std::uintmax_t> { return 400000; });
    bool extracted = false;
    const auto result = discovery.discover(std::array{root}, [&](auto) {
        extracted = true;
        return true;
    });
    QVERIFY(!extracted);
    QCOMPARE(result.failures().size(), std::size_t{1});
    QCOMPARE(result.failures().front().code(), cao::run::RunFailureCode::ArchiveInsufficientCapacity);
    QCOMPARE(readFile(root / "textures/large.dds"), QByteArray("loose winner"));
    QVERIFY(!std::filesystem::exists(root / ".cao-staging"));
}

void ArchiveFirstAssetDiscoveryTests::extractionCapacityRecheck_data() {
    QTest::addColumn<bool>("unknown");
    QTest::newRow("shortage") << false;
    QTest::newRow("unknown") << true;
}

void ArchiveFirstAssetDiscoveryTests::extractionCapacityRecheck() {
    QFETCH(bool, unknown);
    QTemporaryDir directory;
    const auto root = std::filesystem::path(directory.path().toStdWString());
    const auto archive = root / "source.bsa";
    createFixtureArchive(archive);
    const auto original = readFile(archive);
    cao::run::TemporaryArtifactRegistry artifacts;
    const cao::run::ArchiveExtractor extractor(artifacts,
        [&](const auto&) -> std::optional<std::uintmax_t> {
            return unknown ? std::nullopt : std::optional<std::uintmax_t>{0};
        });
    const auto result = extractor.extract(
        {archive, root, {"textures/fixture.dds"}, {"textures/fixture.dds"}});
    QCOMPARE(result.succeeded(), unknown);
    QCOMPARE(readFile(archive), original);
    QVERIFY(result.safeToContinue);
    if (!unknown) {
        QCOMPARE(result.failure, cao::run::ArchiveExtractionFailure::InsufficientCapacity);
        QCOMPARE(result.mutation, cao::execution::MutationState::None);
        QVERIFY(!std::filesystem::exists(root / ".cao-staging"));
        QVERIFY(!std::filesystem::exists(root / "textures"));
    }
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void ArchiveFirstAssetDiscoveryTests::extractionFailureBeforeMergeCommitsNothing() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    const auto archive = root / "missing.bsa";
    cao::run::TemporaryArtifactRegistry artifacts;
    const cao::run::ArchiveExtractor extractor(artifacts);
    const auto result = extractor.extract({archive, root, {"textures/a.dds"}, {"textures/a.dds"}});
    QVERIFY(!result.succeeded());
    QCOMPARE(result.failure, cao::run::ArchiveExtractionFailure::ExtractionFailed);
    QCOMPARE(result.mutation, cao::execution::MutationState::None);
    QVERIFY(result.safeToContinue);
    QVERIFY(!std::filesystem::exists(root / "textures"));
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void ArchiveFirstAssetDiscoveryTests::stagedExtractionCommitsPayload_data() {
    QTest::addColumn<int>("format");
    QTest::newRow("tes3") << 0;
    QTest::newRow("tes4") << 1;
    QTest::newRow("fo4") << 2;
    QTest::newRow("compressed tes4") << 3;
}

void ArchiveFirstAssetDiscoveryTests::stagedExtractionCommitsPayload() {
    QFETCH(int, format);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    const auto archive = root / "source.bsa";
    createRawArchive(archive, format, "textures/a.dds");
    const auto original = readFile(archive);
    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = cao::run::ArchiveExtractor(artifacts).extract(
        {archive, root, {"textures/a.dds"}, {"textures/a.dds"}});
    QVERIFY(result.succeeded());
    QCOMPARE(result.mutation, cao::execution::MutationState::Committed);
    QCOMPARE(readFile(root / "textures/a.dds"), QByteArray("B"));
    QCOMPARE(readFile(archive), original);
    QVERIFY(artifacts.performSafetyCleanup().empty());
    QCOMPARE(readFile(root / "textures/a.dds"), QByteArray("B"));
}

void ArchiveFirstAssetDiscoveryTests::partialMergeRetainsCommittedOutput() {
    QTemporaryDir directory;
    QTemporaryDir stagingDirectory;
    const auto root = std::filesystem::path(directory.path().toStdWString());
    const auto staging = std::filesystem::path(stagingDirectory.path().toStdWString());
    writeFile(staging / "a.dds", "committed");
    writeFile(staging / "z/b.dds", "blocked");
    createTextureArchive(root / "source.bsa", staging,
                         std::array{staging / "a.dds", staging / "z/b.dds"});
    writeFile(root / "z", "obstruction");
    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = cao::run::ArchiveExtractor(artifacts).extract(
        {root / "source.bsa", root, {"a.dds", "z/b.dds"}, {"a.dds", "z/b.dds"}});
    QCOMPARE(result.failure, cao::run::ArchiveExtractionFailure::MergeFailed);
    QCOMPARE(result.mutation, cao::execution::MutationState::PartialOrUnknown);
    QVERIFY(!result.safeToContinue);
    QVERIFY(artifacts.performSafetyCleanup().empty());
    QCOMPARE(readFile(root / "a.dds"), QByteArray("committed"));
    QCOMPARE(readFile(root / "z"), QByteArray("obstruction"));
    QVERIFY(std::filesystem::exists(root / "source.bsa"));
}

void ArchiveFirstAssetDiscoveryTests::frozenPrecedenceSurvivesFailures_data() {
    QTest::addColumn<bool>("loose");
    QTest::newRow("failed winning Archive") << false;
    QTest::newRow("disappeared Loose Asset") << true;
}

void ArchiveFirstAssetDiscoveryTests::frozenPrecedenceSurvivesFailures() {
    QFETCH(bool, loose);
    QTemporaryDir directory;
    const auto root = std::filesystem::path(directory.path().toStdWString());
    createRawArchive(root / "a.bsa", 0, "textures/a.dds");
    createRawArchive(root / "b.bsa", 0, "textures/a.dds");
    if (loose) writeFile(root / "textures/a.dds", "loose");
    std::vector<cao::run::ArchiveExtractionPlan> plans;
    std::vector<cao::run::ArchiveExtractionResult> attempts;
    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = ArchiveFirstAssetDiscovery(archiveEnabledPolicy()).discover(
        std::array{root}, [&](auto) {
            for (const auto& plan : plans)
                attempts.push_back(cao::run::ArchiveExtractor(artifacts).extract(plan));
            return true;
        }, {}, cao::run::ArchivePrecedence::deterministicDiscovery(), {},
        [&](std::span<const cao::run::ArchiveExtractionPlan> preflight) {
            plans.assign(preflight.begin(), preflight.end());
            if (loose) QVERIFY(std::filesystem::remove(root / "textures/a.dds"));
            else writeFile(root / "a.bsa", "corrupted after preflight");
        });
    QVERIFY(result.failures().empty());
    QCOMPARE(attempts.size(), std::size_t{2});
    QCOMPARE(attempts.front().succeeded(), loose);
    QVERIFY(attempts.front().safeToContinue);
    QVERIFY(attempts.back().succeeded());
    QCOMPARE(attempts.back().mutation, cao::execution::MutationState::None);
    QVERIFY(!std::filesystem::exists(root / "textures/a.dds"));
    QVERIFY(std::filesystem::exists(root / "a.bsa"));
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void ArchiveFirstAssetDiscoveryTests::changedManifestFailsBeforeMerge() {
    QTemporaryDir directory;
    const auto root = std::filesystem::path(directory.path().toStdWString());
    const auto archive = root / "source.bsa";
    createRawArchive(archive, 0, "textures/a.dds");
    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = cao::run::ArchiveExtractor(artifacts).extract(
        {archive, root, {"textures/expected.dds"}, {"textures/expected.dds"}});
    QCOMPARE(result.failure, cao::run::ArchiveExtractionFailure::ExtractionFailed);
    QCOMPARE(result.mutation, cao::execution::MutationState::None);
    QVERIFY(result.safeToContinue);
    QVERIFY(std::filesystem::exists(archive));
    QVERIFY(!std::filesystem::exists(root / "textures"));
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void ArchiveFirstAssetDiscoveryTests::mergeRejectsLinkedParent() {
    QTemporaryDir directory;
    const auto base = std::filesystem::path(directory.path().toStdWString());
    const auto root = base / "mod";
    const auto outside = base / "outside";
    std::filesystem::create_directories(outside);
    createRawArchive(root / "source.bsa", 0, "textures/a.dds");
    const auto link = root / "textures";
#ifdef _WIN32
    QProcess process;
    process.start("powershell.exe", {"-NoProfile", "-NonInteractive", "-Command",
        "New-Item -ItemType Junction -Path '" + QString::fromStdWString(link.wstring()) +
        "' -Value '" + QString::fromStdWString(outside.wstring()) + "' -ErrorAction Stop | Out-Null"});
    QVERIFY(process.waitForFinished());
    QCOMPARE(process.exitCode(), 0);
#else
    std::filesystem::create_directory_symlink(outside, link);
#endif
    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = cao::run::ArchiveExtractor(artifacts).extract(
        {root / "source.bsa", root, {"textures/a.dds"}, {"textures/a.dds"}});
    // Remove only the link so fixture cleanup cannot touch its independent target.
    QVERIFY(std::filesystem::remove(link));
    QCOMPARE(result.failure, cao::run::ArchiveExtractionFailure::MergeFailed);
    QCOMPARE(result.mutation, cao::execution::MutationState::PartialOrUnknown);
    QVERIFY(!result.safeToContinue);
    QVERIFY(!std::filesystem::exists(outside / "a.dds"));
    QVERIFY(std::filesystem::exists(root / "source.bsa"));
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void ArchiveFirstAssetDiscoveryTests::rawManifestPaths_data() {
    QTest::addColumn<int>("format");
    QTest::addColumn<bool>("escaping");
    for (int format = 0; format < 3; ++format) {
        QTest::newRow(qPrintable(QStringLiteral("format-%1-contained").arg(format)))
            << format << false;
        QTest::newRow(qPrintable(QStringLiteral("format-%1-escaping").arg(format)))
            << format << true;
    }
}

void ArchiveFirstAssetDiscoveryTests::rawManifestPaths() {
    QFETCH(int, format);
    QFETCH(bool, escaping);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    const auto first = root / "a.bsa";
    const auto second = root / "b.bsa";
    createRawArchive(first, format, "Textures/Shared.dds");
    createRawArchive(second, format,
                     escaping ? "..\\escaped.dds" : "TEXTURES\\folder\\..\\.\\SHARED.DDS");
    const auto beforeFirst = readFile(first);
    const auto beforeSecond = readFile(second);
    std::size_t extractions = 0;
    const auto result = ArchiveFirstAssetDiscovery(archiveEnabledPolicy())
                            .discover(std::array{root}, [&](const auto& archives) {
                                extractions += archives.size();
                                return true;
                            });
    QVERIFY(!result.cancelled());
    if (escaping) {
        QCOMPARE(extractions, std::size_t{0});
        QCOMPARE(result.failures().size(), std::size_t{1});
        QCOMPARE(result.failures().front().code(), cao::run::RunFailureCode::ArchiveEntryInvalid);
        QCOMPARE(result.failures().front().phase(), cao::run::RunPhase::DiscoveringArchives);
        QCOMPARE(result.failures().front().path(), second);
        QVERIFY(result.effectiveAssetTree().paths().empty());
    } else {
        QVERIFY(result.failures().empty());
        QCOMPARE(extractions, std::size_t{2});
        QCOMPARE(result.collisions().size(), std::size_t{1});
        QCOMPARE(result.collisions().front().gamePath(),
                 std::filesystem::path("textures/shared.dds"));
        QCOMPARE(result.collisions().front().winningArchive(), first);
        QCOMPARE(result.collisions().front().shadowedArchives().front(), second);
    }
    QCOMPARE(readFile(first), beforeFirst);
    QCOMPARE(readFile(second), beforeSecond);
    QVERIFY(!std::filesystem::exists(root.parent_path() / "escaped.dds"));
}

void ArchiveFirstAssetDiscoveryTests::collisionsDoNotCrossModRoots() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto base = std::filesystem::path(directory.path().toStdWString());
    const std::array roots{base / "first", base / "second"};
    for (const auto& root : roots) createRawArchive(root / "content.bsa", 1, "textures/shared.dds");
    std::size_t extractions = 0;
    const auto result = ArchiveFirstAssetDiscovery(archiveEnabledPolicy())
                            .discover(roots, [&](const auto& archives) {
                                extractions += archives.size();
                                return true;
                            });
    QVERIFY(result.failures().empty());
    QVERIFY(result.collisions().empty());
    QCOMPARE(extractions, std::size_t{2});
}

void ArchiveFirstAssetDiscoveryTests::lateUnreadableRootBlocksEveryExtraction() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto base = std::filesystem::path(directory.path().toStdWString());
    const std::array roots{base / "first", base / "second"};
    createRawArchive(roots[0] / "content.bsa", 1, "textures/shared.dds");
    writeFile(roots[1] / "broken.bsa", "unreadable");
    std::size_t extractions = 0;
    const auto result = ArchiveFirstAssetDiscovery(archiveEnabledPolicy())
                            .discover(roots, [&](const auto& archives) {
                                extractions += archives.size();
                                return true;
                            });
    QCOMPARE(extractions, std::size_t{0});
    QCOMPARE(result.failures().size(), std::size_t{1});
    QCOMPARE(result.failures().front().code(), cao::run::RunFailureCode::ArchiveUnreadable);
    QCOMPARE(result.failures().front().path(), roots[1] / "broken.bsa");
}

void ArchiveFirstAssetDiscoveryTests::unreadableArchiveStopsExtraction() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    writeFile(root / "broken.bsa", "not an archive");
    bool extracted = false;
    const auto result = ArchiveFirstAssetDiscovery(archiveEnabledPolicy())
                            .discover(std::vector{root}, [&](const auto&) {
                                extracted = true;
                                return true;
                            });
    QVERIFY(!extracted);
    QVERIFY(result.effectiveAssetTree().paths().empty());
    QCOMPARE(result.failures().size(), std::size_t{1});
    QCOMPARE(result.failures().front().code(), cao::run::RunFailureCode::ArchiveUnreadable);
    QCOMPARE(result.failures().front().phase(), cao::run::RunPhase::DiscoveringArchives);
    QCOMPARE(result.failures().front().path(), root / "broken.bsa");
}

void ArchiveFirstAssetDiscoveryTests::explicitArchiveAliasUsesResolvedScope() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto base = std::filesystem::path(directory.path().toStdWString());
    const auto root = base / "mod";
    createRawArchive(root / "a.bsa", 1, "textures/shared.dds");
    const auto alias = base / "alias";
    std::error_code error;
    std::filesystem::create_directory_symlink(root, alias, error);
    if (error) QSKIP("Directory symlink creation is unavailable on this host");
    std::vector<std::filesystem::path> extracted;
    const auto result = ArchiveFirstAssetDiscovery(archiveEnabledPolicy()).discover(
        std::vector{alias / "a.bsa"}, [&](const auto& archives) {
            for (const auto& archive : archives) extracted.push_back(archive.executionPath());
            return true;
        }, {}, cao::run::ArchivePrecedence::explicitOrder({"a.bsa"}));
    QVERIFY(result.failures().empty());
    QCOMPARE(extracted, std::vector{root / "a.bsa"});
    QVERIFY(std::filesystem::remove(alias));
}

void ArchiveFirstAssetDiscoveryTests::dryRunIgnoresPrecedenceAndManifests() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    writeFile(root / "broken.bsa", "invalid manifest");
    const auto compiled = RoutingPolicy::compile(
        RoutingPolicyRequest::forWork(ExecutionMode::DryRun, {RequestedWork::ArchiveExtraction}),
        ProfileCapabilities::define(".bsa", {ProfileCapability::ArchiveExtraction}));
    QVERIFY(compiled.hasPolicy());
    bool extracted = false;
    bool reported = false;
    const auto result =
        ArchiveFirstAssetDiscovery(*compiled.policy())
            .discover(
                std::vector{root},
                [&](const auto&) {
                    extracted = true;
                    return true;
                },
                {},
                cao::run::ArchivePrecedence::explicitOrder({"../missing.bsa", "../missing.bsa"}),
                [&](auto) { reported = true; });
    QVERIFY(result.failures().empty());
    QVERIFY(result.collisions().empty());
    QVERIFY(!extracted);
    QVERIFY(!reported);
    QCOMPARE(result.skippedArchiveCount(cao::routing::SkipReason::DisabledPhase), std::size_t{1});
    QCOMPARE(readFile(root / "broken.bsa"), QByteArray("invalid manifest"));
    QVERIFY(!std::filesystem::exists(root / ".cao-staging"));
}

void ArchiveFirstAssetDiscoveryTests::collisionObserverCanCancelBeforeExtraction() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    createRawArchive(root / "a.bsa", 1, "textures/shared.dds");
    createRawArchive(root / "b.bsa", 1, "textures/shared.dds");
    bool cancelled = false;
    bool extracted = false;
    const auto result =
        ArchiveFirstAssetDiscovery(archiveEnabledPolicy())
            .discover(
                std::vector{root},
                [&](const auto&) {
                    extracted = true;
                    return true;
                },
                [&] { return cancelled; }, cao::run::ArchivePrecedence::deterministicDiscovery(),
                [&](auto collisions) {
                    QCOMPARE(collisions.size(), std::size_t{1});
                    cancelled = true;
                });
    QVERIFY(result.cancelled());
    QVERIFY(result.failures().empty());
    QCOMPARE(result.collisions().size(), std::size_t{1});
    QVERIFY(!extracted);
    QVERIFY(result.effectiveAssetTree().paths().empty());
}

void ArchiveFirstAssetDiscoveryTests::invalidExplicitOrder_data() {
    QTest::addColumn<QStringList>("order");
    QTest::addColumn<int>("code");
    using Code = cao::run::RunFailureCode;
    QTest::newRow("missing") << QStringList{} << int(Code::ArchiveOrderMissing);
    QTest::newRow("extra") << QStringList{"content.bsa", "other.bsa"}
                           << int(Code::ArchiveOrderExtra);
    QTest::newRow("duplicate") << QStringList{"content.bsa", "./content.bsa"}
                               << int(Code::ArchiveOrderDuplicate);
    QTest::newRow("outside") << QStringList{"../content.bsa"} << int(Code::ArchiveOrderOutsideRoot);
}

void ArchiveFirstAssetDiscoveryTests::invalidExplicitOrder() {
    QFETCH(QStringList, order);
    QFETCH(int, code);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto base = std::filesystem::path(directory.path().toStdWString());
    const auto root = base / "mod";
    std::filesystem::create_directories(root);
    const auto staged = base / "source" / "textures" / "shared.dds";
    writeFile(staged, "archived");
    createTextureArchive(root / "content.bsa", base / "source", std::vector{staged});
    std::vector<std::filesystem::path> paths;
    for (const auto& name : order) paths.emplace_back(name.toStdWString());
    bool extracted = false;
    const auto result = ArchiveFirstAssetDiscovery(archiveEnabledPolicy())
                            .discover(
                                std::vector{root},
                                [&](const auto&) {
                                    extracted = true;
                                    return true;
                                },
                                {}, cao::run::ArchivePrecedence::explicitOrder(std::move(paths)));
    QVERIFY(!extracted);
    QCOMPARE(result.failures().size(), std::size_t{1});
    QCOMPARE(int(result.failures().front().code()), code);
    QVERIFY(result.effectiveAssetTree().paths().empty());
}

void ArchiveFirstAssetDiscoveryTests::collisionsAreReportedBeforeExtraction_data() {
    QTest::addColumn<bool>("explicitOrder");
    QTest::addColumn<bool>("loose");
    QTest::addColumn<bool>("fileRoots");
    QTest::newRow("deterministic") << false << false << false;
    QTest::newRow("explicit") << true << false << false;
    QTest::newRow("loose-over-deterministic") << false << true << false;
    QTest::newRow("loose-over-explicit") << true << true << false;
    QTest::newRow("file-roots") << false << true << true;
    QTest::newRow("explicit-file-roots") << true << true << true;
}

void ArchiveFirstAssetDiscoveryTests::collisionsAreReportedBeforeExtraction() {
    QFETCH(bool, explicitOrder);
    QFETCH(bool, loose);
    QFETCH(bool, fileRoots);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto base = std::filesystem::path(directory.path().toStdWString());
    const auto root = base / "mod";
    std::filesystem::create_directories(root);
    for (const auto* name : {"a", "b", "c"}) {
        const auto source = base / name;
        const auto staged = source / "Textures" / "Shared.DDS";
        writeFile(staged, name);
        createTextureArchive(root / (std::string(name) + ".bsa"), source, std::vector{staged});
    }
    const auto shared = root / "textures" / "shared.dds";
    if (loose) writeFile(shared, "loose");
    auto precedence = explicitOrder
                          ? cao::run::ArchivePrecedence::explicitOrder({"c.bsa", "a.bsa", "b.bsa"})
                          : cao::run::ArchivePrecedence::deterministicDiscovery();
    bool reported = false;
    bool reportedBeforeExtraction = false;
    std::vector<std::filesystem::path> extracted;
    std::vector<cao::run::ArchiveCollision> observed;
    const auto result =
        ArchiveFirstAssetDiscovery(archiveEnabledPolicy())
            .discover(
                fileRoots ? std::vector{root / "a.bsa", root / "b.bsa", root / "c.bsa"}
                          : std::vector{root},
                [&](const auto& archives) {
                    reportedBeforeExtraction = reported;
                    for (const auto& archive : archives) {
                        extracted.push_back(archive.executionPath());
                        extractArchiveNoOverwrite(archive.executionPath(), false);
                    }
                    return true;
                },
                {}, precedence,
                [&](auto collisions) {
                    reported = true;
                    QCOMPARE(std::filesystem::exists(shared), loose);
                    observed.assign(collisions.begin(), collisions.end());
                });
    QVERIFY(result.failures().empty());
    QVERIFY(reportedBeforeExtraction);
    QCOMPARE(observed.size(), std::size_t{1});
    QCOMPARE(result.collisions().size(), std::size_t{1});
    const auto& collision = result.collisions().front();
    QCOMPARE(collision.modRoot(), std::filesystem::canonical(root));
    QCOMPARE(collision.gamePath(), std::filesystem::path("textures/shared.dds"));
    QCOMPARE(collision.winningArchive(), root / (explicitOrder ? "c.bsa" : "a.bsa"));
    QCOMPARE(collision.shadowedArchives().size(), std::size_t{2});
    QCOMPARE(collision.shadowedArchives()[0], root / (explicitOrder ? "a.bsa" : "b.bsa"));
    QCOMPARE(collision.shadowedArchives()[1], root / (explicitOrder ? "b.bsa" : "c.bsa"));
    QCOMPARE(collision.looseAssetWins(), loose);
    QCOMPARE(extracted.front(), collision.winningArchive());
    QCOMPARE(readFile(shared), QByteArray(loose ? "loose" : explicitOrder ? "c" : "a"));
}

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
            createFixtureArchive(root / *name);

    std::vector<std::filesystem::path> observed;
    const ArchiveFirstAssetDiscovery discovery(archiveEnabledPolicy());
    const auto result = discovery.discover(roots, [&](std::span<const RoutedAsset> archives) {
        for (const auto& archive : archives) observed.push_back(archive.executionPath());
        return true;
    });
    std::vector<std::filesystem::path> expected;
    for (const auto& root : roots)
        for (const auto& name : names) expected.push_back(root / name);
    QVERIFY(result.failures().empty());
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
    createFixtureArchive(root / "content.bsa");
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
    createFixtureArchive(original / "content.bsa");
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
    QVERIFY(result.failures().empty());
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
    createFixtureArchive(archive);
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

    QVERIFY(effectiveTree.failures().empty());
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

    QVERIFY(effectiveTree.failures().empty());
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

    QVERIFY(effectiveTree.failures().empty());
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
    createFixtureArchive(archive);
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
    createFixtureArchive(archive);

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
    QVERIFY(result->failures().empty());
    QVERIFY(result->effectiveAssetTree().paths().empty());
}

void ArchiveFirstAssetDiscoveryTests::removedDirectoryRootDoesNotThrow()
{
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());

    const auto root = std::filesystem::path(temporaryDirectory.path().toStdWString()) / "mod";
    const auto archive = root / "content.bsa";
    createFixtureArchive(archive);

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
    QVERIFY(result->failures().empty());
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
    createFixtureArchive(archive);

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
