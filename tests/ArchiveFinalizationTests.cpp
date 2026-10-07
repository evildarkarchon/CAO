#include "Run/ArchiveFinalization.h"
#include "ArchiveFinalizationTestSupport.h"
#include "Run/ArchiveFinalizationResult.h"
#include "Run/NativeVolume.h"
#include "Run/RunEvidence.h"
#include "Run/RunPreparation.h"
#include "Run/TemporaryArtifactRegistry.h"

#include <btu/bsa/archive_data.hpp>
#include <btu/bsa/settings.hpp>

#include <QProcess>
#include <QRegularExpression>
#include <QTemporaryDir>
#include <QTest>

#include <algorithm>
#include <array>
#include <chrono>
#include <filesystem>
#include <limits>
#include <optional>
#include <span>
#include <stdexcept>
#include <stop_token>
#include <string>
#include <thread>
#include <utility>
#include <vector>
#ifdef _WIN32
#include <Windows.h>
#endif

namespace {
namespace fs = std::filesystem;
using cao::execution::MutationState;
using cao::routing::ExecutionMode;
using cao::routing::ProfileCapabilities;
using cao::routing::ProfileCapability;
using cao::routing::RequestedWork;
using cao::routing::RoutingPolicy;
using cao::routing::RoutingPolicyRequest;
using cao::run::ArchiveFinalization;
using cao::run::ArchiveFinalizationFailure;
using cao::run::ArchiveFinalizationMutationKind;
using cao::run::ArchiveFinalizationResult;
using cao::run::ArchiveFinalizationSettings;
using cao::run::CapacityProbe;
using cao::run::RunPhase;
using cao::run::RunPhaseRecord;
using cao::run::RunProgress;
using cao::run::TemporaryArtifactRegistry;
using cao::run::VolumeIdentityProbe;

constexpr auto unlimitedCapacity = std::numeric_limits<std::uintmax_t>::max();

/// Writes one test fixture after creating its parent directory.
void writeFile(const fs::path& path, const QByteArray& contents) {
    QVERIFY(QDir().mkpath(QString::fromStdWString(path.parent_path().wstring())));
    QFile file(QString::fromStdWString(path.wstring()));
    QVERIFY(file.open(QIODevice::WriteOnly));
    QCOMPARE(file.write(contents), contents.size());
}

/// Writes a minimal valid 4x4 uncompressed DDS Texture, creating its parent directory.
void writeTexture(const fs::path& path) {
    QVERIFY(QDir().mkpath(QString::fromStdWString(path.parent_path().wstring())));
    DirectX::ScratchImage image;
    QVERIFY(SUCCEEDED(image.Initialize2D(DXGI_FORMAT_R8G8B8A8_UNORM, 4, 4, 1, 1)));
    QVERIFY(SUCCEEDED(DirectX::SaveToDDSFile(*image.GetImage(0, 0, 0), DirectX::DDS_FLAGS_NONE,
                                             path.c_str())));
}

/// Reads a whole fixture file; an unreadable file yields a null array that no fixture matches.
QByteArray readFile(const fs::path& path) {
    QFile file(QString::fromStdWString(path.wstring()));
    if (!file.open(QIODevice::ReadOnly)) return {};
    return file.readAll();
}

/// Copies a selected game's canonical Dummy Plugin bytes for independent filesystem fixtures.
QByteArray canonicalDummyBytes(const btu::Game game) {
    const auto& bytes = *btu::bsa::Settings::get(game).s_dummy_plugin;
    return QByteArray(reinterpret_cast<const char*>(bytes.data()), static_cast<int>(bytes.size()));
}

/// Lists the Archives directly inside a Mod Root, in no particular order.
std::vector<fs::path> archivesIn(const fs::path& root) {
    std::vector<fs::path> archives;
    for (const auto& entry : fs::directory_iterator(root))
        if (entry.is_regular_file() &&
            (entry.path().extension() == ".bsa" || entry.path().extension() == ".ba2"))
            archives.push_back(entry.path());
    return archives;
}

/// Replaces a directory entry with a junction to target through PowerShell, like a user would.
/// Returns whether the junction was created.
bool createJunction(const fs::path& link, const fs::path& target) {
    const auto quotedPath = [](const fs::path& path) {
        auto value = QString::fromStdWString(path.wstring());
        value.replace("'", "''");
        return "'" + value + "'";
    };
    QProcess process;
    process.start("powershell.exe", {"-NoProfile", "-NonInteractive", "-Command",
                                     "New-Item -ItemType Junction -Path " + quotedPath(link) +
                                         " -Value " + quotedPath(target) +
                                         " -ErrorAction Stop | Out-Null"});
    return process.waitForFinished() && process.exitCode() == 0;
}

/// Profile settings for one game without reading any profile directory or global state.
OptimizerProfileSnapshot profileFor(const btu::Game game) {
    return {game,
            static_cast<double>(btu::bsa::Settings::get(game).max_size),
            nifly::NiFileVersion{},
            0u,
            0u,
            DXGI_FORMAT_UNKNOWN,
            {},
            false,
            {},
            {},
            {},
            {}};
}

/// The OptionsCAO defaults, so each test states only the choices its scenario depends on.
ArchiveFinalizationSettings legacyDefaults() {
    return {.compress = true,
            .deleteSources = true,
            .createDummyPlugins = true,
            .mergeIncompressible = true,
            .mergeTextures = false};
}

/// Prepares an Apply run over roots whose Routing Policy requests Archive creation, or only
/// Texture work when createArchives is false.
cao::run::RunPreparation preparationFor(const std::span<const fs::path> roots,
                                        const bool createArchives) {
    const auto work =
        createArchives ? RequestedWork::ArchiveCreation : RequestedWork::NativeTextureOptimization;
    auto compiled = RoutingPolicy::compile(
        RoutingPolicyRequest::forWork(ExecutionMode::Apply, {work}),
        ProfileCapabilities::define(".bsa", {ProfileCapability::ArchiveCreation,
                                             ProfileCapability::NativeTextureOptimization}));
    if (!compiled.hasPolicy()) throw std::runtime_error("Test Routing Policy failed to compile.");
    return cao::run::RunPreparation(
        {roots.begin(), roots.end()},
        cao::run::RunConfiguration(cao::run::SelectedProfileFacts{
            .archiveExtension = ".bsa",
            .supportsNativeTextureOptimization = true,
            .supportsArchiveCreation = true}),
        *compiled.policy(), cao::run::ArchivePrecedence::deterministicDiscovery());
}

/// One Archive Finalization configuration; tests override only what their scenario needs.
struct Finalizer final {
    btu::Game game{btu::Game::SSE};
    ArchiveFinalizationSettings settings{legacyDefaults()};
    CapacityProbe capacity{cao::run::availableArchiveCapacity};
    VolumeIdentityProbe volumeIdentity{cao::run::archiveVolumeIdentity};
    bool createArchives{true};
    QStringList filesToNotPack;
    std::stop_token stop;

    /// Runs the phase once over roots, recording into evidence and staging through artifacts.
    void run(const std::span<const fs::path> roots, cao::run::RunWorkEvidence& evidence,
             TemporaryArtifactRegistry& artifacts) const {
        auto profile = profileFor(game);
        profile.filesToNotPack = filesToNotPack;
        ArchiveFinalization(std::move(profile), settings, capacity, volumeIdentity)
            .run(preparationFor(roots, createArchives), evidence, artifacts, stop);
    }
};

/// Returns the staging estimate Archive Finalization reports when root has no capacity at all.
/// A root with one planned output reports that output's estimate plus its dummy reserve.
std::uintmax_t reportedEstimate(const fs::path& root, Finalizer finalizer) {
    finalizer.capacity = [](const fs::path&) -> std::optional<std::uintmax_t> { return 0; };
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{root}, evidence.workEvidence, artifacts);
    static_cast<void>(artifacts.performSafetyCleanup());
    const auto* result = evidence.finalization();
    if (result == nullptr) throw std::runtime_error("No finalization result was recorded.");
    const auto& detail =
        result->attempts.empty() ? result->detail : result->attempts.front().detail;
    const auto match = QRegularExpression(QStringLiteral("estimated (\\d+) bytes"))
                           .match(QString::fromStdString(detail));
    if (!match.hasMatch()) throw std::runtime_error("No capacity estimate was reported.");
    return match.captured(1).toULongLong();
}

/// Collects every Archive Finalization progress account in publication order.
class ProgressObservation final : public cao::run::RunObservationSink {
   public:
    std::vector<RunProgress> updates;

    /// Retains determinate Archive Finalization progress only.
    void recordPhase(const RunPhaseRecord& phase) override {
        if (phase.phase() == RunPhase::ArchiveFinalization && phase.progress())
            updates.push_back(*phase.progress());
    }
    /// Archive Finalization reports no run-level failures.
    void recordFailure(const cao::run::RunFailure&) override {}
    /// Archive Finalization reports no diagnostics.
    void recordDiagnostic(const cao::run::RunDiagnostic&) override {}
};

/// Reports whether this host can create file symlinks, for tests that need a linked plugin.
bool fileSymlinksAvailable(const fs::path& directory) {
    const auto target = directory / "symlink-probe-target";
    const auto link = directory / "symlink-probe";
    writeFile(target, QByteArrayLiteral("probe"));
    std::error_code error;
    fs::create_symlink(target, link, error);
    fs::remove(link);
    fs::remove(target);
    return !error;
}
}  // namespace

class ArchiveFinalizationTests final : public QObject {
    Q_OBJECT

   private slots:
    /// Exercises phase-wide shortage, unknown capacity, and a race between output attempts.
    void finalizationCapacityChecks_data();
    /// Ensures capacity failures preserve unattempted sources and prevent directory pruning.
    void finalizationCapacityChecks();
    /// Rejects a shortage of one byte below the published Archive and Dummy Plugin sizes.
    void finalizationCapacityEstimateCoversPublishedOutput();
    /// Allows independent volumes to finalize when each fits its own output estimate.
    void finalizationCapacityIsGroupedByVolume();
    /// An idle Mod Root with unknown identity must not inflate other volumes' output estimates.
    void idleUnknownVolumeDoesNotInflateCapacity();
    /// Resolves volume identity and same-volume staging for a Mod Root deeper than MAX_PATH.
    void volumeQueriesAcceptLongModRoots();
    /// Grows the mount point buffer past MAX_PATH and past a separator-dropping near fit.
    void volumeMountPointGrowsForLongMountedFolders();
    /// Rechecks only the remaining dummy-plugin reserve after each same-volume root completes.
    void dummyCapacityDecreasesAfterEachRoot();
    /// Reports plugin-only capacity failure without inventing output progress or pruning folders.
    void finalizationCapacityWithoutOutputs();
    /// Checks capacity before either mutation boundary when a planned Loading Plugin disappears.
    void disappearingLoadingPluginCapacityIsReserved_data();
    /// Rejects Archive-only capacity when finalization may need a fallback Dummy Plugin.
    void disappearingLoadingPluginCapacityIsReserved();

    /// Records a cancelled result without an output total when planning observes cancellation.
    void finalizationPlanningObservesCancellation();
    /// Covers cancellation before work, between outputs, and after the final output.
    void finalizationFreezesTotalAndCancelsBetweenOutputs_data();
    /// Records the total before mutation and each attempt before the next output starts.
    void finalizationFreezesTotalAndCancelsBetweenOutputs();
    /// Keeps a committed output loadable when cancellation leaves later outputs unattempted.
    void cancellationPreservesCommittedArchiveLoadingPlugin();
    /// Stops plugin cleanup when cancellation arrives between Mod Roots.
    void finalizationCancelsPluginCleanupBetweenRoots();

