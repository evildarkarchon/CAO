/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "ApplicationLogging.h"

#include <plog/Log.h>

#include <QCoreApplication>
#include <QFile>
#include <QProcess>
#include <QTemporaryDir>
#include <QtTest>

namespace
{
constexpr auto InfoLogMarker = "application-bootstrap-info-marker";
constexpr auto VerboseLogMarker = "application-bootstrap-verbose-marker";
constexpr auto RedirectedLogMarker = "application-run-redirected-marker";

/// Configures logging twice in an isolated process and emits one observable record.
int runLoggingChild(const QString &firstLogPath,
                    const QString &secondLogPath,
                    const bool debugLog)
{
    if (plog::get() != nullptr)
        return 1;

    cao::application::configureLogging(firstLogPath, debugLog);
    auto *const configuredLogger = plog::get();
    const auto expectedSeverity = debugLog ? plog::Severity::verbose : plog::Severity::info;
    if (configuredLogger == nullptr || configuredLogger->getMaxSeverity() != expectedSeverity)
        return 2;
    if (cao::application::configuredLogPath() != firstLogPath)
        return 3;

    PLOG_INFO << InfoLogMarker;
    PLOG_VERBOSE << VerboseLogMarker;

    cao::application::configureLogging(secondLogPath, !debugLog);
    if (plog::get() != configuredLogger
        || configuredLogger->getMaxSeverity() != expectedSeverity) {
        return 4;
    }
    if (cao::application::configuredLogPath() != firstLogPath)
        return 5;

    return 0;
}

/// Bootstraps at info, then redirects the run to a second path at verbose severity.
int runRedirectChild(const QString &bootstrapLogPath, const QString &runLogPath)
{
    if (plog::get() != nullptr)
        return 1;

    cao::application::configureLogging(bootstrapLogPath, false);
    PLOG_INFO << InfoLogMarker;

    // Stands in for the GUI switching profiles and enabling the debug log before the first run.
    cao::application::applyRunLogging(runLogPath, true);
    if (cao::application::configuredLogPath() != runLogPath)
        return 2;
    if (plog::get()->getMaxSeverity() != plog::Severity::verbose)
        return 3;

    PLOG_VERBOSE << RedirectedLogMarker;
    return 0;
}

/// Verifies one bootstrap severity in a fresh child process.
void verifyBootstrapConfiguration(const bool debugLog)
{
    QTemporaryDir logDirectory;
    QVERIFY(logDirectory.isValid());

    const auto firstLogPath = logDirectory.filePath(QStringLiteral("first.html"));
    const auto secondLogPath = logDirectory.filePath(QStringLiteral("second.html"));

    QProcess child;
    child.start(QCoreApplication::applicationFilePath(),
                {QStringLiteral("--logging-child"),
                 debugLog ? QStringLiteral("debug") : QStringLiteral("info"),
                 firstLogPath,
                 secondLogPath});
    QVERIFY2(child.waitForFinished(), qPrintable(child.errorString()));
    QCOMPARE(child.exitStatus(), QProcess::NormalExit);
    QCOMPARE(child.exitCode(), 0);

    QFile firstLog(firstLogPath);
    QVERIFY(firstLog.open(QFile::ReadOnly | QFile::Text));
    const auto contents = firstLog.readAll();
    QVERIFY(contents.contains(InfoLogMarker));
    QCOMPARE(contents.contains(VerboseLogMarker), debugLog);
    QVERIFY(!QFile::exists(secondLogPath));
}
}

class ApplicationLoggingTests final : public QObject
{
    Q_OBJECT

private slots:
    /// Proves info bootstrap filters verbose output and rejects later reconfiguration.
    void infoBootstrapConfigurationIsSingleAssignment();
    /// Proves debug bootstrap writes verbose output and rejects later reconfiguration.
    void debugBootstrapConfigurationIsSingleAssignment();
    /// Proves a run redirects the logger to its own profile path and severity.
    void runConfigurationRedirectsPathAndSeverity();
};

void ApplicationLoggingTests::infoBootstrapConfigurationIsSingleAssignment()
{
    verifyBootstrapConfiguration(false);
}

void ApplicationLoggingTests::debugBootstrapConfigurationIsSingleAssignment()
{
    verifyBootstrapConfiguration(true);
}

void ApplicationLoggingTests::runConfigurationRedirectsPathAndSeverity()
{
    QTemporaryDir logDirectory;
    QVERIFY(logDirectory.isValid());

    const auto bootstrapLogPath = logDirectory.filePath(QStringLiteral("bootstrap.html"));
    const auto runLogPath = logDirectory.filePath(QStringLiteral("run.html"));

    QProcess child;
    child.start(QCoreApplication::applicationFilePath(),
                {QStringLiteral("--redirect-child"), bootstrapLogPath, runLogPath});
    QVERIFY2(child.waitForFinished(), qPrintable(child.errorString()));
    QCOMPARE(child.exitStatus(), QProcess::NormalExit);
    QCOMPARE(child.exitCode(), 0);

    QFile bootstrapLog(bootstrapLogPath);
    QVERIFY(bootstrapLog.open(QFile::ReadOnly | QFile::Text));
    const auto bootstrapContents = bootstrapLog.readAll();
    QVERIFY(bootstrapContents.contains(InfoLogMarker));
    QVERIFY(!bootstrapContents.contains(RedirectedLogMarker));

    QFile runLog(runLogPath);
    QVERIFY(runLog.open(QFile::ReadOnly | QFile::Text));
    const auto runContents = runLog.readAll();
    QVERIFY(runContents.contains(RedirectedLogMarker));
    QVERIFY(!runContents.contains(InfoLogMarker));
}

int main(int argc, char *argv[])
{
    QCoreApplication application(argc, argv);
    const auto arguments = application.arguments();
    if (arguments.size() == 5 && arguments.at(1) == QStringLiteral("--logging-child")) {
        return runLoggingChild(arguments.at(3),
                               arguments.at(4),
                               arguments.at(2) == QStringLiteral("debug"));
    }
    if (arguments.size() == 4 && arguments.at(1) == QStringLiteral("--redirect-child"))
        return runRedirectChild(arguments.at(2), arguments.at(3));

    ApplicationLoggingTests tests;
    return QTest::qExec(&tests, argc, argv);
}

#include "ApplicationLoggingTests.moc"
