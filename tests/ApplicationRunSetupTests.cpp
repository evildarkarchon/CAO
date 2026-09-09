#include "FilesystemOperations.h"
#include "Profiles.h"
#include "Run/ApplicationRunSetup.h"
#include "OptimizerProfileSnapshot.h"

#include <QTemporaryDir>
#include <QTest>
#include <future>

using cao::routing::RequestedWork;

namespace
{
/// Disables every option whose prior processing path treated the Texture group as selected.
void disableTextureWork(OptionsCAO &options)
{
    options.bTexturesNecessary = false;
    options.bTexturesCompress = false;
    options.bTexturesMipmaps = false;
    options.bTexturesResizeSize = false;
    options.bTexturesResizeRatio = false;
}
}

// Profiles::create is not exercised here; this satisfies its otherwise unrelated link dependency.
void FilesystemOperations::copyDir(const QString &, const QString &, bool)
{
    // The profile fixtures are copied explicitly before Profiles is initialized.
}

class ApplicationRunSetupTests final : public QObject
{
    Q_OBJECT

private slots:
    /// Copies the shipped profiles into an isolated working directory before the singleton initializes.
    void initTestCase();
    /// Restores the process working directory after the isolated profile checks complete.
    void cleanupTestCase();
    /// Verifies a conversion-enabled profile cannot select conversion when Texture work is disabled.
    void disabledTextureWorkDoesNotRequestProfileConversion();
    /// Verifies the shipped FO4 conversion profile compiles without enabling Mesh optimization.
    void fo4ConversionCompilesWithoutMeshOptimization();
    /// Covers resave-only, optimization-only, combined, and disabled Mesh work choices.
    void meshWorkRoutesStandardAndTerrainMeshes_data();
    /// Verifies application choices route both Mesh variants even for resave-only runs.
    void meshWorkRoutesStandardAndTerrainMeshes();
    /// Verifies requesting Archive creation under an Archive-disabled profile fails setup.
    void archiveCreationRequiresProfileArchiveSupport();
    /// Keeps request intent stable when later caller options and profile selection change.
    void requestOwnsCallerIntent();
    /// Loads the named profile and fallback exclusions independently of subsequent UI selection.
    void providerLoadsOwnedConfiguration();
    /// Covers numeric choices that parsing accepts but optimization cannot safely execute.
    void requestRejectsInvalidOptionValues_data();
    /// Rejects invalid option values without attempting filesystem preparation.
    void requestRejectsInvalidOptionValues();
    /// Defers auxiliary file reads until preparation, retaining the caller's selected profile.
    void optimizerProfileLoadsAuxiliaryFilesAfterCapture();

private:
    QString _originalCurrentPath;
    QTemporaryDir _workingDirectory;
};

void ApplicationRunSetupTests::initTestCase()
{
    QVERIFY(_workingDirectory.isValid());
    _originalCurrentPath = QDir::currentPath();

    QDir fixtureRoot(_workingDirectory.path());
    for (const auto &profile : {QStringLiteral("SSE"), QStringLiteral("FO4")}) {
        const auto profileDirectory = QStringLiteral("profiles/") + profile;
        QVERIFY(fixtureRoot.mkpath(profileDirectory));
        const auto source = QStringLiteral(CAO_SOURCE_DIR "/profiles/") + profile
                            + QStringLiteral("/profile.ini");
        const auto destination = fixtureRoot.filePath(profileDirectory
                                                       + QStringLiteral("/profile.ini"));
        QVERIFY2(QFile::copy(source, destination), qPrintable(source));
    }

    // A profile that declares no Archive support at all; the shipped profiles all enable BSAs.
    const auto noArchivesDirectory = QStringLiteral("profiles/NoArchives");
    QVERIFY(fixtureRoot.mkpath(noArchivesDirectory));
    const auto noArchivesProfile = fixtureRoot.filePath(noArchivesDirectory
                                                        + QStringLiteral("/profile.ini"));
    QVERIFY(QFile::copy(QStringLiteral(CAO_SOURCE_DIR "/profiles/SSE/profile.ini"),
                        noArchivesProfile));
    QSettings noArchives(noArchivesProfile, QSettings::IniFormat);
    noArchives.setValue(QStringLiteral("BSA/bsaEnabled"), false);
    noArchives.sync();
    QCOMPARE(noArchives.status(), QSettings::NoError);

    QSettings common(fixtureRoot.filePath(QStringLiteral("profiles/common.ini")),
                     QSettings::IniFormat);
    common.setValue(QStringLiteral("profile"), QStringLiteral("SSE"));
    common.sync();
    QCOMPARE(common.status(), QSettings::NoError);
    QVERIFY(QDir::setCurrent(_workingDirectory.path()));
}

