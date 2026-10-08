#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QProcess>
#include <QTemporaryDir>
#include <QTest>

class CliExecutionTests final : public QObject {
    Q_OBJECT

   private slots:
    /// Malformed options are start failures, while help remains a successful non-run command.
    void reportsArgumentExitCodes() {
        for (const auto& argument : {"--unknown", "--trrw", "--help"}) {
            QProcess process;
            process.start(QStringLiteral(CAO_CLI_PATH), {argument});
            QVERIFY(process.waitForStarted());
            QVERIFY(process.waitForFinished(30000));
            QCOMPARE(process.exitStatus(), QProcess::NormalExit);
            QCOMPARE(process.exitCode(), QByteArray(argument) == "--help" ? 0 : 2);
        }
    }
    /// Stops before any Asset mutation when the selected Mod Root has unowned staging.
    void refusesUnknownStagingBeforeAssetTraversal();

    /// Supplies backend-operation failures, quarantined load failures, and a successful run.
    void reportsAssetExecutionStatus_data();

    /// Executes the actual CLI and verifies exit status after every selected Asset is attempted.
    void reportsAssetExecutionStatus();
    /// Reports pruned empty directories as retained Archive Finalization mutations.
    void reportsDirectoryPruningMutation();

    /// Supplies one row per value of each parity-oracle archive flag and the fact it decides.
    void archiveOptionFlagsChangeOutput_data();
    /// Runs the real CLI with archive creation and checks each archive flag's on-disk effect.
    void archiveOptionFlagsChangeOutput();
};

namespace {
/// Writes non-empty bytes to a file under root; SSE BSA packing never parses the content.
bool writeFixture(const QDir& root, const QString& path) {
    if (!root.mkpath(QFileInfo(root.filePath(path)).path())) return false;
    const QByteArray content("fixture bytes");
    QFile file(root.filePath(path));
    return file.open(QIODevice::WriteOnly) && file.write(content) == content.size();
}

/// Reads the archive-flags byte of a TES4/SSE BSA header; -1 when the header cannot be read.
int bsaArchiveFlags(const QString& path) {
    QFile file(path);
    if (!file.open(QIODevice::ReadOnly)) return -1;
    const auto header = file.read(16);
    // Bytes 0-3 are the "BSA\0" magic and bytes 12-15 the little-endian archive flags.
    if (header.size() != 16 || !header.startsWith(QByteArrayLiteral("BSA\0"))) return -1;
    return static_cast<unsigned char>(header.at(12));
}
}  // namespace

void CliExecutionTests::refusesUnknownStagingBeforeAssetTraversal() {
    const QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const QDir root(directory.path());
    QVERIFY(root.mkpath("profiles/SSE"));
    QVERIFY(root.mkpath("mod/.cao-staging"));
    QVERIFY(root.mkpath("mod/textures/armor"));
    QVERIFY(QFile::copy(QStringLiteral(CAO_SOURCE_DIR "/profiles/SSE/profile.ini"),
                        root.filePath("profiles/SSE/profile.ini")));
    for (const auto& path : {"mod/.cao-staging/unknown", "mod/textures/armor/input.dds"}) {
        QFile file(root.filePath(path));
        QVERIFY(file.open(QIODevice::WriteOnly));
        QCOMPARE(file.write("preserve"), qint64{8});
    }

    QProcess process;
    process.setWorkingDirectory(directory.path());
    process.start(QStringLiteral(CAO_CLI_PATH), {root.filePath("mod"), "om", "SSE", "--t0"});
    QVERIFY2(process.waitForStarted(), qPrintable(process.errorString()));
    QVERIFY2(process.waitForFinished(30000), qPrintable(process.errorString()));
    QCOMPARE(process.exitStatus(), QProcess::NormalExit);
    QCOMPARE(process.exitCode(), 2);
    QVERIFY(QFile::exists(root.filePath("mod/.cao-staging/unknown")));
    QVERIFY(QFile::exists(root.filePath("mod/textures/armor/input.dds")));
    QVERIFY(!QFile::exists(root.filePath("mod/textures/armor/input.dds.caobad")));
}