    /// Keeps a completed output and reports a contained guarded plugin removal failure.
    void finalizationReportsGuardedPluginRemovalFailure();
    /// Retains loading-plugin changes for existing Archives even with zero planned outputs.
    void finalizationRetainsPluginMutations_data();
    /// Derives plugin-only mutation evidence without inventing output attempts.
    void finalizationRetainsPluginMutations();
    /// Exercises exact-byte cleanup and preservation on Windows profiles.
    void finalizationRemovesOnlyExactDummyPlugins_data();
    /// Records only the removed canonical Dummy Plugin, including one written outside CAO.
    void finalizationRemovesOnlyExactDummyPlugins();
    /// Covers native link guards for an exact-byte plugin during Windows cleanup.
    void finalizationRejectsLinkedDummyPlugins_data();
    /// Preserves both a linked entry and its canonical-byte target without reporting mutation.
    void finalizationRejectsLinkedDummyPlugins();
    /// Rejects a Mod Root replaced by a junction after the finalization plan is frozen.
    void finalizationRejectsChangedDummyPluginParent();
    /// Retains a different-content replacement introduced after planning.
    void finalizationPreservesReplacedDummyPlugin();
    /// Retains earlier root removals when a later native guard fails before deletion.
    void finalizationRetainsRemovalPrefixOnGuardFailure();

    /// Reports an occupied existing-Archive plugin destination without changing it or progress.
    void existingArchivePluginCollisionFailsSafely();
    /// Uses selected-profile Archive suffix and plugin extension rules for existing Archives.
    void existingArchivePluginNamesFollowProfile_data();
    /// Publishes the profile's canonical bytes at its suffix-free Loading Plugin name.
    void existingArchivePluginNamesFollowProfile();
    /// Covers shared creation, an existing full Loading Plugin, and an exact existing Dummy Plugin.
    void existingArchivesShareLoadingPlugin_data();
    /// Preserves an existing Loading Plugin identity while recording only new publications.
    void existingArchivesShareLoadingPlugin();

    /// Keeps temporary Texture bytes out of Archives and their packed-source deletion pass.
    void packingPreservesStagingFiles();
    /// Never packs or deletes a Loose Asset matched by the profile's FilesToNotPack list.
    void filesToNotPackAreNeitherPackedNorDeleted();
    /// Retains every Loose Asset when an output cannot include all of its planned sources.
    void failedPackingRetainsSourcesAndExistingArchives();
    /// Rejects a planned source when a parent is swapped for a junction before packing.
    void packedSourceParentJunctionIsRejected();
    /// Reserves distinct names and rejects an output destination occupied after planning.
    void plannedNamesAreDistinctAndCommitPreservesNewDestination();
    /// Uses a same-size Loading Plugin for output naming while ignoring an exact Dummy Plugin.
    void plannedOutputNamesUseExactDummyBytes_data();
    /// Checks selected Windows profiles with suffix-free and suffixed Archive names.
    void plannedOutputNamesUseExactDummyBytes();
    /// Covers recognized Loading Plugin names that appear after planning on Windows profiles.
    void plannedOutputRechecksLoadingPluginNames_data();
    /// Reuses the newly recognized plugin without publishing a Dummy Plugin.
    void plannedOutputRechecksLoadingPluginNames();
    /// Keeps a published Archive and its source when a competing plugin occupies the planned name.
    void plannedPluginCollisionRetainsCommittedArchive();
    /// Reuses an exact dummy created after planning without replacing its filesystem entry.
    void plannedExactDummyIsReused();
    /// Publishes one shared Dummy Plugin as a separate effect of two planned Archives.
    void plannedOutputsRecordSharedDummyPublication();
    /// Publishes a Dummy Plugin if the Loading Plugin observed during planning disappears.
    void plannedLoadingPluginDisappearsBeforeCommit();
    /// Covers a linked Loading Plugin at the chosen or an alternate recognized name.
    void linkedLoadingPluginRemainsUsable_data();
    /// Publishes a Windows fallback so retargeting the linked plugin cannot unload the Archive.
    void linkedLoadingPluginRemainsUsable();
    /// Retains Loose Assets when every recognized Loading Plugin name is occupied by a link.
    void linkedLoadingPluginsWithoutFallbackRetainSources();
    /// Exercises existing and published Loading Plugins during source cleanup.
    void loadingPluginRemainsPinnedThroughSourceCleanup_data();
    /// Prevents deletion of the selected Loading Plugin until all packed sources are removed.
    void loadingPluginRemainsPinnedThroughSourceCleanup();
    /// Exercises retained source recovery with readable and byte-locked files.
    void committedArchiveRetainsLockedSource_data();
    /// Continues later outputs only when committed Archive and retained source bytes are usable.
    void committedArchiveRetainsLockedSource();

    /// Prunes empty directories but never packs when the Routing Policy omits Archive creation.
    void packingRequiresArchiveCreationRequest();
    /// Covers exceptions before and after the output total is recorded.
    void unexpectedExceptionIsRecordedOnce_data();
    /// Records one unsafe phase-level UnexpectedException instead of letting work escape.
    void unexpectedExceptionIsRecordedOnce();
    /// Keeps the committed attempt when Loading Plugin maintenance throws afterwards.
    void cleanupExceptionKeepsRecordedAttempts();
    /// Covers evidence rejection before planning, for a capacity rejection, and after commit.
    void evidenceFailuresPropagateUnchanged_data();
    /// Lets Run Evidence invariant violations reach the Run Executor unconverted.
    void evidenceFailuresPropagateUnchanged();
};

void ArchiveFinalizationTests::finalizationCapacityChecks_data() {
    QTest::addColumn<int>("scenario");
    QTest::newRow("late-root-shortage") << 0;
    QTest::newRow("unknown-capacity") << 1;
    QTest::newRow("capacity-disappears") << 2;
    QTest::newRow("source-grows-after-planning") << 3;
}

void ArchiveFinalizationTests::finalizationCapacityChecks() {
    QFETCH(int, scenario);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto parent = fs::path(directory.path().toStdWString());
    const std::array roots{parent / "mod-a", parent / "mod-b"};
    for (const auto& mod : roots) {
        writeFile(mod / "meshes" / "asset.nif", QByteArray(8192, 'x'));
        fs::create_directory(mod / "empty");
    }
    Finalizer finalizer;
    // Both roots share one volume, so the phase preflight sums both outputs at each root.
    finalizer.volumeIdentity = [](const fs::path&) -> std::optional<std::string> {
        return "shared-volume";
    };
    // Identical Mod Roots plan identical outputs, so one root's estimate stands for both.
    const auto outputEstimate = reportedEstimate(roots.front(), finalizer);
    bool grown = false;
    finalizer.capacity = [&](const fs::path& root) -> std::optional<std::uintmax_t> {
        if (scenario == 1) return std::nullopt;
        // Each output fits individually, but the shared phase cannot fit at the later root.
        if (scenario == 0 && root == fs::canonical(roots[1])) return outputEstimate;
        // The first probe runs after planning froze the later output's estimate.
        if (scenario == 3 && !grown) {
            writeFile(roots[1] / "meshes" / "asset.nif", QByteArray(1024 * 1024, 'y'));
            grown = true;
        }
        if (!archivesIn(roots[0]).empty()) return scenario == 3 ? outputEstimate : 0;
        return unlimitedCapacity;
    };
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(roots, evidence.workEvidence, artifacts);

    const auto& result = *evidence.finalization();
    QVERIFY(result.safeToContinue);
    QVERIFY(!result.cancelled);
    QCOMPARE(evidence.progress()->total(), std::size_t{2});
    QCOMPARE(evidence.progress()->completed(), result.attempts.size());
    const auto committed = scenario == 0 ? std::size_t{0} : scenario == 1 ? 2 : 1;
    for (std::size_t index = 0; index < roots.size(); ++index) {
        const auto& mod = roots[index];
        QCOMPARE(archivesIn(mod).size(), index < committed ? std::size_t{1} : std::size_t{0});
        QCOMPARE(fs::exists(mod / "meshes" / "asset.nif"), index >= committed);
        QCOMPARE(fs::exists(mod / "empty"), scenario != 1);
        // The Dummy Plugin is published with its output and never before a rejected one.
        QCOMPARE(fs::exists(mod / (mod.filename().wstring() + L".esp")), index < committed);
        if (index >= committed) QVERIFY(!fs::exists(mod / ".cao-staging"));
    }
    if (scenario != 1) {
        const auto& rejected = result.attempts.back();
        QCOMPARE(rejected.failure, std::optional{ArchiveFinalizationFailure::InsufficientCapacity});
        QCOMPARE(rejected.mutation, MutationState::None);
        // Every rejecting scenario stops at the later root: in its preflight or its attempt.
        QCOMPARE(rejected.modRoot, fs::canonical(roots[1]));
        QVERIFY(!rejected.detail.empty());
    }
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void ArchiveFinalizationTests::finalizationCapacityEstimateCoversPublishedOutput() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto mod = fs::path(directory.path().toStdWString()) / "mod";
    writeFile(mod / "meshes" / "asset.nif", QByteArray(8192, 'x'));
    Finalizer finalizer;
    const auto estimate = reportedEstimate(mod, finalizer);
    QVERIFY(archivesIn(mod).empty());

    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{mod}, evidence.workEvidence, artifacts);
    const auto& result = *evidence.finalization();
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QVERIFY2(result.attempts.front().succeeded(), result.attempts.front().detail.c_str());
    const auto published =
        fs::file_size(result.attempts.front().archivePath) + fs::file_size(mod / "mod.esp");
    QVERIFY(estimate >= published);
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void ArchiveFinalizationTests::finalizationCapacityIsGroupedByVolume() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto parent = fs::path(directory.path().toStdWString());
    const std::array roots{parent / "mod-a", parent / "mod-b"};
    for (const auto& root : roots)
        writeFile(root / "textures" / "asset.dds", QByteArrayLiteral("source bytes"));
    Finalizer finalizer;
    finalizer.settings.createDummyPlugins = false;
    finalizer.settings.compress = false;
    finalizer.settings.deleteSources = false;
    const auto outputEstimate = reportedEstimate(roots.front(), finalizer);
    // Each root has exactly its own output's allowance, not room for the whole batch.
    finalizer.capacity = [&](const fs::path&) -> std::optional<std::uintmax_t> {
        return outputEstimate;
    };
    finalizer.volumeIdentity = [&](const fs::path& path) -> std::optional<std::string> {
        return path == fs::canonical(roots.front()) ? "first-volume" : "second-volume";
    };
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(roots, evidence.workEvidence, artifacts);
    const auto& result = *evidence.finalization();
    QCOMPARE(result.attempts.size(), std::size_t{2});
    QVERIFY(result.safeToContinue);
    QVERIFY(!result.failure);
    for (const auto& attempt : result.attempts) {
        QVERIFY(attempt.succeeded());
        QVERIFY(fs::exists(attempt.archivePath));
    }
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void ArchiveFinalizationTests::idleUnknownVolumeDoesNotInflateCapacity() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto parent = fs::path(directory.path().toStdWString());
    const std::array roots{parent / "mod-a", parent / "mod-b", parent / "mod-idle"};
    for (std::size_t index = 0; index < 2; ++index)
        writeFile(roots[index] / "textures" / "asset.dds", QByteArrayLiteral("source bytes"));
    fs::create_directory(roots.back());
    Finalizer finalizer;
    finalizer.settings.createDummyPlugins = false;
    finalizer.settings.compress = false;
    const auto outputEstimate = reportedEstimate(roots.front(), finalizer);
    finalizer.capacity = [&](const fs::path& path) -> std::optional<std::uintmax_t> {
        if (path == fs::canonical(roots.back())) return 0;
        return outputEstimate;
    };
    finalizer.volumeIdentity = [&](const fs::path& path) -> std::optional<std::string> {
        if (path == fs::canonical(roots.back())) return std::nullopt;
        return path == fs::canonical(roots.front()) ? "first-volume" : "second-volume";
    };
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(roots, evidence.workEvidence, artifacts);
    const auto& result = *evidence.finalization();
    QCOMPARE(result.attempts.size(), std::size_t{2});
    QVERIFY(!result.failure);
    for (const auto& attempt : result.attempts) QVERIFY(attempt.succeeded());
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void ArchiveFinalizationTests::volumeQueriesAcceptLongModRoots() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto parent = fs::canonical(fs::path(directory.path().toStdWString()));
    auto root = parent;
    // Well past Windows' 260-character MAX_PATH, so both native volume queries receive a long
    // input even though the drive-letter mount point they resolve is short.
    while (root.native().size() <= 600) root /= std::wstring(40, L'd');
    fs::create_directories(root / "textures");
    const auto expected = cao::run::archiveVolumeIdentity(parent);
    QVERIFY(expected.has_value());
    QCOMPARE(cao::run::archiveVolumeIdentity(root), expected);
    try {
        TemporaryArtifactRegistry artifacts;
        const auto staged = artifacts.stageFile(root, root / "textures" / "asset.dds");
        QVERIFY(fs::is_regular_file(staged.path));
        QVERIFY(artifacts.performSafetyCleanup().empty());
    } catch (const std::exception& error) {
        QFAIL(error.what());
    }
}

