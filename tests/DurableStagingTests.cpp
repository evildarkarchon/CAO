#include "Run/TemporaryArtifactRegistry.h"
#include "Run/StagingRecovery.h"
#include <QtTest>
#include <filesystem>
#include <fstream>
#include <cstdio>

namespace fs = std::filesystem;

class DurableStagingTests final : public QObject {
    Q_OBJECT
   private slots:
    /// Missing durable ownership would leave the interrupted output unrecoverable.
    void abandonedOutputIsRecovered();
    /// Recovery must not delete a destination whose rename preceded a killed process.
    void killedAfterRenameKeepsDestination();
    /// A stale record for a missing temporary file must not block a later producer.
    void recoveryAndProductionShareTheOwnershipLock();
    /// A cancelled preparation leaves its durable sibling registration for a later recovery.
    void cancelledPreparationPreservesDurableSibling();
    /// Interrupted snapshot scratch is disposable only under valid manifest ownership.
    void partialScratchIsRecoveredButCorruptOwnershipIsPreserved();
    /// Cleanup removes only registered temporary entries and releases no committed destination.
    void cleanupRemovesTheRunChildAndKeepsCommittedOutput();
    /// A reserved name without a valid ownership proof must never be adopted by a producer.
    void unownedBootstrapIsRejected();
    /// One damaged entry must not stop cleanup of independently registered temporary files.
    void cleanupContinuesAfterADamagedTemporary();
    /// Releasing a file before its destination move must preserve source and cleanup ownership.
    void prematureReleaseKeepsOwnership();
};

void DurableStagingTests::abandonedOutputIsRecovered() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    const auto destination = root / "texture.dds";
    std::ofstream(destination) << "original";
    fs::path temporary;
    try {
        cao::run::TemporaryArtifactRegistry registry;
        auto staged = registry.stageFile(root, destination);
        temporary = staged.path;
        QVERIFY(fs::is_regular_file(temporary));
        QVERIFY(temporary.parent_path() == destination.parent_path());
        std::ofstream(temporary) << "partial";
    } catch (const std::exception& error) {
        QFAIL(error.what());
    }
    cao::run::StagingRecovery recovery;
    const auto failure = recovery.recover(root);
    QVERIFY(!failure.has_value());
    QVERIFY(!fs::exists(temporary));
    QVERIFY(fs::exists(destination));
}

void DurableStagingTests::killedAfterRenameKeepsDestination() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    std::ofstream(root / "texture.tga") << "source";
    QProcess child;
    child.start(QCoreApplication::applicationFilePath(), {"--staging-crash", directory.path()});
    QVERIFY(child.waitForStarted());
    QVERIFY(child.waitForReadyRead(10000));
    QCOMPARE(child.readAllStandardOutput().trimmed(), QByteArray("renamed"));
    cao::run::StagingRecovery blocked;
    const auto active = blocked.recover(root);
    QVERIFY(active.has_value());
    QCOMPARE(active->code(), cao::run::RunFailureCode::StagingActive);
    child.kill();
    QVERIFY(child.waitForFinished());
    cao::run::StagingRecovery recovered;
    QVERIFY(!recovered.recover(root).has_value());
    QFile output(QString::fromStdWString((root / "texture.dds").wstring()));
    QVERIFY(output.open(QIODevice::ReadOnly));
    QCOMPARE(output.readAll(), QByteArray("converted"));
    QVERIFY(fs::exists(root / "texture.tga"));
}

void DurableStagingTests::recoveryAndProductionShareTheOwnershipLock() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    {
        cao::run::TemporaryArtifactRegistry first;
        const auto staged = first.stageFile(root, root / "texture.dds");
        // This disk state also represents registration flushed before exclusive file creation.
        QVERIFY(fs::remove(staged.path));
    }
    cao::run::TemporaryArtifactRegistry second;
    QVERIFY(!second.prepareRoot(root).has_value());
    auto next = second.stageFile(root, root / "texture.dds");
    QVERIFY(fs::exists(next.path));
    QVERIFY(second.performSafetyCleanup().empty());
    QVERIFY(!fs::exists(next.path));
    QVERIFY(fs::exists(root));
}

void DurableStagingTests::cancelledPreparationPreservesDurableSibling() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    fs::path temporary;
    {
        cao::run::TemporaryArtifactRegistry producer;
        temporary = producer.stageFile(root, root / "texture.dds").path;
        std::ofstream(temporary) << "partial";
    }
    std::stop_source cancellation;
    cancellation.request_stop();
    cao::run::TemporaryArtifactRegistry cancelled;

    QVERIFY(!cancelled.prepareRoot(root, cancellation.get_token()).has_value());
    QVERIFY(fs::exists(temporary));
}

