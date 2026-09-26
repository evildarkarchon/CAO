#include "FilesystemOperations.h"
#include "OptionsCAO.h"
#include "Profiles.h"

#include <QTemporaryDir>
#include <QTest>

// Profiles::create is not exercised here; this satisfies its otherwise unrelated link dependency.
void FilesystemOperations::copyDir(const QString &, const QString &, bool)
{
    // The profile fixtures are copied explicitly before Profiles is initialized.
}

namespace
{
/// Builds one CLI argument list with the executable name Qt expects in position zero.
QStringList commandLine(const QStringList &arguments)
{
    return QStringList{QStringLiteral("CathedralAssetsOptimizer")} + arguments;
}
}

class OptionsCAOTests final : public QObject
{
    Q_OBJECT

private slots:
    /// Copies a shipped profile into an isolated working directory before the singleton initializes.
    void initTestCase();
    /// Restores the process working directory after the isolated CLI checks complete.
    void cleanupTestCase();

    /// Verifies flag-only invocations are rejected instead of indexing absent positional arguments.
    void rejectsInvocationWithoutThreePositionalArguments();
    /// Verifies a misspelled profile is reported instead of silently falling back to the default.
    void rejectsUnknownProfile();
    /// Verifies the resize options parse their values rather than acting as boolean switches.
    void resizeOptionsAcceptValues();
    /// Verifies an omitted ratio defaults to one so --trr alone cannot divide by zero.
    void omittedResizeRatiosDefaultToOne();
    /// Verifies zero ratios and zero target sizes are rejected before a run can start.
    void rejectsZeroResizeDimensions();
    /// Verifies a nonnumeric mesh level is reported instead of silently disabling optimization.
    void rejectsNonNumericMeshOptimizationLevel();

private:
    QString _originalCurrentPath;
    QTemporaryDir _workingDirectory;
};

void OptionsCAOTests::initTestCase()
{
    QVERIFY(_workingDirectory.isValid());
    _originalCurrentPath = QDir::currentPath();

    QDir fixtureRoot(_workingDirectory.path());
    const auto profileDirectory = QStringLiteral("profiles/SSE");
    QVERIFY(fixtureRoot.mkpath(profileDirectory));
    const auto source = QStringLiteral(CAO_SOURCE_DIR "/profiles/SSE/profile.ini");
    const auto destination = fixtureRoot.filePath(profileDirectory
                                                   + QStringLiteral("/profile.ini"));
    QVERIFY2(QFile::copy(source, destination), qPrintable(source));

    QSettings common(fixtureRoot.filePath(QStringLiteral("profiles/common.ini")),
                     QSettings::IniFormat);
    common.setValue(QStringLiteral("profile"), QStringLiteral("SSE"));
    common.sync();
    QCOMPARE(common.status(), QSettings::NoError);
    QVERIFY(QDir::setCurrent(_workingDirectory.path()));
}

void OptionsCAOTests::cleanupTestCase()
{
    QVERIFY(QDir::setCurrent(_originalCurrentPath));
}

void OptionsCAOTests::rejectsInvocationWithoutThreePositionalArguments()
{
    OptionsCAO options;

    // Three arguments satisfy a naive count check while carrying no positional value at all.
    QVERIFY_EXCEPTION_THROWN(
        options.parseArguments(commandLine({QStringLiteral("--dr"),
                                            QStringLiteral("--l"),
                                            QStringLiteral("--a")})),
        std::runtime_error);

    QVERIFY_EXCEPTION_THROWN(
        options.parseArguments(commandLine({_workingDirectory.path(), QStringLiteral("om")})),
        std::runtime_error);
}

void OptionsCAOTests::rejectsUnknownProfile()
{
    OptionsCAO options;

    QVERIFY_EXCEPTION_THROWN(
        options.parseArguments(commandLine({_workingDirectory.path(),
                                            QStringLiteral("om"),
                                            QStringLiteral("SSEE")})),
        std::runtime_error);
}