void ArchiveFinalizationTests::volumeMountPointGrowsForLongMountedFolders() {
#ifdef _WIN32
    // Mounting a real volume in a folder needs elevation, so this query reproduces the
    // documented GetVolumePathNameW buffer contract for a deep mounted folder instead.
    const auto mount = L"C:\\" + std::wstring(600, L'm') + L"\\";
    std::vector<DWORD> sizes;
    std::size_t truncatedFits = 0;
    const auto query = [&](const wchar_t*, wchar_t* output, const DWORD size) -> BOOL {
        sizes.push_back(size);
        if (size < mount.size()) {
            SetLastError(ERROR_FILENAME_EXCED_RANGE);
            return FALSE;
        }
        // One character short, the real API succeeds without the trailing separator.
        const auto written = size == mount.size() ? mount.size() - 1 : mount.size();
        if (written != mount.size()) ++truncatedFits;
        mount.copy(output, written);
        output[written] = L'\0';
        return TRUE;
    };
    // A short path through a junction can resolve to a mounted folder longer than itself.
    const std::wstring junction = L"C:\\junction\\mod";
    QCOMPARE(cao::run::volumeMountPoint(junction, query), mount);
    QCOMPARE(sizes, (std::vector<DWORD>{MAX_PATH, 2 * MAX_PATH, 4 * MAX_PATH}));
    QCOMPARE(truncatedFits, std::size_t{0});

    // The first guess holds the path plus a separator and terminator; for this input it is
    // exactly one character short, so the separator-dropping result must be retried.
    sizes.clear();
    const auto nearFit = mount.substr(0, mount.size() - 2);
    QCOMPARE(cao::run::volumeMountPoint(nearFit, query), mount);
    QCOMPARE(sizes, (std::vector<DWORD>{static_cast<DWORD>(mount.size()),
                                        static_cast<DWORD>(2 * mount.size())}));
    QCOMPARE(truncatedFits, std::size_t{1});

    // Failures other than an undersized buffer are reported without retrying.
    sizes.clear();
    const auto failing = [&](const wchar_t*, wchar_t*, const DWORD size) -> BOOL {
        sizes.push_back(size);
        SetLastError(ERROR_INVALID_NAME);
        return FALSE;
    };
    QVERIFY(cao::run::volumeMountPoint(junction, failing).empty());
    QCOMPARE(GetLastError(), static_cast<DWORD>(ERROR_INVALID_NAME));
    QCOMPARE(sizes.size(), std::size_t{1});
#else
    QSKIP("Volume mount point buffers are a Windows API contract");
#endif
}

void ArchiveFinalizationTests::dummyCapacityDecreasesAfterEachRoot() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto parent = fs::path(directory.path().toStdWString());
    const std::array roots{parent / "mod-a", parent / "mod-b"};
    for (const auto& root : roots)
        writeFile(root / "existing.bsa", QByteArrayLiteral("retained archive"));
    const auto firstPlugin = roots.front() / "existing.esp";
    Finalizer finalizer;
    finalizer.capacity = [&](const fs::path&) -> std::optional<std::uintmax_t> {
        if (fs::exists(firstPlugin)) return fs::file_size(firstPlugin);
        return unlimitedCapacity;
    };
    std::size_t volumeQueries = 0;
    finalizer.volumeIdentity = [&](const fs::path&) -> std::optional<std::string> {
        ++volumeQueries;
        return "shared-volume";
    };
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(roots, evidence.workEvidence, artifacts);
    const auto& result = *evidence.finalization();
    QVERIFY(!result.failure);
    QVERIFY(result.attempts.empty());
    QCOMPARE(evidence.progress()->total(), std::size_t{0});
    QCOMPARE(volumeQueries, roots.size());
    for (const auto& root : roots) QVERIFY(fs::exists(root / "existing.esp"));
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void ArchiveFinalizationTests::finalizationCapacityWithoutOutputs() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto mod = fs::path(directory.path().toStdWString()) / "mod";
    writeFile(mod / "existing.bsa", QByteArrayLiteral("retained archive"));
    fs::create_directory(mod / "empty");
    Finalizer finalizer;
    finalizer.capacity = [](const fs::path&) -> std::optional<std::uintmax_t> { return 0; };
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{mod}, evidence.workEvidence, artifacts);
    const auto& result = *evidence.finalization();
    QCOMPARE(result.failure, std::optional{ArchiveFinalizationFailure::InsufficientCapacity});
    QVERIFY(result.attempts.empty());
    QVERIFY(result.safeToContinue);
    QVERIFY(!result.detail.empty());
    QCOMPARE(evidence.progress()->completed(), std::size_t{0});
    QCOMPARE(evidence.progress()->total(), std::size_t{0});
    QVERIFY(fs::exists(mod / "empty"));
    QVERIFY(!fs::exists(mod / "existing.esp"));
    QVERIFY(!fs::exists(mod / ".cao-staging"));
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void ArchiveFinalizationTests::disappearingLoadingPluginCapacityIsReserved_data() {
    QTest::addColumn<bool>("shortageAtPreflight");
    QTest::newRow("phase-preflight") << true;
    QTest::newRow("attempt-recheck") << false;
}

void ArchiveFinalizationTests::disappearingLoadingPluginCapacityIsReserved() {
    QFETCH(bool, shortageAtPreflight);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto mod = fs::path(directory.path().toStdWString()) / "mod";
    const auto source = mod / "textures" / "asset.dds";
    writeFile(source, QByteArrayLiteral("source bytes"));
    const auto earlierPlugin = mod / "mod.esm";
    writeFile(earlierPlugin, QByteArrayLiteral("existing loading plugin"));
    Finalizer finalizer;
    finalizer.settings.compress = false;
    finalizer.settings.createDummyPlugins = false;
    // The no-dummy run reports an Archive-only allowance independent of the fallback branch.
    const auto archiveOnlyCapacity = reportedEstimate(mod, finalizer);
    finalizer.settings.createDummyPlugins = true;
    std::size_t capacityQueries = 0;
    finalizer.capacity = [&](const fs::path&) -> std::optional<std::uintmax_t> {
        // Planning saw the Loading Plugin; it disappears before any output mutation.
        if (++capacityQueries == 1) {
            std::error_code error;
            fs::remove(earlierPlugin, error);
            if (!shortageAtPreflight) return unlimitedCapacity;
        }
        return archiveOnlyCapacity;
    };
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{mod}, evidence.workEvidence, artifacts);
    const auto& result = *evidence.finalization();
    QVERIFY(!fs::exists(earlierPlugin));
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QCOMPARE(result.attempts.front().failure,
             std::optional{ArchiveFinalizationFailure::InsufficientCapacity});
    QCOMPARE(result.attempts.front().mutation, MutationState::None);
    QVERIFY(result.mutations.empty());
    QVERIFY(archivesIn(mod).empty());
    QVERIFY(!fs::exists(mod / "mod.esp"));
    QVERIFY(fs::exists(source));
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void ArchiveFinalizationTests::finalizationPlanningObservesCancellation() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto mod = fs::path(directory.path().toStdWString()) / "mod";
    const auto source = mod / "textures" / "asset.dds";
    writeFile(source, QByteArrayLiteral("source bytes"));
    std::stop_source stop;
    stop.request_stop();
    Finalizer finalizer;
    finalizer.stop = stop.get_token();
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{mod}, evidence.workEvidence, artifacts);
    const auto& result = *evidence.finalization();
    QVERIFY(result.cancelled);
    QVERIFY(!result.failure);
    QVERIFY(result.attempts.empty());
    // Cancelled planning has no trustworthy output total to publish.
    QVERIFY(evidence.progress() == nullptr);
    QVERIFY(fs::exists(source));
    QVERIFY(!fs::exists(mod / ".cao-staging"));
    QVERIFY(artifacts.performSafetyCleanup().empty());
    QVERIFY(evidence.seal().cancellationObserved());
}

void ArchiveFinalizationTests::finalizationFreezesTotalAndCancelsBetweenOutputs_data() {
    QTest::addColumn<int>("cancelAfter");
    QTest::newRow("before-first-output") << 0;
    QTest::newRow("between-outputs") << 1;
    QTest::newRow("after-final-output") << 2;
    QTest::newRow("complete-run") << -1;
}