void ApplicationRunSetupTests::cleanupTestCase()
{
    QVERIFY(QDir::setCurrent(_originalCurrentPath));
}

void ApplicationRunSetupTests::disabledTextureWorkDoesNotRequestProfileConversion()
{
    Profiles::setCurrentProfile(QStringLiteral("SSE"));
    OptionsCAO options;
    disableTextureWork(options);

    const auto result = cao::run::prepareApplicationRun(options);

    QVERIFY(result.hasPolicy());
    QVERIFY(!result.policy()->requests(RequestedWork::ConvertibleTextureConversion));
    QVERIFY(!result.policy()->maintainsMeshReferences());
}

void ApplicationRunSetupTests::fo4ConversionCompilesWithoutMeshOptimization()
{
    Profiles::setCurrentProfile(QStringLiteral("FO4"));
    OptionsCAO options;
    disableTextureWork(options);
    options.bTexturesNecessary = true;
    options.iMeshesOptimizationLevel = 0;

    const auto result = cao::run::prepareApplicationRun(options);

    QVERIFY(result.hasPolicy());
    QVERIFY(result.policy()->requests(RequestedWork::ConvertibleTextureConversion));
    QVERIFY(result.policy()->maintainsMeshReferences());
    QVERIFY(!result.policy()->requests(RequestedWork::StandardMeshOptimization));
    QVERIFY(!result.policy()->requests(RequestedWork::TerrainMeshOptimization));
}

void ApplicationRunSetupTests::meshWorkRoutesStandardAndTerrainMeshes_data()
{
    QTest::addColumn<int>("optimizationLevel");
    QTest::addColumn<bool>("resave");
    QTest::addColumn<bool>("shouldRoute");

    QTest::newRow("disabled") << 0 << false << false;
    QTest::newRow("resave only") << 0 << true << true;
    QTest::newRow("optimization only") << 1 << false << true;
    QTest::newRow("optimization and resave") << 1 << true << true;
}

void ApplicationRunSetupTests::meshWorkRoutesStandardAndTerrainMeshes()
{
    QFETCH(int, optimizationLevel);
    QFETCH(bool, resave);
    QFETCH(bool, shouldRoute);

    Profiles::setCurrentProfile(QStringLiteral("SSE"));
    OptionsCAO options;
    disableTextureWork(options);
    options.iMeshesOptimizationLevel = optimizationLevel;
    options.bMeshesResave = resave;

    const auto result = cao::run::prepareApplicationRun(options);

    QVERIFY(result.hasPolicy());
    const cao::routing::AssetRouter router(*result.policy());
    for (const auto *path : {"meshes/armor.nif", "meshes/terrain.btr"}) {
        const auto decision = router.route(std::filesystem::path(path));
        QCOMPARE(std::holds_alternative<cao::routing::RoutedAsset>(decision), shouldRoute);
    }
}

void ApplicationRunSetupTests::archiveCreationRequiresProfileArchiveSupport()
{
    Profiles::setCurrentProfile(QStringLiteral("NoArchives"));
    OptionsCAO options;
    disableTextureWork(options);
    options.iMeshesOptimizationLevel = 0;
    // The CLI accepts --bc for any profile, so setup is the only place this can still be caught
    // before Manager packs Archives and deletes the Loose sources.
    options.bBsaCreate = true;

    const auto result = cao::run::prepareApplicationRun(options);

    QVERIFY(!result.hasPolicy());
    QVERIFY(!cao::run::policyValidationErrorMessages(result.errors()).isEmpty());
}

void ApplicationRunSetupTests::requestOwnsCallerIntent()
{
    Profiles::setCurrentProfile(QStringLiteral("FO4"));
    OptionsCAO options;
    options.mode = OptionsCAO::SeveralMods;
    options.userPath = QStringLiteral("mods");
    options.bDryRun = true;
    options.bMeshesResave = true;
    const auto request = cao::run::makeApplicationRunRequest(options);
    options.userPath = QStringLiteral("changed");
    Profiles::setCurrentProfile(QStringLiteral("SSE"));
    QCOMPARE(request.profileIdentity(), std::string("FO4"));
    QCOMPARE(request.executionMode(), cao::routing::ExecutionMode::DryRun);
    QCOMPARE(request.modSelection().kind(), cao::run::ModSelectionKind::ChildModRoots);
    QCOMPARE(request.modSelection().directory(), std::filesystem::path("mods"));
    QVERIFY(request.requests(RequestedWork::ConvertibleTextureConversion));
    QVERIFY(request.requests(RequestedWork::StandardMeshOptimization));
    QVERIFY(request.requests(RequestedWork::TerrainMeshOptimization));
}