void OptionsCAOTests::resizeOptionsAcceptValues()
{
    OptionsCAO ratioOptions;
    ratioOptions.parseArguments(commandLine({_workingDirectory.path(),
                                             QStringLiteral("om"),
                                             QStringLiteral("SSE"),
                                             QStringLiteral("--trr"),
                                             QStringLiteral("--trrw"),
                                             QStringLiteral("2"),
                                             QStringLiteral("--trrh"),
                                             QStringLiteral("4")}));

    QVERIFY(ratioOptions.bTexturesResizeRatio);
    QCOMPARE(ratioOptions.iTexturesTargetWidthRatio, uint(2));
    QCOMPARE(ratioOptions.iTexturesTargetHeightRatio, uint(4));
    QCOMPARE(ratioOptions.isValid(), QString());

    OptionsCAO sizeOptions;
    sizeOptions.parseArguments(commandLine({_workingDirectory.path(),
                                            QStringLiteral("om"),
                                            QStringLiteral("SSE"),
                                            QStringLiteral("--trs"),
                                            QStringLiteral("--trsw"),
                                            QStringLiteral("512"),
                                            QStringLiteral("--trsh"),
                                            QStringLiteral("256")}));

    QVERIFY(sizeOptions.bTexturesResizeSize);
    QCOMPARE(sizeOptions.iTexturesTargetWidth, size_t(512));
    QCOMPARE(sizeOptions.iTexturesTargetHeight, size_t(256));
    QCOMPARE(sizeOptions.isValid(), QString());
}

void OptionsCAOTests::omittedResizeRatiosDefaultToOne()
{
    OptionsCAO options;
    options.parseArguments(commandLine({_workingDirectory.path(),
                                        QStringLiteral("om"),
                                        QStringLiteral("SSE"),
                                        QStringLiteral("--trr")}));

    QVERIFY(options.bTexturesResizeRatio);
    QCOMPARE(options.iTexturesTargetWidthRatio, uint(1));
    QCOMPARE(options.iTexturesTargetHeightRatio, uint(1));
    QCOMPARE(options.isValid(), QString());
}

void OptionsCAOTests::rejectsZeroResizeDimensions()
{
    OptionsCAO zeroRatio;
    zeroRatio.parseArguments(commandLine({_workingDirectory.path(),
                                          QStringLiteral("om"),
                                          QStringLiteral("SSE"),
                                          QStringLiteral("--trr"),
                                          QStringLiteral("--trrw"),
                                          QStringLiteral("0")}));

    QVERIFY(!zeroRatio.isValid().isEmpty());

    // --trs without dimensions leaves both at the zero default, which would request an empty image.
    OptionsCAO zeroSize;
    zeroSize.parseArguments(commandLine({_workingDirectory.path(),
                                         QStringLiteral("om"),
                                         QStringLiteral("SSE"),
                                         QStringLiteral("--trs")}));

    QVERIFY(!zeroSize.isValid().isEmpty());
}

void OptionsCAOTests::rejectsNonNumericMeshOptimizationLevel()
{
    OptionsCAO options;

    // "full" converts to zero, which isValid() accepts, so the run would silently proceed with
    // mesh optimization disabled while any selected destructive work still ran.
    QVERIFY_EXCEPTION_THROWN(
        options.parseArguments(commandLine({_workingDirectory.path(),
                                            QStringLiteral("om"),
                                            QStringLiteral("SSE"),
                                            QStringLiteral("-m"),
                                            QStringLiteral("full")})),
        std::runtime_error);

    OptionsCAO validOptions;
    validOptions.parseArguments(commandLine({_workingDirectory.path(),
                                             QStringLiteral("om"),
                                             QStringLiteral("SSE"),
                                             QStringLiteral("-m"),
                                             QStringLiteral("3")}));

    QCOMPARE(validOptions.iMeshesOptimizationLevel, 3);
    QCOMPARE(validOptions.isValid(), QString());
}

QTEST_MAIN(OptionsCAOTests)
#include "OptionsCAOTests.moc"