void ArchiveFinalizationTests::finalizationFreezesTotalAndCancelsBetweenOutputs() {
    QFETCH(int, cancelAfter);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto parent = fs::path(directory.path().toStdWString());
    const std::array roots{parent / "mod-a", parent / "mod-b"};
    for (const auto& mod : roots)
        writeFile(mod / "textures" / "asset.dds", QByteArrayLiteral("source bytes"));
    ProgressObservation observation;
    ArchiveFinalizationEvidenceFixture evidence(&observation);
    std::stop_source stop;
    Finalizer finalizer;
    finalizer.settings.createDummyPlugins = false;
    finalizer.settings.compress = false;
    finalizer.stop = stop.get_token();
    // Probes arrive as one phase preflight per root, then one recheck at the start of each
    // atomic output attempt. Stopping during an attempt's recheck lets that attempt finish.
    std::size_t probes = 0;
    bool frozenBeforeMutation = false;
    bool firstRecordedBeforeSecond = false;
    finalizer.capacity = [&](const fs::path&) -> std::optional<std::uintmax_t> {
        ++probes;
        if (probes == 1) {
            frozenBeforeMutation = evidence.progress() != nullptr &&
                                   evidence.progress()->total() == 2 &&
                                   evidence.progress()->completed() == 0;
            for (const auto& mod : roots)
                frozenBeforeMutation = frozenBeforeMutation && archivesIn(mod).empty() &&
                                       !fs::exists(mod / ".cao-staging") &&
                                       fs::exists(mod / "textures" / "asset.dds");
        }
        if (probes == 4)
            firstRecordedBeforeSecond = evidence.finalization()->attempts.size() == 1 &&
                                        archivesIn(roots[0]).size() == 1;
        if (cancelAfter >= 0 && probes == roots.size() + static_cast<std::size_t>(cancelAfter))
            stop.request_stop();
        return unlimitedCapacity;
    };
    TemporaryArtifactRegistry artifacts;
    finalizer.run(roots, evidence.workEvidence, artifacts);

    QVERIFY(frozenBeforeMutation);
    const auto attempted =
        cancelAfter < 0 ? std::size_t{2} : static_cast<std::size_t>(cancelAfter);
    if (attempted == 2) QVERIFY(firstRecordedBeforeSecond);
    const auto& result = *evidence.finalization();
    // Directory pruning belongs to finalization and must wait for the entire output plan.
    for (const auto& mod : roots) QCOMPARE(fs::exists(mod / "textures"), cancelAfter >= 0);
    QCOMPARE(result.attempts.size(), attempted);
    QCOMPARE(result.cancelled, cancelAfter >= 0);
    QVERIFY(result.safeToContinue);
    QCOMPARE(observation.updates.size(), attempted + 1);
    for (std::size_t index = 0; index < observation.updates.size(); ++index) {
        QCOMPARE(observation.updates[index].total(), std::size_t{2});
        QCOMPARE(observation.updates[index].completed(), index);
        QCOMPARE(observation.updates[index].succeeded(), index);
        QCOMPARE(observation.updates[index].failed(), std::size_t{0});
    }
    for (std::size_t index = 0; index < roots.size(); ++index) {
        QCOMPARE(archivesIn(roots[index]).size(),
                 index < attempted ? std::size_t{1} : std::size_t{0});
        QCOMPARE(fs::exists(roots[index] / "textures" / "asset.dds"), index >= attempted);
        if (index < attempted) {
            const auto& attempt = result.attempts[index];
            QVERIFY(attempt.succeeded());
            QCOMPARE(attempt.modRoot, fs::canonical(roots[index]));
            QCOMPARE(attempt.mutation, MutationState::Committed);
            QVERIFY(btu::bsa::read_archive(attempt.archivePath).has_value());
        }
    }
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void ArchiveFinalizationTests::cancellationPreservesCommittedArchiveLoadingPlugin() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto parent = fs::path(directory.path().toStdWString());
    const std::array roots{parent / "mod-a", parent / "mod-b"};
    for (const auto& mod : roots)
        writeFile(mod / "textures" / "asset.dds", QByteArrayLiteral("source bytes"));
    std::stop_source stop;
    Finalizer finalizer;
    finalizer.settings.compress = false;
    finalizer.stop = stop.get_token();
    std::size_t probes = 0;
    bool noPluginBeforeAttempts = false;
    finalizer.capacity = [&](const fs::path&) -> std::optional<std::uintmax_t> {
        // The first probe follows planning, which publishes no Loading Plugin.
        if (++probes == 1)
            noPluginBeforeAttempts =
                !fs::exists(roots[0] / "mod-a.esp") && !fs::exists(roots[1] / "mod-b.esp");
        // The third probe starts the first output attempt, which must finish atomically.
        if (probes == 3) stop.request_stop();
        return unlimitedCapacity;
    };
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(roots, evidence.workEvidence, artifacts);
    QVERIFY(noPluginBeforeAttempts);
    const auto& result = *evidence.finalization();
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QVERIFY(result.cancelled);
    QVERIFY(result.attempts.front().succeeded());
    QVERIFY(btu::bsa::read_archive(result.attempts.front().archivePath).has_value());
    QVERIFY(!fs::exists(roots[0] / "textures" / "asset.dds"));
    QVERIFY(fs::exists(roots[0] / "mod-a.esp"));
    QVERIFY(fs::exists(roots[1] / "textures" / "asset.dds"));
    QVERIFY(archivesIn(roots[1]).empty());
    QVERIFY(!fs::exists(roots[1] / "mod-b.esp"));
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void ArchiveFinalizationTests::finalizationCancelsPluginCleanupBetweenRoots() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto parent = fs::path(directory.path().toStdWString());
    const std::array roots{parent / "mod-a", parent / "mod-b"};
    for (const auto& root : roots) {
        writeFile(root / "existing.bsa", QByteArrayLiteral("retained archive"));
        fs::create_directories(root / "empty" / "nested");
    }
    const auto firstPlugin = roots.front() / "existing.esp";
    const auto secondPlugin = roots.back() / "existing.esp";
    std::stop_source stop;
    Finalizer finalizer;
    finalizer.stop = stop.get_token();
    finalizer.capacity = [&](const fs::path& root) -> std::optional<std::uintmax_t> {
        if (root == fs::canonical(roots.back()) && fs::exists(firstPlugin)) stop.request_stop();
        return unlimitedCapacity;
    };
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(roots, evidence.workEvidence, artifacts);
    const auto& result = *evidence.finalization();
    QVERIFY(result.attempts.empty());
    QVERIFY(result.cancelled);
    QVERIFY(result.safeToContinue);
    QVERIFY(fs::exists(firstPlugin));
    QVERIFY(!fs::exists(secondPlugin));
    for (const auto& root : roots) QVERIFY(fs::exists(root / "empty" / "nested"));
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void ArchiveFinalizationTests::finalizationReportsGuardedPluginRemovalFailure() {
#ifdef _WIN32
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto mod = fs::path(directory.path().toStdWString()) / "mod";
    writeFile(mod / "textures" / "asset.dds", QByteArrayLiteral("source bytes"));
    const auto plugin = mod / "existing.esp";
    writeFile(plugin, canonicalDummyBytes(btu::Game::SSE));
    Finalizer finalizer;
    finalizer.settings.createDummyPlugins = false;
    finalizer.settings.compress = false;
    finalizer.settings.deleteSources = false;
    HANDLE heldPlugin = INVALID_HANDLE_VALUE;
    finalizer.capacity = [&](const fs::path&) -> std::optional<std::uintmax_t> {
        // After planning, block only the Dummy Plugin's deletion; reads stay shared.
        if (heldPlugin == INVALID_HANDLE_VALUE)
            heldPlugin = CreateFileW(plugin.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr,
                                     OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
        return unlimitedCapacity;
    };
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{mod}, evidence.workEvidence, artifacts);
    QVERIFY(heldPlugin != INVALID_HANDLE_VALUE);
    QVERIFY(CloseHandle(heldPlugin));
    const auto& result = *evidence.finalization();
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QVERIFY(result.attempts.front().succeeded());
    QVERIFY(result.safeToContinue);
    QCOMPARE(result.failure, std::optional{ArchiveFinalizationFailure::PluginRemovalFailed});
    QVERIFY(result.mutations.empty());
    QVERIFY(!result.detail.empty());
    QVERIFY(fs::exists(result.attempts.front().archivePath));
    QVERIFY(fs::exists(plugin));
    QVERIFY(artifacts.performSafetyCleanup().empty());
#else
    QSKIP("Windows file sharing modes provide a deterministic post-output failure.");
#endif
}

void ArchiveFinalizationTests::finalizationRetainsPluginMutations_data() {
    QTest::addColumn<bool>("createDummies");
    QTest::newRow("create-for-existing-archive") << true;
    QTest::newRow("remove-existing-dummy") << false;
}

void ArchiveFinalizationTests::finalizationRetainsPluginMutations() {
    QFETCH(bool, createDummies);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto mod = fs::path(directory.path().toStdWString()) / "mod";
    writeFile(mod / "existing.bsa", QByteArrayLiteral("retained archive"));
    const auto plugin = mod / "existing.esp";
    if (!createDummies) writeFile(plugin, canonicalDummyBytes(btu::Game::SSE));
    Finalizer finalizer;
    finalizer.settings.createDummyPlugins = createDummies;
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{mod}, evidence.workEvidence, artifacts);
    const auto& result = *evidence.finalization();
    QVERIFY(result.attempts.empty());
    QVERIFY(!result.failure);
    QVERIFY(result.safeToContinue);
    QCOMPARE(result.mutations.size(), std::size_t{1});
    const auto& mutation = result.mutations.front();
    QCOMPARE(mutation.modRoot, fs::canonical(mod));
    QCOMPARE(mutation.path, plugin);
    QCOMPARE(mutation.kind, createDummies ? ArchiveFinalizationMutationKind::PluginCreation
                                          : ArchiveFinalizationMutationKind::PluginRemoval);
    QCOMPARE(mutation.mutation, MutationState::Committed);
    QCOMPARE(mutation.count, std::size_t{1});
    QCOMPARE(fs::exists(plugin), createDummies);
    QVERIFY(artifacts.performSafetyCleanup().empty());

    const auto terminal = evidence.seal();
    QVERIFY(terminal.archiveFinalization() != nullptr);
    QVERIFY(terminal.archiveFinalization()->attempts.empty());
    QCOMPARE(terminal.phase(RunPhase::ArchiveFinalization)->progress()->total(), std::size_t{0});
    QCOMPARE(terminal.mutationSummaries().size(), std::size_t{1});
    QCOMPARE(terminal.mutationSummaries().front().committed, std::size_t{1});
}

void ArchiveFinalizationTests::finalizationRemovesOnlyExactDummyPlugins_data() {
    QTest::addColumn<int>("game");
    QTest::newRow("SSE") << 4;
    QTest::newRow("FO4") << 5;
}

void ArchiveFinalizationTests::finalizationRemovesOnlyExactDummyPlugins() {
    QFETCH(int, game);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto parent = fs::path(directory.path().toStdWString());
    const auto mod = parent / "mod";
    writeFile(mod / "existing.bsa", QByteArrayLiteral("retained archive"));
    const auto dummy = canonicalDummyBytes(static_cast<btu::Game>(game));
    QByteArray sameSize = dummy;
    sameSize[0] = static_cast<char>(sameSize[0] ^ 0x5a);
    const auto exact = mod / "external.esp";
    const auto different = mod / "different.esp";
    const auto full = mod / "full.esm";
    const auto unrelated = parent / "outside.esp";
    writeFile(exact, dummy);
    writeFile(different, sameSize);
    writeFile(full, QByteArrayLiteral("full loading plugin"));
    writeFile(unrelated, dummy);

    Finalizer finalizer;
    finalizer.game = static_cast<btu::Game>(game);
    finalizer.settings.createDummyPlugins = false;
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{mod}, evidence.workEvidence, artifacts);
    const auto& result = *evidence.finalization();
    QVERIFY(result.attempts.empty());
    QVERIFY(!result.failure);
    QVERIFY(result.safeToContinue);
    QCOMPARE(evidence.progress()->completed(), std::size_t{0});
    QCOMPARE(evidence.progress()->total(), std::size_t{0});
    QCOMPARE(result.mutations.size(), std::size_t{1});
    QCOMPARE(result.mutations.front().modRoot, fs::canonical(mod));
    QCOMPARE(result.mutations.front().path, exact);
    QCOMPARE(result.mutations.front().kind, ArchiveFinalizationMutationKind::PluginRemoval);
    QCOMPARE(result.mutations.front().mutation, MutationState::Committed);
    QVERIFY(!fs::exists(exact));
    for (const auto& [path, expected] :
         {std::pair{different, sameSize}, std::pair{full, QByteArrayLiteral("full loading plugin")},
          std::pair{unrelated, dummy}})
        QCOMPARE(readFile(path), expected);
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void ArchiveFinalizationTests::finalizationRejectsLinkedDummyPlugins_data() {
    QTest::addColumn<bool>("symlink");
    QTest::newRow("hard-link") << false;
    QTest::newRow("file-symlink") << true;
}

void ArchiveFinalizationTests::finalizationRejectsLinkedDummyPlugins() {
#ifdef _WIN32
    QFETCH(bool, symlink);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto parent = fs::path(directory.path().toStdWString());
    const auto mod = parent / "mod";
    writeFile(mod / "existing.bsa", QByteArrayLiteral("retained archive"));
    const auto bytes = canonicalDummyBytes(btu::Game::SSE);
    const auto target = parent / "outside.bin";
    const auto plugin = mod / "external.esp";
    writeFile(target, bytes);
    std::error_code linkError;
    if (symlink)
        fs::create_symlink(target, plugin, linkError);
    else
        fs::create_hard_link(target, plugin, linkError);
    if (linkError && symlink) QSKIP("File symlink creation is unavailable on this host");
    QVERIFY2(!linkError, linkError.message().c_str());

    Finalizer finalizer;
    finalizer.settings.createDummyPlugins = false;
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{mod}, evidence.workEvidence, artifacts);
    const auto& result = *evidence.finalization();
    QVERIFY(result.attempts.empty());
    QCOMPARE(result.failure, std::optional{ArchiveFinalizationFailure::PluginRemovalFailed});
    QVERIFY(result.safeToContinue);
    QVERIFY(result.mutations.empty());
    QCOMPARE(fs::is_symlink(fs::symlink_status(plugin)), symlink);
    QCOMPARE(readFile(target), bytes);
    QVERIFY(fs::exists(plugin));
    QVERIFY(artifacts.performSafetyCleanup().empty());
#else
    QSKIP("Windows native file identity provides the link guard.");
#endif
}