void ApplicationRunSetupTests::providerLoadsOwnedConfiguration()
{
    QFile ignored(QStringLiteral("profiles/SSE/ignoredMods.txt"));
    QVERIFY(ignored.open(QIODevice::WriteOnly));
    ignored.write("# comment\n  Tool Mod  \n\n");
    ignored.close();
    const auto provider = cao::run::makeApplicationRunConfigurationProvider();
    Profiles::setCurrentProfile(QStringLiteral("SSE"));
    auto loading = std::async(std::launch::async, [provider] { return provider->load("FO4"); });
    const auto configuration = loading.get();
    QCOMPARE(configuration.profile().archiveExtension, std::optional<std::string>(".ba2"));
    QCOMPARE(configuration.ignoredMods().size(), std::size_t(1));
    QCOMPARE(configuration.ignoredMods()[0], std::string("Tool Mod"));
    QCOMPARE(configuration.separatorMarkers()[0], std::string("separator"));
    QVERIFY_EXCEPTION_THROWN(static_cast<void>(provider->load("MissingProfile")), std::runtime_error);
}

void ApplicationRunSetupTests::requestRejectsInvalidOptionValues_data()
{
    QTest::addColumn<int>("invalidChoice");
    QTest::newRow("negative mesh level") << 0;
    QTest::newRow("excessive mesh level") << 1;
    QTest::newRow("zero width ratio") << 2;
    QTest::newRow("zero height ratio") << 3;
    QTest::newRow("zero target width") << 4;
    QTest::newRow("zero target height") << 5;
    QTest::newRow("odd target width") << 6;
    QTest::newRow("invalid mode") << 7;
}

void ApplicationRunSetupTests::requestRejectsInvalidOptionValues()
{
    QFETCH(int, invalidChoice);
    OptionsCAO options;
    options.mode = OptionsCAO::SingleMod;
    options.userPath = QStringLiteral("not-yet-resolved");
    switch (invalidChoice) {
        case 0: options.iMeshesOptimizationLevel = -1; break;
        case 1: options.iMeshesOptimizationLevel = 4; break;
        case 2: options.bTexturesResizeRatio = true; options.iTexturesTargetWidthRatio = 0; break;
        case 3: options.bTexturesResizeRatio = true; options.iTexturesTargetHeightRatio = 0; break;
        case 4: options.bTexturesResizeSize = true; options.iTexturesTargetWidth = 0; break;
        case 5: options.bTexturesResizeSize = true; options.iTexturesTargetHeight = 0; break;
        case 6: options.bTexturesResizeSize = true; options.iTexturesTargetWidth = 513; break;
        case 7: options.mode = static_cast<OptionsCAO::OptimizationMode>(2); break;
    }
    QVERIFY_EXCEPTION_THROWN(static_cast<void>(cao::run::makeApplicationRunRequest(options)),
                             std::invalid_argument);
}

void ApplicationRunSetupTests::optimizerProfileLoadsAuxiliaryFilesAfterCapture()
{
    Profiles::setCurrentProfile(QStringLiteral("FO4"));
    auto snapshot = OptimizerProfileSnapshot::captureIntent();
    QVERIFY(snapshot.customHeadparts.isEmpty());
    QVERIFY(snapshot.filesToNotPack.isEmpty());
    QFile headparts(QStringLiteral("profiles/FO4/customHeadparts.txt"));
    QVERIFY(headparts.open(QIODevice::WriteOnly));
    headparts.write("# comment\nNew Headpart\n");
    headparts.close();
    QFile exclusions(QStringLiteral("profiles/SSE/FilesToNotPack.txt"));
    QVERIFY(exclusions.open(QIODevice::WriteOnly));
    exclusions.write("New Exclusion\n");
    exclusions.close();
    Profiles::setCurrentProfile(QStringLiteral("SSE"));
    auto loading = std::async(std::launch::async, [&snapshot] { snapshot.loadAuxiliaryFiles(); });
    loading.get();
    QCOMPARE(snapshot.customHeadparts, QStringList{QStringLiteral("New Headpart")});
    QCOMPARE(snapshot.filesToNotPack, QStringList{QStringLiteral("New Exclusion")});
}

QTEST_APPLESS_MAIN(ApplicationRunSetupTests)
#include "ApplicationRunSetupTests.moc"
