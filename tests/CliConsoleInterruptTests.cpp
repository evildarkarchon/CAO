#include "CliRun.h"
#include <QProcess>
#include <QtTest>
#include <chrono>
#include <thread>
#ifdef _WIN32
#define NOMINMAX
#include <windows.h>
#endif

class CliConsoleInterruptTests final : public QObject {
    Q_OBJECT
   private slots:
    /// Exercises repeated Windows Ctrl+C delivery in an isolated hidden console.
    void consoleInterruptsRemainCooperative() {
#ifdef _WIN32
        QProcess process;
        process.setCreateProcessArgumentsModifier([](QProcess::CreateProcessArguments* arguments) {
            arguments->flags |= CREATE_NEW_CONSOLE;
            arguments->startupInfo->dwFlags |= STARTF_USESHOWWINDOW;
            arguments->startupInfo->wShowWindow = SW_HIDE;
        });
        process.start(QCoreApplication::applicationFilePath(), {"--interrupt-probe"});
        QVERIFY(process.waitForStarted());
        QVERIFY(process.waitForFinished(10000));
        QCOMPARE(process.exitStatus(), QProcess::NormalExit);
        QCOMPARE(process.exitCode(), 0);
#else
        QSKIP("The native console probe requires Windows");
#endif
    }
};

/// Runs the native probe in a child console so Ctrl+C cannot escape the test runner.
int main(int argc, char** argv) {
    QCoreApplication application(argc, argv);
#ifdef _WIN32
    if (application.arguments().contains("--interrupt-probe")) {
        cao::cli::ConsoleInterrupt interruption;
        if (!GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0)) return 10;
        const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
        while (!interruption.requested() && std::chrono::steady_clock::now() < deadline)
            std::this_thread::sleep_for(std::chrono::milliseconds(10));
        if (!interruption.requested()) return 11;
        if (!GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0)) return 12;
        std::this_thread::sleep_for(std::chrono::milliseconds(100));
        return interruption.requested() ? 0 : 13;
    }
#endif
    CliConsoleInterruptTests tests;
    return QTest::qExec(&tests, argc, argv);
}
#include "CliConsoleInterruptTests.moc"