void ArchiveFinalizationTests::finalizationRejectsChangedDummyPluginParent() {
#ifdef _WIN32
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto parent = fs::path(directory.path().toStdWString());
    const auto mod = parent / "mod";
    const auto outside = parent / "outside";
    const auto retained = parent / "retained-mod";
    const auto bytes = canonicalDummyBytes(btu::Game::SSE);
    writeFile(mod / "existing.bsa", QByteArrayLiteral("retained archive"));
    writeFile(mod / "original.esp", bytes);
    writeFile(outside / "external.esp", bytes);

    Finalizer finalizer;
    finalizer.settings.createDummyPlugins = false;
    bool swapped = false;
    // With no output or plugin reserve, the volume query is the phase's only probe after
    // planning froze the Mod Root; swap the root for a junction there.
    finalizer.volumeIdentity = [&](const fs::path&) -> std::optional<std::string> {
        std::error_code error;
        fs::rename(mod, retained, error);
        swapped = !error && createJunction(mod, outside);
        return "volume";
    };
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{mod}, evidence.workEvidence, artifacts);
    std::error_code cleanupError;
    fs::remove(mod, cleanupError);
    QVERIFY(swapped);
    QVERIFY2(!cleanupError, cleanupError.message().c_str());
    const auto& result = *evidence.finalization();
    QVERIFY(result.attempts.empty());
    QCOMPARE(result.failure, std::optional{ArchiveFinalizationFailure::PluginRemovalFailed});
    QVERIFY(result.safeToContinue);
    QVERIFY(result.mutations.empty());
    QVERIFY(fs::exists(retained / "original.esp"));
    QCOMPARE(readFile(outside / "external.esp"), bytes);
    QVERIFY(artifacts.performSafetyCleanup().empty());
#else
    QSKIP("Windows junctions provide the changed-parent regression case.");
#endif
}

void ArchiveFinalizationTests::finalizationPreservesReplacedDummyPlugin() {
#ifdef _WIN32
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto parent = fs::path(directory.path().toStdWString());
    const auto mod = parent / "mod";
    writeFile(mod / "textures" / "asset.dds", QByteArrayLiteral("source bytes"));
    const auto bytes = canonicalDummyBytes(btu::Game::SSE);
    QByteArray replacement = bytes;
    replacement[0] = static_cast<char>(replacement[0] ^ 0x5a);
    const auto plugin = mod / "external.esp";
    const auto displaced = parent / "displaced.bin";
    writeFile(plugin, bytes);

    Finalizer finalizer;
    finalizer.settings.createDummyPlugins = false;
    finalizer.settings.compress = false;
    finalizer.settings.deleteSources = false;
    bool replaced = false;
    finalizer.capacity = [&](const fs::path&) -> std::optional<std::uintmax_t> {
        // Planning recognized the exact dummy; replace it with other bytes before cleanup.
        if (!replaced) {
            fs::rename(plugin, displaced);
            writeFile(plugin, replacement);
            replaced = true;
        }
        return unlimitedCapacity;
    };
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{mod}, evidence.workEvidence, artifacts);
    QVERIFY(replaced);
    const auto& result = *evidence.finalization();
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QVERIFY(result.attempts.front().succeeded());
    QVERIFY(!result.failure);
    QVERIFY(result.mutations.empty());
    QCOMPARE(readFile(plugin), replacement);
    QCOMPARE(readFile(displaced), bytes);
    QVERIFY(artifacts.performSafetyCleanup().empty());
#else
    QSKIP("Windows file identity guards provide the replacement regression case.");
#endif
}

void ArchiveFinalizationTests::finalizationRetainsRemovalPrefixOnGuardFailure() {
#ifdef _WIN32
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto parent = fs::path(directory.path().toStdWString());
    const std::array roots{parent / "mod-a", parent / "mod-b"};
    const auto bytes = canonicalDummyBytes(btu::Game::SSE);
    for (const auto& root : roots) {
        writeFile(root / "existing.bsa", QByteArrayLiteral("retained archive"));
        writeFile(root / "external.esp", bytes);
    }
    const auto blocked = roots.back() / "external.esp";
    // Shared reads let planning recognize the exact dummy while its deletion stays blocked.
    const auto held = CreateFileW(blocked.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr,
                                  OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    QVERIFY(held != INVALID_HANDLE_VALUE);
    Finalizer finalizer;
    finalizer.settings.createDummyPlugins = false;
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(roots, evidence.workEvidence, artifacts);
    QVERIFY(CloseHandle(held));
    const auto& result = *evidence.finalization();
    QVERIFY(result.attempts.empty());
    QCOMPARE(result.failure, std::optional{ArchiveFinalizationFailure::PluginRemovalFailed});
    QVERIFY(result.safeToContinue);
    QCOMPARE(result.mutations.size(), std::size_t{1});
    QCOMPARE(result.mutations.front().modRoot, fs::canonical(roots.front()));
    QCOMPARE(result.mutations.front().path, roots.front() / "external.esp");
    QCOMPARE(result.mutations.front().kind, ArchiveFinalizationMutationKind::PluginRemoval);
    QCOMPARE(result.mutations.front().mutation, MutationState::Committed);
    QVERIFY(!fs::exists(roots.front() / "external.esp"));
    QVERIFY(fs::exists(blocked));
    QCOMPARE(evidence.progress()->completed(), std::size_t{0});
    QCOMPARE(evidence.progress()->total(), std::size_t{0});
    QVERIFY(artifacts.performSafetyCleanup().empty());
#else
    QSKIP("Windows sharing modes provide the guarded deletion failure.");
#endif
}

void ArchiveFinalizationTests::existingArchivePluginCollisionFailsSafely() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto mod = fs::path(directory.path().toStdWString()) / "mod";
    writeFile(mod / "existing.bsa", QByteArrayLiteral("retained archive"));
    const auto occupied = mod / "existing.esp";
    Finalizer finalizer;
    finalizer.capacity = [&](const fs::path&) -> std::optional<std::uintmax_t> {
        // A non-plugin entry takes the Loading Plugin name after planning.
        std::error_code error;
        fs::create_directory(occupied, error);
        return unlimitedCapacity;
    };
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{mod}, evidence.workEvidence, artifacts);
    const auto& result = *evidence.finalization();
    QVERIFY(result.attempts.empty());
    QCOMPARE(result.failure, std::optional{ArchiveFinalizationFailure::PluginCreationFailed});
    QVERIFY(result.safeToContinue);
    QVERIFY(result.mutations.empty());
    QCOMPARE(evidence.progress()->completed(), std::size_t{0});
    QCOMPARE(evidence.progress()->total(), std::size_t{0});
    QVERIFY(fs::is_directory(occupied));
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void ArchiveFinalizationTests::existingArchivePluginNamesFollowProfile_data() {
    QTest::addColumn<int>("game");
    QTest::addColumn<QString>("archiveName");
    QTest::newRow("SLE suffix-free") << 3 << QStringLiteral("bundle.bsa");
    QTest::newRow("SSE texture suffix") << 4 << QStringLiteral("bundle - Textures.bsa");
    QTest::newRow("FO4 main suffix") << 5 << QStringLiteral("bundle - Main.ba2");
    QTest::newRow("FO4 texture suffix") << 5 << QStringLiteral("bundle - Textures.ba2");
}

void ArchiveFinalizationTests::existingArchivePluginNamesFollowProfile() {
    QFETCH(int, game);
    QFETCH(QString, archiveName);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto mod = fs::path(directory.path().toStdWString()) / "mod";
    writeFile(mod / archiveName.toStdWString(), QByteArrayLiteral("retained archive"));
    Finalizer finalizer;
    finalizer.game = static_cast<btu::Game>(game);
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{mod}, evidence.workEvidence, artifacts);
    const auto& result = *evidence.finalization();
    QVERIFY(result.attempts.empty());
    QVERIFY(!result.failure);
    QCOMPARE(result.mutations.size(), std::size_t{1});
    const auto plugin = mod / "bundle.esp";
    QCOMPARE(result.mutations.front().modRoot, fs::canonical(mod));
    QCOMPARE(result.mutations.front().path, plugin);
    QCOMPARE(result.mutations.front().kind, ArchiveFinalizationMutationKind::PluginCreation);
    QCOMPARE(result.mutations.front().mutation, MutationState::Committed);
    QCOMPARE(readFile(plugin), canonicalDummyBytes(static_cast<btu::Game>(game)));
    QVERIFY(artifacts.performSafetyCleanup().empty());
    // Durable ownership controls remain; Safety Cleanup removes the owned run child.
    for (const auto& entry : fs::directory_iterator(mod / ".cao-staging"))
        QVERIFY(!entry.is_directory());
    QVERIFY(fs::exists(plugin));
}

void ArchiveFinalizationTests::existingArchivesShareLoadingPlugin_data() {
    QTest::addColumn<int>("preexisting");
    QTest::newRow("create-once") << 0;
    QTest::newRow("preserve-full-plugin") << 1;
    QTest::newRow("preserve-exact-dummy") << 2;
}

void ArchiveFinalizationTests::existingArchivesShareLoadingPlugin() {
    QFETCH(int, preexisting);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto mod = fs::path(directory.path().toStdWString()) / "mod";
    writeFile(mod / "bundle - Main.ba2", QByteArrayLiteral("retained main archive"));
    writeFile(mod / "bundle - Textures.ba2", QByteArrayLiteral("retained texture archive"));
    const auto dummy = mod / "bundle.esp";
    const auto full = mod / "bundle.esm";
    const auto dummyBytes = canonicalDummyBytes(btu::Game::FO4);
    const auto originalBytes =
        preexisting == 1 ? QByteArrayLiteral("full loading plugin") : dummyBytes;
    const auto original = preexisting == 1 ? full : dummy;
    if (preexisting != 0) writeFile(original, originalBytes);
    Finalizer finalizer;
    finalizer.game = btu::Game::FO4;
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{mod}, evidence.workEvidence, artifacts);
    const auto& result = *evidence.finalization();
    QVERIFY(result.attempts.empty());
    QVERIFY(!result.failure);
    QVERIFY(result.safeToContinue);
    QCOMPARE(evidence.progress()->completed(), std::size_t{0});
    QCOMPARE(evidence.progress()->total(), std::size_t{0});
    QCOMPARE(result.mutations.size(), preexisting == 0 ? std::size_t{1} : std::size_t{0});
    if (preexisting == 0) {
        QCOMPARE(result.mutations.front().path, dummy);
        QCOMPARE(result.mutations.front().mutation, MutationState::Committed);
    }
    const auto expected = preexisting == 0 ? dummy : original;
    QCOMPARE(readFile(expected), preexisting == 0 ? dummyBytes : originalBytes);
    if (preexisting == 1) QVERIFY(!fs::exists(dummy));
    QVERIFY(artifacts.performSafetyCleanup().empty());
    QVERIFY(fs::exists(expected));
}

