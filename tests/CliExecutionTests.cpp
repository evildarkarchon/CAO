#include <QDir>
#include <QFile>
#include <QProcess>
#include <QTemporaryDir>
#include <QTest>

class CliExecutionTests final : public QObject {
    Q_OBJECT

   private slots:
    /// Supplies backend-operation failures, quarantined load failures, and a successful run.
    void reportsAssetExecutionStatus_data();

    /// Executes the actual CLI and verifies exit status after every selected Asset is attempted.
    void reportsAssetExecutionStatus();
};

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

    // A failed Asset must not prevent later Assets from receiving their existing processing
    // and quarantine behavior; the terminal status must retain those failures afterward.
    if (expectedExitCode != 0) QVERIFY(standardOutput.contains("|2|2"));
    for (const auto& name : inputNames) {
        QCOMPARE(QFile::exists(root.filePath("mod/" + name)), !quarantined);
        QCOMPARE(QFile::exists(root.filePath("mod/" + name + ".caobad")), quarantined);
    }
    QCOMPARE(process.exitCode(), expectedExitCode);
}

QTEST_GUILESS_MAIN(CliExecutionTests)

#include "CliExecutionTests.moc"