void CliExecutionTests::reportsAssetExecutionStatus_data() {
    QTest::addColumn<QString>("extension");
    QTest::addColumn<QStringList>("options");
    QTest::addColumn<int>("expectedExitCode");
    QTest::addColumn<bool>("quarantined");

    QTest::newRow("missing-animation-tool") << ".hkx" << QStringList{"-a"} << 1 << false;
    QTest::newRow("quarantined-texture-loads") << ".dds" << QStringList{"--t0"} << 1 << true;
    QTest::newRow("disabled-assets-succeed") << ".hkx" << QStringList{} << 0 << false;
}

void CliExecutionTests::reportsAssetExecutionStatus() {
    QFETCH(QString, extension);
    QFETCH(QStringList, options);
    QFETCH(int, expectedExitCode);
    QFETCH(bool, quarantined);

    const QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const QDir root(directory.path());
    QVERIFY(root.mkpath("profiles/SSE"));
    QVERIFY(root.mkpath("mod"));
    QVERIFY(QFile::copy(QStringLiteral(CAO_SOURCE_DIR "/profiles/SSE/profile.ini"),
                       root.filePath("profiles/SSE/profile.ini")));

    // Keep hkxcmd absent in the isolated working directory even on developer machines that
    // have it installed, so the real animation backend fails without running an external tool.
    QVERIFY(!QFile::exists(root.filePath("bin/hkxcmd.exe")));
    const QStringList inputNames{"first" + extension, "second" + extension};
    for (const auto& name : inputNames) {
        QFile asset(root.filePath("mod/" + name));
        QVERIFY(asset.open(QIODevice::WriteOnly));
        QCOMPARE(asset.write("malformed"), qint64{9});
    }

    QProcess process;
    process.setWorkingDirectory(directory.path());
    QStringList arguments{root.filePath("mod"), "om", "SSE"};
    arguments.append(options);
    process.start(QStringLiteral(CAO_CLI_PATH), arguments);
    QVERIFY2(process.waitForStarted(), qPrintable(process.errorString()));
    QVERIFY2(process.waitForFinished(30000), qPrintable(process.errorString()));
    QCOMPARE(process.exitStatus(), QProcess::NormalExit);
    const auto standardOutput = process.readAllStandardOutput();
    const auto cleanup = standardOutput.indexOf("Safety Cleanup");
    const auto outcome = standardOutput.indexOf("Outcome|");
    QVERIFY(cleanup >= 0);
    QVERIFY(outcome > cleanup);
    QVERIFY(standardOutput.contains(expectedExitCode == 0 ? "Outcome|Succeeded"
                                                        : "Outcome|Completed With Failures"));

    // A failed Asset must not prevent later Assets from receiving their existing processing
    // and quarantine behavior; the terminal status must retain those failures afterward.
    if (expectedExitCode != 0) QVERIFY(standardOutput.contains("|2|2"));
    for (const auto& name : inputNames) {
        QCOMPARE(QFile::exists(root.filePath("mod/" + name)), !quarantined);
        QCOMPARE(QFile::exists(root.filePath("mod/" + name + ".caobad")), quarantined);
    }
    QCOMPARE(process.exitCode(), expectedExitCode);
}

void CliExecutionTests::reportsDirectoryPruningMutation() {
    const QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const QDir root(directory.path());
    QVERIFY(root.mkpath("profiles/SSE"));
    QVERIFY(root.mkpath("mod/empty"));
    QVERIFY(QFile::copy(QStringLiteral(CAO_SOURCE_DIR "/profiles/SSE/profile.ini"),
                       root.filePath("profiles/SSE/profile.ini")));

    QProcess process;
    process.setWorkingDirectory(directory.path());
    process.start(QStringLiteral(CAO_CLI_PATH), {root.filePath("mod"), "om", "SSE", "--t0"});
    QVERIFY2(process.waitForStarted(), qPrintable(process.errorString()));
    QVERIFY2(process.waitForFinished(30000), qPrintable(process.errorString()));
    QCOMPARE(process.exitStatus(), QProcess::NormalExit);
    QCOMPARE(process.exitCode(), 0);
    QVERIFY(!QFile::exists(root.filePath("mod/empty")));
    const auto standardOutput = process.readAllStandardOutput();
    QVERIFY(standardOutput.contains("Archive Finalization|1|partial-or-unknown=0"));
}