void ArchiveFinalizationTests::packingPreservesStagingFiles() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto mod = fs::path(directory.path().toStdWString()) / "mod";
    QVERIFY(fs::create_directories(mod));
    TemporaryArtifactRegistry artifacts;
    const auto staged = artifacts.stageArchiveFile(mod).path;
    const auto nestedStaged = mod / "textures" / ".cao-staging-old" / "pending.dds";
    writeFile(staged, QByteArrayLiteral("temporary bytes"));
    writeFile(nestedStaged, QByteArrayLiteral("unverified temporary bytes"));
    writeFile(mod / "textures" / "complete.dds", QByteArrayLiteral("committed bytes"));
    Finalizer finalizer;
    finalizer.settings.createDummyPlugins = false;
    finalizer.settings.compress = false;
    ArchiveFinalizationEvidenceFixture evidence;
    finalizer.run(std::array{mod}, evidence.workEvidence, artifacts);
    const auto& result = *evidence.finalization();
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QVERIFY(result.attempts.front().succeeded());
    QVERIFY(fs::exists(staged));
    QVERIFY(fs::exists(nestedStaged));
    QVERIFY(!fs::exists(mod / "textures" / "complete.dds"));
    QVERIFY(!archivesIn(mod).empty());
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void ArchiveFinalizationTests::filesToNotPackAreNeitherPackedNorDeleted() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto mod = fs::path(directory.path().toStdWString()) / "mod";
    const auto packed = mod / "textures" / "asset.dds";
    const auto retained = mod / "textures" / "keep" / "asset.dds";
    writeFile(packed, QByteArrayLiteral("packed bytes"));
    writeFile(retained, QByteArrayLiteral("retained bytes"));
    Finalizer finalizer;
    finalizer.settings.createDummyPlugins = false;
    finalizer.settings.compress = false;
    // Profile rules use forward slashes and match native paths as case-insensitive substrings.
    finalizer.filesToNotPack = QStringList{QStringLiteral("TEXTURES/KEEP/")};
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{mod}, evidence.workEvidence, artifacts);
    const auto& result = *evidence.finalization();
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QVERIFY(result.attempts.front().succeeded());
    QVERIFY(!fs::exists(packed));
    QCOMPARE(readFile(retained), QByteArrayLiteral("retained bytes"));
    const auto archive = btu::bsa::read_archive(result.attempts.front().archivePath);
    QVERIFY(archive.has_value());
    QCOMPARE(archive->size(), std::size_t{1});
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void ArchiveFinalizationTests::failedPackingRetainsSourcesAndExistingArchives() {
#ifdef _WIN32
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto mod = fs::path(directory.path().toStdWString()) / "mod";
    const auto available = mod / "textures" / "available.dds";
    const auto unavailable = mod / "textures" / "unavailable.dds";
    const auto existingArchive = mod / "mod.bsa";
    writeFile(available, QByteArrayLiteral("available source bytes"));
    writeFile(unavailable, QByteArrayLiteral("unavailable source bytes"));
    writeFile(existingArchive, QByteArrayLiteral("previous archive bytes"));
    // Deny reads while allowing rename/delete, so a partial writer result cannot be mistaken
    // for a successful output or hidden by an unrelated quarantine sharing violation.
    const auto locked = CreateFileW(unavailable.c_str(), GENERIC_WRITE,
                                    FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr, OPEN_EXISTING,
                                    FILE_ATTRIBUTE_NORMAL, nullptr);
    QVERIFY(locked != INVALID_HANDLE_VALUE);
    Finalizer finalizer;
    finalizer.settings.createDummyPlugins = false;
    finalizer.settings.compress = false;
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{mod}, evidence.workEvidence, artifacts);
    QVERIFY(CloseHandle(locked));

    const auto& result = *evidence.finalization();
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QVERIFY(!result.attempts.front().succeeded());
    QCOMPARE(result.attempts.front().mutation, MutationState::None);
    QVERIFY(fs::exists(available));
    QVERIFY(fs::exists(unavailable));
    QCOMPARE(readFile(existingArchive), QByteArrayLiteral("previous archive bytes"));
    QVERIFY(artifacts.performSafetyCleanup().empty());
#else
    QSKIP("Windows sharing modes provide a deterministic source read failure.");
#endif
}

void ArchiveFinalizationTests::packedSourceParentJunctionIsRejected() {
#ifdef _WIN32
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto base = fs::path(directory.path().toStdWString());
    const auto mod = base / "mod";
    const auto sourceParent = mod / "textures";
    const auto source = sourceParent / "armor" / "asset.dds";
    const auto outside = base / "outside";
    const auto outsideSource = outside / "armor" / "asset.dds";
    const auto retainedParent = mod / "retained-textures";
    writeFile(source, QByteArrayLiteral("planned source bytes"));
    writeFile(outsideSource, QByteArrayLiteral("outside source bytes"));
    writeFile(outside / "marker.txt", QByteArrayLiteral("keep target directory"));
    Finalizer finalizer;
    finalizer.settings.createDummyPlugins = false;
    finalizer.settings.compress = false;
    bool swapped = false;
    finalizer.capacity = [&](const fs::path&) -> std::optional<std::uintmax_t> {
        // Swap the planned source's parent for a junction before the output attempt starts.
        if (!swapped) {
            std::error_code error;
            fs::rename(sourceParent, retainedParent, error);
            swapped = !error && createJunction(sourceParent, outside);
        }
        return unlimitedCapacity;
    };
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{mod}, evidence.workEvidence, artifacts);
    std::error_code cleanupError;
    fs::remove(sourceParent, cleanupError);
    QVERIFY(swapped);
    QVERIFY2(!cleanupError, cleanupError.message().c_str());
    const auto& result = *evidence.finalization();
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QCOMPARE(result.attempts.front().failure, std::optional{ArchiveFinalizationFailure::WriteFailed});
    QCOMPARE(result.attempts.front().mutation, MutationState::None);
    QVERIFY(!fs::exists(result.attempts.front().archivePath));
    QVERIFY(fs::exists(retainedParent / "armor" / "asset.dds"));
    QCOMPARE(readFile(outsideSource), QByteArrayLiteral("outside source bytes"));
    QVERIFY(artifacts.performSafetyCleanup().empty());
#else
    QSKIP("Windows junctions provide the parent substitution regression case.");
#endif
}

void ArchiveFinalizationTests::plannedNamesAreDistinctAndCommitPreservesNewDestination() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto mod = fs::path(directory.path().toStdWString()) / "mod";
    const auto mesh = mod / "meshes" / "asset.nif";
    const auto sound = mod / "sound" / "asset.wav";
    writeFile(mesh, QByteArrayLiteral("mesh bytes"));
    writeFile(sound, QByteArrayLiteral("sound bytes"));
    ProgressObservation observation;
    ArchiveFinalizationEvidenceFixture evidence(&observation);
    Finalizer finalizer;
    finalizer.settings.createDummyPlugins = false;
    finalizer.settings.compress = false;
    finalizer.settings.mergeIncompressible = false;
    finalizer.settings.mergeTextures = false;
    // The standard Archive takes the Mod Root's name; the incompressible one needs another.
    const auto firstDestination = mod / "mod.bsa";
    bool competed = false;
    finalizer.capacity = [&](const fs::path&) -> std::optional<std::uintmax_t> {
        // Simulate a competing creator after planning and before the first attempt.
        if (!competed) {
            writeFile(firstDestination, QByteArrayLiteral("competing creator bytes"));
            competed = true;
        }
        return unlimitedCapacity;
    };
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{mod}, evidence.workEvidence, artifacts);
    const auto& result = *evidence.finalization();
    QCOMPARE(result.attempts.size(), std::size_t{2});
    QVERIFY(result.safeToContinue);
    QVERIFY(!result.cancelled);
    QCOMPARE(result.attempts[0].archivePath, firstDestination);
    QVERIFY(result.attempts[1].archivePath != firstDestination);
    QVERIFY(!result.attempts[0].succeeded());
    QCOMPARE(result.attempts[0].mutation, MutationState::None);
    QVERIFY(result.attempts[1].succeeded());
    QCOMPARE(result.attempts[1].mutation, MutationState::Committed);
    QCOMPARE(observation.updates.size(), std::size_t{3});
    QCOMPARE(observation.updates[1].total(), std::size_t{2});
    QCOMPARE(observation.updates[1].completed(), std::size_t{1});
    QCOMPARE(observation.updates[1].failed(), std::size_t{1});
    QCOMPARE(observation.updates[1].succeeded(), std::size_t{0});
    QCOMPARE(observation.updates.back().completed(), std::size_t{2});
    QCOMPARE(observation.updates.back().failed(), std::size_t{1});
    QCOMPARE(observation.updates.back().succeeded(), std::size_t{1});
    QVERIFY(fs::exists(mesh));
    QVERIFY(!fs::exists(sound));
    QCOMPARE(readFile(firstDestination), QByteArrayLiteral("competing creator bytes"));
    QVERIFY(btu::bsa::read_archive(result.attempts[1].archivePath).has_value());
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void ArchiveFinalizationTests::plannedOutputNamesUseExactDummyBytes_data() {
    QTest::addColumn<int>("game");
    QTest::addColumn<QString>("archiveName");
    QTest::newRow("SLE suffix-free") << 3 << QStringLiteral("loader.bsa");
    QTest::newRow("SSE texture suffix") << 4 << QStringLiteral("loader - Textures.bsa");
    QTest::newRow("FO4 texture suffix") << 5 << QStringLiteral("loader - Textures.ba2");
}

void ArchiveFinalizationTests::plannedOutputNamesUseExactDummyBytes() {
    QFETCH(int, game);
    QFETCH(QString, archiveName);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto mod = fs::path(directory.path().toStdWString()) / "mod";
    // BA2 texture Archives parse their DDS sources, so every profile needs a real Texture.
    writeTexture(mod / "textures" / "asset.dds");
    const auto dummy = canonicalDummyBytes(static_cast<btu::Game>(game));
    writeFile(mod / "dummy.esp", dummy);
    writeFile(mod / "loader.esm", QByteArray(dummy.size(), '\0'));
    Finalizer finalizer;
    finalizer.game = static_cast<btu::Game>(game);
    finalizer.settings.compress = false;
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{mod}, evidence.workEvidence, artifacts);
    const auto& result = *evidence.finalization();
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QVERIFY2(result.attempts.front().succeeded(), result.attempts.front().detail.c_str());
    QCOMPARE(result.attempts.front().archivePath.filename(),
             fs::path(archiveName.toStdWString()));
    // The same-size full plugin loads the output, so no Dummy Plugin is needed.
    for (const auto& mutation : result.mutations)
        QVERIFY(mutation.kind != ArchiveFinalizationMutationKind::PluginCreation);
    QVERIFY(!fs::exists(mod / "loader.esp"));
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void ArchiveFinalizationTests::plannedOutputRechecksLoadingPluginNames_data() {
    QTest::addColumn<int>("game");
    QTest::addColumn<QString>("pluginName");
    QTest::newRow("SLE suffix-free") << 3 << QStringLiteral("mod.esm");
    QTest::newRow("SSE texture suffix") << 4 << QStringLiteral("mod - Textures.esl");
    QTest::newRow("FO4 suffix-free") << 5 << QStringLiteral("mod.esm");
}

