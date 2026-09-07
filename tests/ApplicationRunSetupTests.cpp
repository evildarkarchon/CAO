#include "FilesystemOperations.h"
#include "Profiles.h"
#include "Run/ApplicationRunSetup.h"

#include <QTemporaryDir>
#include <QTest>

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

QTEST_APPLESS_MAIN(ApplicationRunSetupTests)
#include "ApplicationRunSetupTests.moc"