void CliExecutionTests::archiveOptionFlagsChangeOutput_data() {
    QTest::addColumn<QStringList>("flags");
    QTest::addColumn<QString>("fact");
    QTest::addColumn<bool>("expected");

    // An archive holding incompressible files is never compressed, and merging them is the
    // default, so the compression rows keep the sound in its own archive to leave mod.bsa
    // compressible.
    QTest::newRow("compress-on") << QStringList{"--bmi", "0", "--bcomp", "1"} << "compressed"
                                 << true;
    QTest::newRow("compress-off") << QStringList{"--bmi", "0", "--bcomp", "0"} << "compressed"
                                  << false;
    QTest::newRow("dummies-on") << QStringList{"--bdum", "1"} << "dummy plugin" << true;
    QTest::newRow("dummies-off") << QStringList{"--bdum", "0"} << "dummy plugin" << false;
    QTest::newRow("merge-incompressible-on")
        << QStringList{"--bmi", "1"} << "incompressible archive" << false;
    QTest::newRow("merge-incompressible-off")
        << QStringList{"--bmi", "0"} << "incompressible archive" << true;
    QTest::newRow("merge-textures-on") << QStringList{"--bmt", "1"} << "textures archive" << false;
    QTest::newRow("merge-textures-off") << QStringList{"--bmt", "0"} << "textures archive" << true;
    QTest::newRow("delete-sources-on") << QStringList{"--bds", "1"} << "loose source" << false;
    QTest::newRow("delete-sources-off") << QStringList{"--bds", "0"} << "loose source" << true;
}

void CliExecutionTests::archiveOptionFlagsChangeOutput() {
    QFETCH(QStringList, flags);
    QFETCH(QString, fact);
    QFETCH(bool, expected);

    const QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const QDir root(directory.path());
    QVERIFY(root.mkpath("profiles/SSE"));
    QVERIFY(QFile::copy(QStringLiteral(CAO_SOURCE_DIR "/profiles/SSE/profile.ini"),
                        root.filePath("profiles/SSE/profile.ini")));
    // One Standard, one Texture and one Incompressible file, so every split and merge is visible.
    for (const auto& path : {"mod/meshes/a.nif", "mod/textures/a.dds", "mod/sound/a.wav"})
        QVERIFY2(writeFixture(root, path), path);

    QProcess process;
    process.setWorkingDirectory(directory.path());
    process.start(QStringLiteral(CAO_CLI_PATH),
                  QStringList{root.filePath("mod"), "om", "SSE", "--bc"} + flags);
    QVERIFY2(process.waitForStarted(), qPrintable(process.errorString()));
    QVERIFY2(process.waitForFinished(30000), qPrintable(process.errorString()));
    QCOMPARE(process.exitStatus(), QProcess::NormalExit);
    const auto standardOutput = process.readAllStandardOutput();
    QVERIFY2(process.exitCode() == 0, standardOutput.constData());
    QVERIFY(QFile::exists(root.filePath("mod/mod.bsa")));

    // Archive names follow the Mod Root: the unmerged Incompressible archive takes the next
    // free counter name after the Standard archive's mod.bsa.
    bool actual = false;
    if (fact == "compressed") {
        const auto archiveFlags = bsaArchiveFlags(root.filePath("mod/mod.bsa"));
        QVERIFY(archiveFlags >= 0);
        actual = (archiveFlags & 0x04) != 0;
    } else if (fact == "dummy plugin") {
        actual = QFile::exists(root.filePath("mod/mod.esp"));
    } else if (fact == "incompressible archive") {
        actual = QFile::exists(root.filePath("mod/mod0.bsa"));
    } else if (fact == "textures archive") {
        actual = QFile::exists(root.filePath("mod/mod - Textures.bsa"));
    } else if (fact == "loose source") {
        actual = QFile::exists(root.filePath("mod/meshes/a.nif"));
    } else {
        QFAIL(qPrintable("Unknown fact: " + fact));
    }
    QCOMPARE(actual, expected);
}

QTEST_GUILESS_MAIN(CliExecutionTests)

#include "CliExecutionTests.moc"