void ArchiveFinalizationTests::plannedOutputRechecksLoadingPluginNames() {
    QFETCH(int, game);
    QFETCH(QString, pluginName);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto mod = fs::path(directory.path().toStdWString()) / "mod";
    const auto source =
        mod / (game == 5 ? "meshes" : "textures") / (game == 5 ? "asset.nif" : "asset.dds");
    writeFile(source, QByteArrayLiteral("source bytes"));
    const auto alternatePlugin = mod / pluginName.toStdWString();
    std::optional<fs::file_time_type> originalWriteTime;
    Finalizer finalizer;
    finalizer.game = static_cast<btu::Game>(game);
    finalizer.settings.compress = false;
    finalizer.capacity = [&](const fs::path&) -> std::optional<std::uintmax_t> {
        // Planning found no Loading Plugin; one appears at another recognized name.
        if (!originalWriteTime) {
            writeFile(alternatePlugin, QByteArrayLiteral("existing loading plugin"));
            originalWriteTime = fs::last_write_time(alternatePlugin) - std::chrono::hours(24);
            fs::last_write_time(alternatePlugin, *originalWriteTime);
        }
        return unlimitedCapacity;
    };
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{mod}, evidence.workEvidence, artifacts);
    QVERIFY(originalWriteTime.has_value());
    const auto& result = *evidence.finalization();
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QVERIFY2(result.attempts.front().succeeded(), result.attempts.front().detail.c_str());
    QCOMPARE(result.attempts.front().mutation, MutationState::Committed);
    for (const auto& mutation : result.mutations)
        QVERIFY(mutation.kind != ArchiveFinalizationMutationKind::PluginCreation);
    QVERIFY(btu::bsa::read_archive(result.attempts.front().archivePath).has_value());
    QVERIFY(!fs::exists(source));
    // The planned fallback Dummy Plugin name stays free.
    QVERIFY(!fs::exists(mod / "mod.esp"));
    QCOMPARE(fs::last_write_time(alternatePlugin), *originalWriteTime);
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void ArchiveFinalizationTests::plannedPluginCollisionRetainsCommittedArchive() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto mod = fs::path(directory.path().toStdWString()) / "mod";
    const auto source = mod / "textures" / "asset.dds";
    writeFile(source, QByteArrayLiteral("source bytes"));
    const auto plannedPlugin = mod / "mod.esp";
    Finalizer finalizer;
    finalizer.settings.compress = false;
    bool occupied = false;
    finalizer.capacity = [&](const fs::path&) -> std::optional<std::uintmax_t> {
        // A competitor takes the planned Dummy Plugin name after planning.
        if (!occupied) {
            writeFile(plannedPlugin, QByteArrayLiteral("competing plugin bytes"));
            occupied = true;
        }
        return unlimitedCapacity;
    };
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{mod}, evidence.workEvidence, artifacts);
    const auto& result = *evidence.finalization();
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QCOMPARE(result.attempts.front().failure,
             std::optional{ArchiveFinalizationFailure::PluginCreationFailed});
    QCOMPARE(result.attempts.front().mutation, MutationState::Committed);
    QVERIFY(!result.attempts.front().safeToContinue);
    QVERIFY(!result.safeToContinue);
    QVERIFY(result.mutations.empty());
    QVERIFY(btu::bsa::read_archive(result.attempts.front().archivePath).has_value());
    QVERIFY(fs::exists(source));
    QCOMPARE(readFile(plannedPlugin), QByteArrayLiteral("competing plugin bytes"));
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void ArchiveFinalizationTests::plannedExactDummyIsReused() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto mod = fs::path(directory.path().toStdWString()) / "mod";
    const auto source = mod / "textures" / "asset.dds";
    writeFile(source, QByteArrayLiteral("source bytes"));
    const auto plannedPlugin = mod / "mod.esp";
    const auto dummy = canonicalDummyBytes(btu::Game::SSE);
    std::optional<fs::file_time_type> originalWriteTime;
    Finalizer finalizer;
    finalizer.settings.compress = false;
    finalizer.capacity = [&](const fs::path&) -> std::optional<std::uintmax_t> {
        // An exact Dummy Plugin appears at the planned name after planning.
        if (!originalWriteTime) {
            writeFile(plannedPlugin, dummy);
            originalWriteTime = fs::last_write_time(plannedPlugin) - std::chrono::hours(24);
            fs::last_write_time(plannedPlugin, *originalWriteTime);
        }
        return unlimitedCapacity;
    };
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{mod}, evidence.workEvidence, artifacts);
    QVERIFY(originalWriteTime.has_value());
    const auto& result = *evidence.finalization();
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QVERIFY(result.attempts.front().succeeded());
    QCOMPARE(result.attempts.front().mutation, MutationState::Committed);
    QVERIFY(btu::bsa::read_archive(result.attempts.front().archivePath).has_value());
    QVERIFY(!fs::exists(source));
    QCOMPARE(fs::last_write_time(plannedPlugin), *originalWriteTime);
    QCOMPARE(readFile(plannedPlugin), dummy);
    for (const auto& mutation : result.mutations)
        QVERIFY(mutation.kind != ArchiveFinalizationMutationKind::PluginCreation);
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void ArchiveFinalizationTests::plannedOutputsRecordSharedDummyPublication() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto mod = fs::path(directory.path().toStdWString()) / "mod";
    writeFile(mod / "meshes" / "asset.nif", QByteArrayLiteral("mesh bytes"));
    writeFile(mod / "textures" / "asset.dds", QByteArrayLiteral("texture bytes"));
    Finalizer finalizer;
    finalizer.settings.compress = false;
    finalizer.settings.deleteSources = false;
    finalizer.settings.mergeTextures = false;
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{mod}, evidence.workEvidence, artifacts);
    const auto& result = *evidence.finalization();
    QCOMPARE(result.attempts.size(), std::size_t{2});
    for (const auto& attempt : result.attempts) {
        QVERIFY2(attempt.succeeded(), attempt.detail.c_str());
        QCOMPARE(attempt.mutation, MutationState::Committed);
    }
    QCOMPARE(result.mutations.size(), std::size_t{1});
    const auto& mutation = result.mutations.front();
    QCOMPARE(mutation.modRoot, fs::canonical(mod));
    QCOMPARE(mutation.path, mod / "mod.esp");
    QCOMPARE(mutation.kind, ArchiveFinalizationMutationKind::PluginCreation);
    QCOMPARE(mutation.mutation, MutationState::Committed);
    QCOMPARE(readFile(mutation.path), canonicalDummyBytes(btu::Game::SSE));
    QVERIFY(artifacts.performSafetyCleanup().empty());

    const auto sealed = evidence.seal();
    QCOMPARE(sealed.mutationSummaries().size(), std::size_t{1});
    QCOMPARE(sealed.mutationSummaries().front().modRoot, fs::canonical(mod));
    QCOMPARE(sealed.mutationSummaries().front().kind, cao::run::MutationKind::ArchiveFinalization);
    QCOMPARE(sealed.mutationSummaries().front().committed, std::size_t{3});
    QCOMPARE(sealed.mutationSummaries().front().partialOrUnknown, std::size_t{0});
}

void ArchiveFinalizationTests::plannedLoadingPluginDisappearsBeforeCommit() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto mod = fs::path(directory.path().toStdWString()) / "mod";
    const auto source = mod / "textures" / "asset.dds";
    writeFile(source, QByteArrayLiteral("source bytes"));
    const auto earlierPlugin = mod / "mod.esm";
    writeFile(earlierPlugin, QByteArrayLiteral("existing loading plugin"));
    Finalizer finalizer;
    finalizer.settings.compress = false;
    bool removed = false;
    finalizer.capacity = [&](const fs::path&) -> std::optional<std::uintmax_t> {
        // Planning saw the Loading Plugin; it disappears before the output attempt.
        if (!removed) removed = fs::remove(earlierPlugin);
        return unlimitedCapacity;
    };
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{mod}, evidence.workEvidence, artifacts);
    QVERIFY(removed);
    const auto& result = *evidence.finalization();
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QVERIFY2(result.attempts.front().succeeded(), result.attempts.front().detail.c_str());
    QCOMPARE(result.attempts.front().mutation, MutationState::Committed);
    QVERIFY(!fs::exists(source));
    std::size_t pluginCreations = 0;
    for (const auto& mutation : result.mutations) {
        if (mutation.kind != ArchiveFinalizationMutationKind::PluginCreation) continue;
        ++pluginCreations;
        QCOMPARE(mutation.modRoot, fs::canonical(mod));
        QCOMPARE(mutation.path, mod / "mod.esp");
        QCOMPARE(mutation.mutation, MutationState::Committed);
    }
    QCOMPARE(pluginCreations, std::size_t{1});
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void ArchiveFinalizationTests::linkedLoadingPluginRemainsUsable_data() {
    QTest::addColumn<QString>("linkedName");
    QTest::newRow("alternate-name") << QStringLiteral("mod.esm");
    QTest::newRow("dummy-destination") << QStringLiteral("mod.esp");
}

void ArchiveFinalizationTests::linkedLoadingPluginRemainsUsable() {
    QFETCH(QString, linkedName);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto parent = fs::path(directory.path().toStdWString());
    const auto mod = parent / "mod";
    const auto source = mod / "textures" / "asset.dds";
    writeFile(source, QByteArrayLiteral("source bytes"));
    const auto target = parent / "real-plugin.bin";
    writeFile(target, QByteArrayLiteral("real loading plugin bytes"));
    const auto link = mod / linkedName.toStdWString();
    std::error_code linkError;
    fs::create_symlink(target, link, linkError);
    if (linkError) QSKIP("File symlink creation is unavailable on this host");

    Finalizer finalizer;
    finalizer.settings.compress = false;
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{mod}, evidence.workEvidence, artifacts);
    const auto& result = *evidence.finalization();
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QVERIFY2(result.attempts.front().succeeded(), result.attempts.front().detail.c_str());
    QVERIFY(btu::bsa::read_archive(result.attempts.front().archivePath).has_value());
    QVERIFY(!fs::exists(source));
    QVERIFY(fs::is_symlink(fs::symlink_status(link)));
#ifdef _WIN32
    // The output "mod - Textures.bsa" loads through either stem with any SSE plugin extension.
    std::vector<fs::path> recognized;
    for (const auto& extension : btu::bsa::Settings::get(btu::Game::SSE).plugin_extensions) {
        recognized.push_back(mod / (u8"mod - Textures" + extension));
        recognized.push_back(mod / (u8"mod" + extension));
    }
    std::size_t createdPlugins = 0;
    fs::path fallback;
    for (const auto& mutation : result.mutations) {
        if (mutation.kind != ArchiveFinalizationMutationKind::PluginCreation) continue;
        ++createdPlugins;
        fallback = mutation.path;
        QVERIFY(fallback != link);
        QVERIFY(std::find(recognized.begin(), recognized.end(), fallback) != recognized.end());
        QVERIFY(fs::is_regular_file(fs::symlink_status(fallback)));
    }
    QCOMPARE(createdPlugins, std::size_t{1});
#else
    if (link != mod / "mod.esp") QVERIFY(!fs::exists(mod / "mod.esp"));
    for (const auto& mutation : result.mutations)
        QVERIFY(mutation.kind != ArchiveFinalizationMutationKind::PluginCreation);
#endif
    QVERIFY(artifacts.performSafetyCleanup().empty());
    QVERIFY(fs::remove(link));
#ifdef _WIN32
    QVERIFY(fs::exists(fallback));
#endif
}

void ArchiveFinalizationTests::linkedLoadingPluginsWithoutFallbackRetainSources() {
#ifdef _WIN32
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto parent = fs::path(directory.path().toStdWString());
    const auto mod = parent / "mod";
    const auto source = mod / "textures" / "asset.dds";
    writeFile(source, QByteArrayLiteral("source bytes"));
    const auto target = parent / "real-plugin.bin";
    writeFile(target, QByteArrayLiteral("linked loading plugin"));
    if (!fileSymlinksAvailable(parent)) QSKIP("File symlink creation is unavailable on this host");

    // The output "mod - Textures.bsa" loads through either stem with any SSE plugin extension.
    std::vector<fs::path> recognized;
    for (const auto& extension : btu::bsa::Settings::get(btu::Game::SSE).plugin_extensions) {
        recognized.push_back(mod / (u8"mod - Textures" + extension));
        recognized.push_back(mod / (u8"mod" + extension));
    }
    Finalizer finalizer;
    finalizer.settings.compress = false;
    bool linked = false;
    finalizer.capacity = [&](const fs::path&) -> std::optional<std::uintmax_t> {
        // After planning, links occupy every recognized Loading Plugin name.
        if (!linked) {
            linked = true;
            for (const auto& path : recognized) {
                std::error_code error;
                fs::create_symlink(target, path, error);
                linked = linked && !error;
            }
        }
        return unlimitedCapacity;
    };
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{mod}, evidence.workEvidence, artifacts);
    QVERIFY(linked);
    const auto& result = *evidence.finalization();
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QCOMPARE(result.attempts.front().failure,
             std::optional{ArchiveFinalizationFailure::PluginCreationFailed});
    QVERIFY(!result.attempts.front().succeeded());
    QVERIFY(fs::exists(source));
    QVERIFY(btu::bsa::read_archive(result.attempts.front().archivePath).has_value());
    QVERIFY(artifacts.performSafetyCleanup().empty());
    for (const auto& path : recognized) fs::remove(path);
#else
    QSKIP("Windows reparse points require an ordinary Loading Plugin fallback.");
#endif
}

void ArchiveFinalizationTests::loadingPluginRemainsPinnedThroughSourceCleanup_data() {
    QTest::addColumn<int>("pluginKind");
    QTest::newRow("existing-plugin") << 0;
    QTest::newRow("published-fallback") << 1;
}