void DurableStagingTests::partialScratchIsRecoveredButCorruptOwnershipIsPreserved() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    fs::path temporary;
    {
        cao::run::TemporaryArtifactRegistry registry;
        temporary = registry.stageFile(root, root / "texture.dds").path;
    }
    const auto staging = root / ".cao-staging";
    std::ofstream(staging / "ownership.manifest.next") << "partial replacement";
    {
        cao::run::StagingRecovery recovery;
        QVERIFY(!recovery.recover(root).has_value());
    }
    QVERIFY(!fs::exists(temporary));
    QVERIFY(!fs::exists(staging / "ownership.manifest.next"));
    {
        cao::run::TemporaryArtifactRegistry registry;
        temporary = registry.stageFile(root, root / "texture.dds").path;
    }
    std::ofstream(staging / "ownership.manifest") << "corrupt";
    std::ofstream(staging / "ownership.manifest.next") << "must stay";
    cao::run::StagingRecovery recovery;
    QVERIFY(recovery.recover(root).has_value());
    QVERIFY(fs::exists(temporary));
    QVERIFY(fs::exists(staging / "ownership.manifest.next"));
}

void DurableStagingTests::cleanupRemovesTheRunChildAndKeepsCommittedOutput() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    const auto destination = root / "texture.dds";
    cao::run::TemporaryArtifactRegistry registry;
    const auto staged = registry.stageFile(root, destination);
    std::ofstream(staged.path) << "committed";
    fs::rename(staged.path, destination);
    registry.commit(staged.registration);
    const auto uncommitted = registry.stageFile(root, root / "other.dds");
    QVERIFY(registry.performSafetyCleanup().empty());
    QVERIFY(!fs::exists(staged.path));
    QVERIFY(!fs::exists(uncommitted.path));
    QVERIFY(fs::exists(root));
    QVERIFY(fs::exists(destination));
    QVERIFY(registry.performSafetyCleanup().empty());
}

void DurableStagingTests::unownedBootstrapIsRejected() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    fs::create_directory(root / ".cao-staging");
    cao::run::TemporaryArtifactRegistry registry;
    QVERIFY_EXCEPTION_THROWN((void)registry.stageFile(root, root / "texture.dds"),
                             std::runtime_error);
    QVERIFY(fs::is_empty(root / ".cao-staging"));
}

void DurableStagingTests::cleanupContinuesAfterADamagedTemporary() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    cao::run::TemporaryArtifactRegistry registry;
    const auto intact = registry.stageFile(root, root / "first.dds");
    const auto damaged = registry.stageFile(root, root / "second.dds");
    QVERIFY(fs::remove(damaged.path));
    QVERIFY(fs::create_directory(damaged.path));
    std::ofstream(damaged.path / "unregistered") << "keep";
    const auto failures = registry.performSafetyCleanup();
    QCOMPARE(failures.size(), std::size_t{1});
    QVERIFY(!fs::exists(intact.path));
    QVERIFY(fs::exists(damaged.path / "unregistered"));
}

void DurableStagingTests::prematureReleaseKeepsOwnership() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    const auto destination = root / "texture.dds";
    std::ofstream(destination) << "original";
    cao::run::TemporaryArtifactRegistry registry;
    const auto staged = registry.stageFile(root, destination);
    QVERIFY_EXCEPTION_THROWN(registry.commit(staged.registration), std::logic_error);
    QVERIFY(fs::exists(staged.path));
    QVERIFY(registry.performSafetyCleanup().empty());
    QVERIFY(!fs::exists(staged.path));
    QFile original(QString::fromStdWString(destination.wstring()));
    QVERIFY(original.open(QIODevice::ReadOnly));
    QCOMPARE(original.readAll(), QByteArray("original"));
}

/// Runs a real interrupted producer without destructor cleanup, or the normal Qt test suite.
int main(int argc, char** argv) {
    QCoreApplication application(argc, argv);
    if (application.arguments().size() == 3 && application.arguments().at(1) == "--staging-crash") {
        const auto root = fs::canonical(fs::path(application.arguments().at(2).toStdWString()));
        cao::run::TemporaryArtifactRegistry registry;
        const auto staged = registry.stageFile(root, root / "texture.dds");
        std::ofstream(staged.path) << "converted";
        fs::rename(staged.path, root / "texture.dds");
        std::fputs("renamed\n", stdout);
        std::fflush(stdout);
        return application.exec();
    }
    DurableStagingTests tests;
    return QTest::qExec(&tests, argc, argv);
}
#include "DurableStagingTests.moc"