void ArchiveFinalizationTests::loadingPluginRemainsPinnedThroughSourceCleanup() {
#ifdef _WIN32
    QFETCH(int, pluginKind);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto mod = fs::path(directory.path().toStdWString()) / "mod";
    // A long cleanup makes the interval after plugin selection observable without a test hook.
    std::vector<fs::path> sources;
    for (int index = 0; index < 1024; ++index) {
        sources.push_back(mod / "textures" / ("asset-" + std::to_string(index) + ".dds"));
        writeFile(sources.back(), QByteArrayLiteral("source bytes"));
    }
    std::sort(sources.begin(), sources.end());
    const auto plugin = mod / (pluginKind == 1 ? "mod.esp" : "mod.esm");
    if (pluginKind == 0) writeFile(plugin, QByteArrayLiteral("existing loading plugin"));

    Finalizer finalizer;
    finalizer.settings.compress = false;
    bool sawCleanupGap = false;
    bool removedPlugin = false;
    std::jthread remover([&](const std::stop_token stop) {
        while (!stop.stop_requested()) {
            std::error_code firstError;
            std::error_code lastError;
            // Cleanup is in progress once exactly one end of the ordered source set is gone.
            const auto first = fs::exists(sources.front(), firstError);
            const auto last = fs::exists(sources.back(), lastError);
            if (!firstError && !lastError && first != last) {
                sawCleanupGap = true;
                removedPlugin = DeleteFileW(plugin.c_str()) != 0;
                break;
            }
            std::this_thread::yield();
        }
    });
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{mod}, evidence.workEvidence, artifacts);
    remover.request_stop();
    remover.join();

    const auto& result = *evidence.finalization();
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QVERIFY2(result.attempts.front().succeeded(), result.attempts.front().detail.c_str());
    QVERIFY2(sawCleanupGap, "The test did not observe source cleanup in progress.");
    QVERIFY(!removedPlugin);
    QVERIFY(btu::bsa::read_archive(result.attempts.front().archivePath).has_value());
    for (const auto& source : sources) QVERIFY(!fs::exists(source));
    QVERIFY(fs::is_regular_file(plugin));
    QVERIFY(DeleteFileW(plugin.c_str()));
    QVERIFY(artifacts.performSafetyCleanup().empty());
#else
    QSKIP("Windows file sharing pins Loading Plugins during cleanup.");
#endif
}

void ArchiveFinalizationTests::committedArchiveRetainsLockedSource_data() {
    QTest::addColumn<bool>("denyReads");
    QTest::newRow("readable-source-continues") << false;
    QTest::newRow("unreadable-source-stops") << true;
}

void ArchiveFinalizationTests::committedArchiveRetainsLockedSource() {
#ifdef _WIN32
    QFETCH(bool, denyReads);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto parent = fs::path(directory.path().toStdWString());
    const auto mod = parent / "mod-a";
    const auto laterMod = parent / "mod-b";
    const auto source = mod / "textures" / "asset.dds";
    writeFile(source, QByteArrayLiteral("retained source bytes"));
    const auto laterSource = laterMod / "textures" / "later.dds";
    writeFile(laterSource, QByteArrayLiteral("later source bytes"));
    const auto emptyDirectory = mod / "empty" / "nested";
    fs::create_directories(emptyDirectory);
    // Permit packing and recovery verification reads but deny source deletion after commit.
    const auto locked = CreateFileW(source.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr,
                                    OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    QVERIFY(locked != INVALID_HANDLE_VALUE);
    // Windows byte locks permit the Archive writer's memory mapping, but reject ordinary
    // retained-source reads. This distinguishes existence from actual recovery evidence.
    OVERLAPPED range{};
    const bool rangeLocked =
        !denyReads || LockFileEx(locked, LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY, 0,
                                 MAXDWORD, MAXDWORD, &range) != 0;
    if (!rangeLocked) CloseHandle(locked);
    QVERIFY(rangeLocked);
    Finalizer finalizer;
    finalizer.settings.createDummyPlugins = false;
    finalizer.settings.compress = false;
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{mod, laterMod}, evidence.workEvidence, artifacts);
    QVERIFY(CloseHandle(locked));
    const auto& result = *evidence.finalization();
    QCOMPARE(result.attempts.size(), denyReads ? std::size_t{1} : std::size_t{2});
    QVERIFY(!result.attempts.front().succeeded());
    QCOMPARE(result.attempts.front().failure,
             std::optional{ArchiveFinalizationFailure::SourceCleanupFailed});
    QCOMPARE(result.attempts.front().mutation, MutationState::Committed);
    QCOMPARE(result.safeToContinue, !denyReads);
    QVERIFY(btu::bsa::read_archive(result.attempts.front().archivePath).has_value());
    QCOMPARE(fs::exists(laterSource), denyReads);
    QCOMPARE(archivesIn(laterMod).size(), denyReads ? std::size_t{0} : std::size_t{1});
    QCOMPARE(fs::exists(emptyDirectory), denyReads);
    if (!denyReads) {
        QVERIFY(result.attempts.back().succeeded());
        QVERIFY(btu::bsa::read_archive(result.attempts.back().archivePath).has_value());
    }
    QCOMPARE(readFile(source), QByteArrayLiteral("retained source bytes"));
    QVERIFY(artifacts.performSafetyCleanup().empty());
#else
    QSKIP("Windows sharing modes provide a deterministic source cleanup failure.");
#endif
}

void ArchiveFinalizationTests::packingRequiresArchiveCreationRequest() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto mod = fs::path(directory.path().toStdWString()) / "mod";
    const auto source = mod / "textures" / "asset.dds";
    writeFile(source, QByteArrayLiteral("source bytes"));
    writeFile(mod / "existing.bsa", QByteArrayLiteral("retained archive"));
    fs::create_directories(mod / "empty" / "nested");
    Finalizer finalizer;
    finalizer.createArchives = false;
    std::size_t probes = 0;
    finalizer.capacity = [&](const fs::path&) -> std::optional<std::uintmax_t> {
        ++probes;
        return unlimitedCapacity;
    };
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{mod}, evidence.workEvidence, artifacts);
    const auto& result = *evidence.finalization();
    QVERIFY(result.attempts.empty());
    QVERIFY(!result.failure);
    QVERIFY(result.safeToContinue);
    QCOMPARE(evidence.progress()->total(), std::size_t{0});
    // Neither packing nor Loading Plugin maintenance for the existing Archive ran.
    QCOMPARE(probes, std::size_t{0});
    QVERIFY(fs::exists(source));
    QCOMPARE(archivesIn(mod), std::vector<fs::path>{mod / "existing.bsa"});
    QVERIFY(!fs::exists(mod / "existing.esp"));
    QVERIFY(!fs::exists(mod / ".cao-staging"));
    // Empty-directory pruning still runs without an Archive creation request.
    QVERIFY(!fs::exists(mod / "empty"));
    QCOMPARE(result.mutations.size(), std::size_t{1});
    QCOMPARE(result.mutations.front().kind, ArchiveFinalizationMutationKind::EmptyDirectoryPruning);
    QCOMPARE(result.mutations.front().mutation, MutationState::Committed);
    QCOMPARE(result.mutations.front().count, std::size_t{2});
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void ArchiveFinalizationTests::unexpectedExceptionIsRecordedOnce_data() {
    QTest::addColumn<bool>("afterPlan");
    QTest::newRow("planning") << false;
    QTest::newRow("after-output-total") << true;
}

void ArchiveFinalizationTests::unexpectedExceptionIsRecordedOnce() {
    QFETCH(bool, afterPlan);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto parent = fs::path(directory.path().toStdWString());
    const auto mod = parent / "mod";
    const auto source = mod / "textures" / "asset.dds";
    writeFile(source, QByteArrayLiteral("source bytes"));
    Finalizer finalizer;
    finalizer.volumeIdentity = [](const fs::path&) -> std::optional<std::string> {
        throw std::runtime_error("volume query failed");
    };
    // A missing Mod Root fails planning; the throwing volume query fails after the plan.
    const auto root = afterPlan ? mod : parent / "missing";
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{root}, evidence.workEvidence, artifacts);
    const auto* result = evidence.finalization();
    QVERIFY(result != nullptr);
    QCOMPARE(result->failure, std::optional{ArchiveFinalizationFailure::UnexpectedException});
    QVERIFY(!result->safeToContinue);
    QVERIFY(!result->detail.empty());
    if (afterPlan) QCOMPARE(result->detail, std::string("volume query failed"));
    QVERIFY(result->attempts.empty());
    if (afterPlan)
        QCOMPARE(evidence.progress()->total(), std::size_t{1});
    else
        QVERIFY(evidence.progress() == nullptr);
    QVERIFY(fs::exists(source));
    QVERIFY(archivesIn(mod).empty());
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void ArchiveFinalizationTests::cleanupExceptionKeepsRecordedAttempts() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto parent = fs::path(directory.path().toStdWString());
    const auto packed = parent / "mod-a";
    const auto existing = parent / "mod-b";
    writeFile(packed / "textures" / "asset.dds", QByteArrayLiteral("source bytes"));
    writeFile(existing / "existing.bsa", QByteArrayLiteral("retained archive"));
    Finalizer finalizer;
    finalizer.settings.compress = false;
    bool moved = false;
    finalizer.capacity = [&](const fs::path& root) -> std::optional<std::uintmax_t> {
        // The existing Archive's plugin reserve is rechecked after mod-a's output committed;
        // removing mod-b then makes its Loading Plugin maintenance throw.
        if (root == fs::canonical(existing) && !archivesIn(packed).empty()) {
            std::error_code error;
            fs::rename(existing, parent / "moved", error);
            moved = !error;
        }
        return unlimitedCapacity;
    };
    ArchiveFinalizationEvidenceFixture evidence;
    TemporaryArtifactRegistry artifacts;
    finalizer.run(std::array{packed, existing}, evidence.workEvidence, artifacts);
    QVERIFY(moved);
    const auto& result = *evidence.finalization();
    QCOMPARE(result.failure, std::optional{ArchiveFinalizationFailure::UnexpectedException});
    QVERIFY(!result.safeToContinue);
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QVERIFY(result.attempts.front().succeeded());
    QCOMPARE(result.attempts.front().mutation, MutationState::Committed);
    QCOMPARE(evidence.progress()->completed(), std::size_t{1});
    QVERIFY(btu::bsa::read_archive(result.attempts.front().archivePath).has_value());
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void ArchiveFinalizationTests::evidenceFailuresPropagateUnchanged_data() {
    QTest::addColumn<int>("stage");
    QTest::newRow("before-output-total") << 0;
    QTest::newRow("capacity-rejection") << 1;
    QTest::newRow("committed-output") << 2;
}

void ArchiveFinalizationTests::evidenceFailuresPropagateUnchanged() {
    QFETCH(int, stage);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto mod = fs::path(directory.path().toStdWString()) / "mod";
    const auto source = mod / "textures" / "asset.dds";
    writeFile(source, QByteArrayLiteral("source bytes"));
    ArchiveFinalizationEvidenceFixture evidence;
    // Leaving the phase makes the next Archive Finalization record an invariant violation.
    const auto leavePhase = [&] {
        evidence.evidence.recordPhase(RunPhaseRecord::executed(RunPhase::SafetyCleanup));
    };
    if (stage == 0) leavePhase();
    Finalizer finalizer;
    finalizer.settings.createDummyPlugins = false;
    finalizer.settings.compress = false;
    std::size_t probes = 0;
    finalizer.capacity = [&](const fs::path&) -> std::optional<std::uintmax_t> {
        // The second probe opens the output attempt, after the preflight passed.
        if (++probes == 2 && stage != 0) {
            leavePhase();
            if (stage == 1) return 0;
        }
        return unlimitedCapacity;
    };
    TemporaryArtifactRegistry artifacts;
    const auto runPhase = [&] { finalizer.run(std::array{mod}, evidence.workEvidence, artifacts); };
    QVERIFY_EXCEPTION_THROWN(runPhase(), cao::run::RunEvidenceInvariantViolation);
    // Evidence failures are never converted into a finalization result.
    QVERIFY(evidence.finalization() == nullptr || !evidence.finalization()->failure);
    QCOMPARE(fs::exists(source), stage != 2);
    QCOMPARE(archivesIn(mod).size(), stage == 2 ? std::size_t{1} : std::size_t{0});
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

QTEST_GUILESS_MAIN(ArchiveFinalizationTests)

#include "ArchiveFinalizationTests.moc"
