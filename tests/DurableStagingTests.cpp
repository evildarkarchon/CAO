#include "Run/TemporaryArtifactRegistry.h"
#include "Run/StagingRecovery.h"
#include <QtTest>
#include <filesystem>
#include <fstream>
#include <cstdio>
#include <optional>
#include <type_traits>
#include <utility>

namespace fs = std::filesystem;

class DurableStagingTests final : public QObject {
    Q_OBJECT
   private slots:
    /// Arbitrary extracted Archive bytes remain recoverable after the producer exits.
    void abandonedArchiveEntryIsRecovered();
    /// Safety Cleanup removes partial Archive entries while preserving moved, committed entries.
    void archiveCleanupKeepsCommittedEntry();
    /// Missing durable ownership would leave the interrupted output unrecoverable.
    void abandonedOutputIsRecovered();
    /// Recovery must not delete an Asset destination published before a killed producer exits.
    void killedAfterPublicationKeepsDestination();
    /// A stale record for a missing temporary file must not block a later producer.
    void recoveryAndProductionShareTheOwnershipLock();
    /// A clean Apply root stays pinned before the first staging artifact is created.
    void cleanRootCannotBeRenamedDuringRecoveryScope();
    /// A cancelled preparation leaves its durable sibling registration for a later recovery.
    void cancelledPreparationPreservesDurableSibling();
    /// A malformed sibling record cannot authorize deletion of a similarly named Texture file.
    void malformedSiblingOwnershipIsPreserved();
    /// An uppercase native Texture destination still produces a canonical recoverable sibling.
    void uppercaseDdsDestinationUsesRecoverableSibling();
    /// Covers every supported Mesh extension in the canonical durable sibling namespace.
    void meshSiblingRecoveryPreservesOriginal_data();
    /// Recovers abandoned Mesh bytes while preserving the untouched original Mesh.
    void meshSiblingRecoveryPreservesOriginal();
    /// A mismatched Mesh suffix cannot authorize deletion under a valid ownership prefix.
    void malformedMeshSiblingOwnershipIsPreserved();
    /// Recovers abandoned uppercase HKX staging while preserving the original Animation bytes.
    void animationSiblingRecoveryPreservesOriginal();
    /// An Animation prefix with a mismatched suffix cannot authorize evidence deletion.
    void malformedAnimationSiblingOwnershipIsPreserved();
    /// Interrupted snapshot scratch is disposable only under valid manifest ownership.
    void partialScratchIsRecoveredButCorruptOwnershipIsPreserved();
    /// Cleanup removes only registered temporary entries and releases no committed destination.
    void cleanupRemovesTheRunChildAndKeepsCommittedOutput();
    /// A reserved name without a valid ownership proof must never be adopted by a producer.
    void unownedBootstrapIsRejected();
    /// One damaged entry must not stop cleanup of independently registered temporary files.
    void cleanupContinuesAfterADamagedTemporary();
    /// Generic commit cannot release durable staging before or after a separate destination move.
    void genericCommitCannotReleaseDurableStage();
    /// Both publication policies commit staged bytes and release only the temporary path.
    void publicationPoliciesPreserveDestinationBytes();
    /// No-replace publication refuses a destination that became occupied after staging.
    void occupiedDestinationIsNotPublished();
    /// A missing staged file fails before destination mutation and consumes its receipt.
    void unavailableStagedFileConsumesReceipt();
    /// A receipt is movable but cannot authorize a second publication attempt.
    void publicationReceiptIsMoveOnlyAndOneUse();
    /// A receipt cannot invoke an ownership scope after that scope has ended.
    void expiredPublicationScopeCannotPublish();
    /// Asset publication cannot redirect staged bytes to another destination.
    void assetPublicationRejectsChangedDestination();
    /// Archive publication rejects destinations outside its root or in reserved staging.
    void archivePublicationRejectsUnsafeDestinations();
    /// Win32 device aliases cannot become ordinary committed files below a Mod Root.
    void archivePublicationRejectsDeviceAlias();
    /// A linked destination parent cannot redirect committed bytes outside the Mod Root.
    void archivePublicationRejectsLinkedParent();
    /// Replacing an Asset parent after staging invalidates its publication route.
    void assetPublicationRejectsReplacedParent();
    /// Replacement refuses a different leaf even when its pathname and parent stay the same.
    void assetPublicationRejectsReplacedDestination();
    /// A failed durable release retains the published fact and leaves recovery ownership intact.
    void publicationReleaseFailurePreservesCommittedDestination();
    /// Recovery retains an Archive destination and removes abandoned staging after producer death.
    void archivePublicationSurvivesProducerTermination();
};

void DurableStagingTests::abandonedArchiveEntryIsRecovered() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    fs::path temporary;
    {
        cao::run::TemporaryArtifactRegistry registry;
        temporary = registry.stageArchiveFile(root).path;
        QVERIFY(fs::is_regular_file(temporary));
        QCOMPARE(fs::file_size(temporary), std::uintmax_t{0});
        QCOMPARE(temporary.parent_path().parent_path(), root / ".cao-staging");
        QVERIFY(temporary.parent_path().filename().string().starts_with("run-"));
        std::ofstream(temporary, std::ios::binary) << "arbitrary Archive script bytes";
    }
    cao::run::StagingRecovery recovery;
    QVERIFY(!recovery.recover(root).has_value());
    QVERIFY(!fs::exists(temporary));
    QVERIFY(!fs::exists(temporary.parent_path()));
}

void DurableStagingTests::archiveCleanupKeepsCommittedEntry() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    const auto destination = root / "entry.pex";
    cao::run::TemporaryArtifactRegistry registry;
    auto committed = registry.stageArchiveFileForPublication(root);
    auto abandoned = registry.stageArchiveFileForPublication(root);
    const auto committedPath = committed.path();
    const auto abandonedPath = abandoned.path();
    QVERIFY(committedPath != abandonedPath);
    std::ofstream(committedPath, std::ios::binary) << "complete script";
    std::ofstream(abandonedPath, std::ios::binary) << "partial script";
    const auto publication = committed.publish(destination, cao::run::PublicationPolicy::NoReplace);
    QVERIFY2(publication.state == cao::run::PublicationState::PublishedAndReleased,
             publication.errorDetail.c_str());
    QVERIFY(registry.performSafetyCleanup().empty());
    QVERIFY(!fs::exists(abandonedPath));
    QVERIFY(!fs::exists(committedPath.parent_path()));
    QVERIFY_EXCEPTION_THROWN((void)registry.stageArchiveFile(root), std::logic_error);
    QFile output(QString::fromStdWString(destination.wstring()));
    QVERIFY(output.open(QIODevice::ReadOnly));
    QCOMPARE(output.readAll(), QByteArray("complete script"));
}

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

void DurableStagingTests::killedAfterPublicationKeepsDestination() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    std::ofstream(root / "texture.tga") << "source";
    QProcess child;
    child.start(QCoreApplication::applicationFilePath(), {"--staging-crash", directory.path()});
    QVERIFY(child.waitForStarted());
    QVERIFY(child.waitForReadyRead(10000));
    QCOMPARE(child.readAllStandardOutput().trimmed(), QByteArray("published-still-owned"));
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

void DurableStagingTests::cleanRootCannotBeRenamedDuringRecoveryScope() {
#ifdef _WIN32
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    const auto moved = root.parent_path() / (root.filename().wstring() + L"-moved");
    {
        cao::run::StagingRecovery recovery;
        QVERIFY(!recovery.recover(root).has_value());
        QVERIFY(!fs::exists(root / ".cao-staging"));
        std::error_code error;
        fs::rename(root, moved, error);
        if (!error) fs::rename(moved, root);
        QVERIFY(error);
    }
    std::error_code error;
    fs::rename(root, moved, error);
    QVERIFY(!error);
    fs::rename(moved, root, error);
    QVERIFY(!error);
#else
    QSKIP("Root pinning against rename is a Windows runtime contract");
#endif
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

void DurableStagingTests::malformedSiblingOwnershipIsPreserved() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    fs::create_directory(root / "textures");
    fs::path temporary;
    {
        cao::run::TemporaryArtifactRegistry producer;
        temporary = producer.stageFile(root, root / "textures" / "texture.dds").path;
        std::ofstream(temporary) << "partial";
    }
    const auto manifest = root / ".cao-staging" / "ownership.manifest";
    std::ifstream input(manifest, std::ios::binary);
    std::string bytes(std::istreambuf_iterator<char>(input), {});
    input.close();
    const auto relative = temporary.lexically_relative(root).generic_string();
    const auto position = bytes.find(relative);
    QVERIFY(position != std::string::npos);
    bytes.replace(position, relative.size(), "textures/.cao-staging-texture-wrong-short.dds");
    std::ofstream(manifest, std::ios::binary | std::ios::trunc) << bytes;

    cao::run::StagingRecovery recovery;
    const auto failure = recovery.recover(root);

    QVERIFY(failure.has_value());
    QCOMPARE(failure->code(), cao::run::RunFailureCode::StagingOwnershipUnverified);
    QVERIFY(fs::exists(temporary));
}

void DurableStagingTests::uppercaseDdsDestinationUsesRecoverableSibling() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    fs::path temporary;
    {
        cao::run::TemporaryArtifactRegistry producer;
        temporary = producer.stageFile(root, root / "Texture.DDS").path;
        QCOMPARE(temporary.extension(), fs::path(".dds"));
        std::ofstream(temporary) << "partial";
    }

    cao::run::StagingRecovery recovery;
    const auto failure = recovery.recover(root);

    QVERIFY(!failure.has_value());
    QVERIFY(!fs::exists(temporary));
}

void DurableStagingTests::meshSiblingRecoveryPreservesOriginal_data() {
    QTest::addColumn<QString>("extension");
    QTest::newRow("standard-nif") << QStringLiteral("NIF");
    QTest::newRow("terrain-btr") << QStringLiteral("BTR");
    QTest::newRow("terrain-bto") << QStringLiteral("BTO");
}

void DurableStagingTests::meshSiblingRecoveryPreservesOriginal() {
    QFETCH(QString, extension);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    fs::create_directory(root / "meshes");
    const auto destination = root / "meshes" / ("Mesh." + extension.toStdString());
    std::ofstream(destination) << "original Mesh";
    fs::path temporary;
    {
        cao::run::TemporaryArtifactRegistry producer;
        temporary = producer.stageFile(root, destination).path;
        QCOMPARE(temporary.parent_path(), destination.parent_path());
        QCOMPARE(temporary.extension(), fs::path("." + extension.toLower().toStdString()));
        QVERIFY(temporary.filename().string().starts_with(".cao-staging-mesh-"));
        std::ofstream(temporary) << "partial Mesh";
    }
    cao::run::StagingRecovery recovery;
    QVERIFY(!recovery.recover(root).has_value());
    QVERIFY(!fs::exists(temporary));
    QFile original(QString::fromStdWString(destination.wstring()));
    QVERIFY(original.open(QIODevice::ReadOnly));
    QCOMPARE(original.readAll(), QByteArray("original Mesh"));
}

void DurableStagingTests::malformedMeshSiblingOwnershipIsPreserved() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    fs::path temporary;
    {
        cao::run::TemporaryArtifactRegistry producer;
        temporary = producer.stageFile(root, root / "mesh.nif").path;
        std::ofstream(temporary) << "partial Mesh";
    }
    const auto manifest = root / ".cao-staging" / "ownership.manifest";
    std::ifstream input(manifest, std::ios::binary);
    std::string bytes(std::istreambuf_iterator<char>(input), {});
    input.close();
    const auto relative = temporary.lexically_relative(root).generic_string();
    const auto position = bytes.find(relative);
    QVERIFY(position != std::string::npos);
    auto invalid = temporary;
    invalid.replace_extension(".dds");
    // A real similarly named file proves recovery refuses the invalid namespace before deletion.
    std::ofstream(invalid) << "unowned evidence";
    bytes.replace(position, relative.size(), invalid.lexically_relative(root).generic_string());
    std::ofstream(manifest, std::ios::binary | std::ios::trunc) << bytes;
    cao::run::StagingRecovery recovery;
    const auto failure = recovery.recover(root);
    QVERIFY(failure.has_value());
    QCOMPARE(failure->code(), cao::run::RunFailureCode::StagingOwnershipUnverified);
    QVERIFY(fs::exists(temporary));
    QVERIFY(fs::exists(invalid));
}

void DurableStagingTests::animationSiblingRecoveryPreservesOriginal() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    fs::create_directory(root / "animations");
    const auto destination = root / "animations" / "Walk.HKX";
    std::ofstream(destination) << "original Animation";
    fs::path temporary;
    {
        cao::run::TemporaryArtifactRegistry producer;
        temporary = producer.stageFile(root, destination).path;
        QCOMPARE(temporary.parent_path(), destination.parent_path());
        QCOMPARE(temporary.extension(), fs::path(".hkx"));
        QVERIFY(temporary.filename().string().starts_with(".cao-staging-animation-"));
        std::ofstream(temporary) << "partial Animation";
    }

    cao::run::StagingRecovery recovery;
    QVERIFY(!recovery.recover(root).has_value());
    QVERIFY(!fs::exists(temporary));
    QFile original(QString::fromStdWString(destination.wstring()));
    QVERIFY(original.open(QIODevice::ReadOnly));
    QCOMPARE(original.readAll(), QByteArray("original Animation"));
}

void DurableStagingTests::malformedAnimationSiblingOwnershipIsPreserved() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    fs::path temporary;
    {
        cao::run::TemporaryArtifactRegistry producer;
        temporary = producer.stageFile(root, root / "walk.hkx").path;
        std::ofstream(temporary) << "partial Animation";
    }
    const auto manifest = root / ".cao-staging" / "ownership.manifest";
    std::ifstream input(manifest, std::ios::binary);
    std::string bytes(std::istreambuf_iterator<char>(input), {});
    input.close();
    const auto relative = temporary.lexically_relative(root).generic_string();
    const auto position = bytes.find(relative);
    QVERIFY(position != std::string::npos);
    auto invalid = temporary;
    invalid.replace_extension(".nif");
    // A valid Mesh extension still cannot prove ownership under an Animation staging prefix.
    std::ofstream(invalid) << "unowned evidence";
    bytes.replace(position, relative.size(), invalid.lexically_relative(root).generic_string());
    std::ofstream(manifest, std::ios::binary | std::ios::trunc) << bytes;

    cao::run::StagingRecovery recovery;
    const auto failure = recovery.recover(root);
    QVERIFY(failure.has_value());
    QCOMPARE(failure->code(), cao::run::RunFailureCode::StagingOwnershipUnverified);
    QFile partial(QString::fromStdWString(temporary.wstring()));
    QVERIFY(partial.open(QIODevice::ReadOnly));
    QCOMPARE(partial.readAll(), QByteArray("partial Animation"));
    QFile evidence(QString::fromStdWString(invalid.wstring()));
    QVERIFY(evidence.open(QIODevice::ReadOnly));
    QCOMPARE(evidence.readAll(), QByteArray("unowned evidence"));
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
    auto staged = registry.stageFileForPublication(root, destination);
    const auto stagedPath = staged.path();
    std::ofstream(stagedPath) << "committed";
    const auto publication = staged.publish(destination, cao::run::PublicationPolicy::Replace);
    QVERIFY2(publication.state == cao::run::PublicationState::PublishedAndReleased,
             publication.errorDetail.c_str());
    auto uncommitted = registry.stageFileForPublication(root, root / "other.dds");
    const auto uncommittedPath = uncommitted.path();
    QVERIFY(registry.performSafetyCleanup().empty());
    QVERIFY(!fs::exists(stagedPath));
    QVERIFY(!fs::exists(uncommittedPath));
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

void DurableStagingTests::genericCommitCannotReleaseDurableStage() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    const auto destination = root / "texture.dds";
    std::ofstream(destination) << "original";
    cao::run::TemporaryArtifactRegistry registry;
    const auto staged = registry.stageFile(root, destination);
    QVERIFY_EXCEPTION_THROWN(registry.commit(staged.registration), std::logic_error);
    QVERIFY(fs::exists(staged.path));
    const auto archive = registry.stageArchiveFile(root);
    QVERIFY_EXCEPTION_THROWN(registry.commit(archive.registration), std::logic_error);
    const auto separatelyMoved = root / "separately-moved.dds";
    fs::rename(staged.path, separatelyMoved);
    QVERIFY_EXCEPTION_THROWN(registry.commit(staged.registration), std::logic_error);
    // Restoring the registered name makes retained ownership observable through cleanup.
    fs::rename(separatelyMoved, staged.path);
    QVERIFY(registry.performSafetyCleanup().empty());
    QVERIFY(!fs::exists(staged.path));
    QVERIFY(!fs::exists(archive.path));
    QFile original(QString::fromStdWString(destination.wstring()));
    QVERIFY(original.open(QIODevice::ReadOnly));
    QCOMPARE(original.readAll(), QByteArray("original"));
}

void DurableStagingTests::publicationPoliciesPreserveDestinationBytes() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    const auto assetDestination = root / "texture.dds";
    const auto archiveDestination = root / "entry.pex";
    std::ofstream(assetDestination, std::ios::binary) << "old asset";
    cao::run::TemporaryArtifactRegistry registry;

    auto asset = registry.stageFileForPublication(root, assetDestination);
    const auto assetTemporary = asset.path();
    std::ofstream(assetTemporary, std::ios::binary) << "new asset";
    const auto assetResult =
        asset.publish(assetDestination, cao::run::PublicationPolicy::Replace);
    QVERIFY2(assetResult.state == cao::run::PublicationState::PublishedAndReleased,
             assetResult.errorDetail.c_str());
    QVERIFY(assetResult.errorDetail.empty());

    auto archive = registry.stageArchiveFileForPublication(root);
    const auto archiveTemporary = archive.path();
    std::ofstream(archiveTemporary, std::ios::binary) << "new archive entry";
    const auto archiveResult =
        archive.publish(archiveDestination, cao::run::PublicationPolicy::NoReplace);
    QCOMPARE(archiveResult.state, cao::run::PublicationState::PublishedAndReleased);
    QVERIFY(archiveResult.errorDetail.empty());
    QVERIFY(registry.performSafetyCleanup().empty());
    QVERIFY(!fs::exists(assetTemporary));
    QVERIFY(!fs::exists(archiveTemporary));
    QFile assetBytes(QString::fromStdWString(assetDestination.wstring()));
    QVERIFY(assetBytes.open(QIODevice::ReadOnly));
    QCOMPARE(assetBytes.readAll(), QByteArray("new asset"));
    QFile archiveBytes(QString::fromStdWString(archiveDestination.wstring()));
    QVERIFY(archiveBytes.open(QIODevice::ReadOnly));
    QCOMPARE(archiveBytes.readAll(), QByteArray("new archive entry"));
}

void DurableStagingTests::occupiedDestinationIsNotPublished() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    const auto destination = root / "entry.pex";
    cao::run::TemporaryArtifactRegistry registry;
    auto receipt = registry.stageArchiveFileForPublication(root);
    const auto temporary = receipt.path();
    std::ofstream(temporary, std::ios::binary) << "new entry";
    // Occupy the leaf after staging so the final native no-replace operation must arbitrate it.
    std::ofstream(destination, std::ios::binary) << "competing entry";
    const auto result = receipt.publish(destination, cao::run::PublicationPolicy::NoReplace);
    QCOMPARE(result.state, cao::run::PublicationState::NotPublished);
    QVERIFY(!result.errorDetail.empty());
    QVERIFY(fs::exists(temporary));
    QVERIFY(registry.performSafetyCleanup().empty());
    QVERIFY(!fs::exists(temporary));
    QFile output(QString::fromStdWString(destination.wstring()));
    QVERIFY(output.open(QIODevice::ReadOnly));
    QCOMPARE(output.readAll(), QByteArray("competing entry"));
}

void DurableStagingTests::unavailableStagedFileConsumesReceipt() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    const auto destination = root / "texture.dds";
    std::ofstream(destination, std::ios::binary) << "original";
    cao::run::TemporaryArtifactRegistry registry;
    auto receipt = registry.stageFileForPublication(root, destination);
    const auto temporary = receipt.path();
    QVERIFY(fs::remove(temporary));
    const auto first = receipt.publish(destination, cao::run::PublicationPolicy::Replace);
    QCOMPARE(first.state, cao::run::PublicationState::NotPublished);
    QVERIFY(!first.errorDetail.empty());
    // Recreating the same owned name must not revive a consumed publication authority.
    std::ofstream(temporary, std::ios::binary) << "late bytes";
    const auto second = receipt.publish(destination, cao::run::PublicationPolicy::Replace);
    QCOMPARE(second.state, cao::run::PublicationState::NotPublished);
    QVERIFY(!second.errorDetail.empty());
    QVERIFY(registry.performSafetyCleanup().empty());
    QVERIFY(!fs::exists(temporary));
    QFile output(QString::fromStdWString(destination.wstring()));
    QVERIFY(output.open(QIODevice::ReadOnly));
    QCOMPARE(output.readAll(), QByteArray("original"));
}

void DurableStagingTests::publicationReceiptIsMoveOnlyAndOneUse() {
    using Receipt = cao::run::TemporaryArtifactRegistry::PublicationReceipt;
    static_assert(!std::is_copy_constructible_v<Receipt>);
    static_assert(!std::is_copy_assignable_v<Receipt>);
    static_assert(std::is_move_constructible_v<Receipt>);

    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    const auto destination = root / "texture.dds";
    cao::run::TemporaryArtifactRegistry registry;
    auto original = registry.stageFileForPublication(root, destination);
    auto receipt = std::move(original);
    std::ofstream(receipt.path(), std::ios::binary) << "once";
    const auto first = receipt.publish(destination, cao::run::PublicationPolicy::Replace);
    QVERIFY2(first.state == cao::run::PublicationState::PublishedAndReleased,
             first.errorDetail.c_str());
    const auto second = receipt.publish(destination, cao::run::PublicationPolicy::Replace);
    QCOMPARE(second.state, cao::run::PublicationState::NotPublished);
    QVERIFY(!second.errorDetail.empty());
    QVERIFY(registry.performSafetyCleanup().empty());
    QFile output(QString::fromStdWString(destination.wstring()));
    QVERIFY(output.open(QIODevice::ReadOnly));
    QCOMPARE(output.readAll(), QByteArray("once"));
}

void DurableStagingTests::expiredPublicationScopeCannotPublish() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    const auto destination = root / "entry.pex";
    std::optional<cao::run::TemporaryArtifactRegistry::PublicationReceipt> receipt;
    fs::path temporary;
    {
        cao::run::TemporaryArtifactRegistry registry;
        receipt.emplace(registry.stageArchiveFileForPublication(root));
        temporary = receipt->path();
        std::ofstream(temporary, std::ios::binary) << "unpublished";
    }
    QVERIFY_EXCEPTION_THROWN((void)receipt->path(), std::logic_error);
    const auto result = receipt->publish(destination, cao::run::PublicationPolicy::NoReplace);
    QCOMPARE(result.state, cao::run::PublicationState::NotPublished);
    QVERIFY(!result.errorDetail.empty());
    QVERIFY(!fs::exists(destination));
    cao::run::StagingRecovery recovery;
    QVERIFY(!recovery.recover(root).has_value());
    QVERIFY(!fs::exists(temporary));
}

void DurableStagingTests::assetPublicationRejectsChangedDestination() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    const auto intended = root / "intended.dds";
    const auto redirected = root / "redirected.dds";
    cao::run::TemporaryArtifactRegistry registry;
    auto receipt = registry.stageFileForPublication(root, intended);
    const auto temporary = receipt.path();
    std::ofstream(temporary, std::ios::binary) << "staged";
    const auto result = receipt.publish(redirected, cao::run::PublicationPolicy::Replace);
    QCOMPARE(result.state, cao::run::PublicationState::NotPublished);
    QVERIFY(!result.errorDetail.empty());
    QVERIFY(!fs::exists(intended));
    QVERIFY(!fs::exists(redirected));
    QVERIFY(fs::exists(temporary));
    QVERIFY(registry.performSafetyCleanup().empty());
    QVERIFY(!fs::exists(temporary));
}

void DurableStagingTests::archivePublicationRejectsUnsafeDestinations() {
    QTemporaryDir directory;
    QTemporaryDir outside;
    QVERIFY(directory.isValid());
    QVERIFY(outside.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    const auto escaped = fs::canonical(fs::path(outside.path().toStdWString())) / "escape.pex";
    const auto reserved = root / ".cao-staging" / "reserved.pex";
    cao::run::TemporaryArtifactRegistry registry;

    auto escapedReceipt = registry.stageArchiveFileForPublication(root);
    const auto escapedTemporary = escapedReceipt.path();
    std::ofstream(escapedTemporary, std::ios::binary) << "escape";
    const auto escapedResult =
        escapedReceipt.publish(escaped, cao::run::PublicationPolicy::NoReplace);
    QCOMPARE(escapedResult.state, cao::run::PublicationState::NotPublished);
    QVERIFY(!escapedResult.errorDetail.empty());
    QVERIFY(!fs::exists(escaped));

    auto reservedReceipt = registry.stageArchiveFileForPublication(root);
    const auto reservedTemporary = reservedReceipt.path();
    std::ofstream(reservedTemporary, std::ios::binary) << "reserved";
    const auto reservedResult =
        reservedReceipt.publish(reserved, cao::run::PublicationPolicy::NoReplace);
    QCOMPARE(reservedResult.state, cao::run::PublicationState::NotPublished);
    QVERIFY(!reservedResult.errorDetail.empty());
    QVERIFY(!fs::exists(reserved));

    auto relativeReceipt = registry.stageArchiveFileForPublication(root);
    const auto relativeTemporary = relativeReceipt.path();
    std::ofstream(relativeTemporary, std::ios::binary) << "relative";
    const auto relativeResult = relativeReceipt.publish(
        fs::path("relative.pex"), cao::run::PublicationPolicy::NoReplace);
    QCOMPARE(relativeResult.state, cao::run::PublicationState::NotPublished);
    QVERIFY(!relativeResult.errorDetail.empty());

    QVERIFY(registry.performSafetyCleanup().empty());
    QVERIFY(!fs::exists(escapedTemporary));
    QVERIFY(!fs::exists(reservedTemporary));
    QVERIFY(!fs::exists(relativeTemporary));
}

void DurableStagingTests::archivePublicationRejectsDeviceAlias() {
#ifdef _WIN32
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    cao::run::TemporaryArtifactRegistry registry;
    auto receipt = registry.stageArchiveFileForPublication(root);
    const auto temporary = receipt.path();
    std::ofstream(temporary, std::ios::binary) << "staged";
    const auto result = receipt.publish(root / "NUL.pex", cao::run::PublicationPolicy::NoReplace);
    QCOMPARE(result.state, cao::run::PublicationState::NotPublished);
    QVERIFY(!result.errorDetail.empty());
    QVERIFY(fs::exists(temporary));
    QVERIFY(registry.performSafetyCleanup().empty());
#else
    QSKIP("DOS device aliases apply only to Windows publication");
#endif
}

void DurableStagingTests::archivePublicationRejectsLinkedParent() {
    QTemporaryDir directory;
    QTemporaryDir outside;
    QVERIFY(directory.isValid());
    QVERIFY(outside.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    const auto outsideRoot = fs::canonical(fs::path(outside.path().toStdWString()));
    const auto linkedParent = root / "linked";
    std::error_code linkError;
    fs::create_directory_symlink(outsideRoot, linkedParent, linkError);
    if (linkError) QSKIP("This filesystem does not permit directory symlinks");

    cao::run::TemporaryArtifactRegistry registry;
    auto receipt = registry.stageArchiveFileForPublication(root);
    const auto temporary = receipt.path();
    std::ofstream(temporary, std::ios::binary) << "staged";
    const auto result =
        receipt.publish(linkedParent / "entry.pex", cao::run::PublicationPolicy::NoReplace);
    QCOMPARE(result.state, cao::run::PublicationState::NotPublished);
    QVERIFY(!result.errorDetail.empty());
    QVERIFY(!fs::exists(outsideRoot / "entry.pex"));
    QVERIFY(registry.performSafetyCleanup().empty());
    QVERIFY(!fs::exists(temporary));
    QVERIFY(fs::remove(linkedParent));
}

void DurableStagingTests::assetPublicationRejectsReplacedParent() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    const auto parent = root / "assets";
    const auto shiftedParent = root / "shifted-assets";
    QVERIFY(fs::create_directory(parent));
    const auto destination = parent / "texture.dds";
    cao::run::TemporaryArtifactRegistry registry;
    auto receipt = registry.stageFileForPublication(root, destination);
    const auto temporary = receipt.path();
    std::ofstream(temporary, std::ios::binary) << "staged";
    std::error_code renameError;
    fs::rename(parent, shiftedParent, renameError);
    if (renameError) {
        QVERIFY(registry.performSafetyCleanup().empty());
        QSKIP("The staged parent is pinned against replacement by this filesystem");
    }
    QVERIFY(fs::create_directory(parent));
    fs::rename(shiftedParent / temporary.filename(), temporary, renameError);
    if (renameError) {
        fs::remove(parent);
        fs::rename(shiftedParent, parent);
        QVERIFY(registry.performSafetyCleanup().empty());
        QSKIP("The staged file is pinned against relocation by this filesystem");
    }
    // The staged name exists again, so only the changed parent identity can reject this route.
    const auto result = receipt.publish(destination, cao::run::PublicationPolicy::Replace);
    QCOMPARE(result.state, cao::run::PublicationState::NotPublished);
    QVERIFY(!result.errorDetail.empty());
    QVERIFY(!fs::exists(destination));
    fs::rename(temporary, shiftedParent / temporary.filename());
    QVERIFY(fs::remove(parent));
    fs::rename(shiftedParent, parent);
    QVERIFY(registry.performSafetyCleanup().empty());
    QVERIFY(!fs::exists(temporary));
}

void DurableStagingTests::assetPublicationRejectsReplacedDestination() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    const auto destination = root / "texture.dds";
    const auto oldDestination = root / "old-texture.dds";
    std::ofstream(destination, std::ios::binary) << "original";
    cao::run::TemporaryArtifactRegistry registry;
    auto receipt = registry.stageFileForPublication(root, destination);
    const auto temporary = receipt.path();
    std::ofstream(temporary, std::ios::binary) << "optimized original";
    fs::rename(destination, oldDestination);
    std::ofstream(destination, std::ios::binary) << "newcomer";

    const auto result = receipt.publish(destination, cao::run::PublicationPolicy::Replace);
    QCOMPARE(result.state, cao::run::PublicationState::NotPublished);
    QVERIFY(!result.errorDetail.empty());
    QCOMPARE(fs::file_size(destination), std::uintmax_t{8});
    QCOMPARE(fs::file_size(oldDestination), std::uintmax_t{8});
    QVERIFY(registry.performSafetyCleanup().empty());
    QVERIFY(!fs::exists(temporary));
}

void DurableStagingTests::publicationReleaseFailurePreservesCommittedDestination() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    const auto destination = root / "entry.pex";
    fs::path temporary;
    const auto scratch = root / ".cao-staging" / "ownership.manifest.next";
    {
        cao::run::TemporaryArtifactRegistry registry;
        auto receipt = registry.stageArchiveFileForPublication(root);
        temporary = receipt.path();
        std::ofstream(temporary, std::ios::binary) << "committed entry";
        // The fixed scratch name blocks only the post-publication manifest release.
        std::ofstream(scratch, std::ios::binary) << "occupied scratch";
        const auto result =
            receipt.publish(destination, cao::run::PublicationPolicy::NoReplace);
        QVERIFY2(result.state == cao::run::PublicationState::PublishedStillOwned,
                 result.errorDetail.c_str());
        QVERIFY(!result.errorDetail.empty());
        QVERIFY(!fs::exists(temporary));
        QFile output(QString::fromStdWString(destination.wstring()));
        QVERIFY(output.open(QIODevice::ReadOnly));
        QCOMPARE(output.readAll(), QByteArray("committed entry"));
        QVERIFY(registry.performSafetyCleanup().empty());
        cao::run::StagingRecovery contender;
        const auto active = contender.recover(root);
        QVERIFY(active.has_value());
        QCOMPARE(active->code(), cao::run::RunFailureCode::StagingActive);
    }
    cao::run::StagingRecovery recovery;
    QVERIFY(!recovery.recover(root).has_value());
    QVERIFY(!fs::exists(scratch));
    QVERIFY(!fs::exists(temporary));
    QFile output(QString::fromStdWString(destination.wstring()));
    QVERIFY(output.open(QIODevice::ReadOnly));
    QCOMPARE(output.readAll(), QByteArray("committed entry"));
}

void DurableStagingTests::archivePublicationSurvivesProducerTermination() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = fs::canonical(fs::path(directory.path().toStdWString()));
    const auto destination = root / "scripts" / "entry.pex";
    QVERIFY(fs::create_directory(destination.parent_path()));
    const auto unrelated = root / "unrelated.txt";
    std::ofstream(unrelated, std::ios::binary) << "keep this";

    QProcess producer;
    producer.start(QCoreApplication::applicationFilePath(),
                   {"--archive-publication-crash", QString::fromStdWString(root.wstring())});
    QVERIFY(producer.waitForReadyRead(10000));
    QCOMPARE(producer.readAllStandardOutput().trimmed(), QByteArray("published-still-owned"));
    cao::run::StagingRecovery contender;
    const auto active = contender.recover(root);
    QVERIFY(active.has_value());
    QCOMPARE(active->code(), cao::run::RunFailureCode::StagingActive);
    producer.kill();
    QVERIFY(producer.waitForFinished());

    cao::run::StagingRecovery recovery;
    QVERIFY(!recovery.recover(root).has_value());
    QFile output(QString::fromStdWString(destination.wstring()));
    QVERIFY(output.open(QIODevice::ReadOnly));
    QCOMPARE(output.readAll(), QByteArray("committed entry"));
    QFile retained(QString::fromStdWString(unrelated.wstring()));
    QVERIFY(retained.open(QIODevice::ReadOnly));
    QCOMPARE(retained.readAll(), QByteArray("keep this"));
    const auto staging = root / ".cao-staging";
    for (const auto& entry : fs::directory_iterator(staging))
        QVERIFY(!entry.is_directory());
    QVERIFY(!fs::exists(staging / "ownership.manifest.next"));
}

/// Runs a real interrupted producer without destructor cleanup, or the normal Qt test suite.
int main(int argc, char** argv) {
    QCoreApplication application(argc, argv);
    if (application.arguments().size() == 3 &&
        application.arguments().at(1) == "--archive-publication-crash") {
        const auto root = fs::canonical(fs::path(application.arguments().at(2).toStdWString()));
        cao::run::TemporaryArtifactRegistry registry;
        auto published = registry.stageArchiveFileForPublication(root);
        auto abandoned = registry.stageArchiveFileForPublication(root);
        std::ofstream(published.path(), std::ios::binary) << "committed entry";
        std::ofstream(abandoned.path(), std::ios::binary) << "abandoned entry";
        // Occupying scratch after both stages forces release to fail after native publication.
        std::ofstream(root / ".cao-staging" / "ownership.manifest.next", std::ios::binary)
            << "occupied scratch";
        const auto result = published.publish(root / "scripts" / "entry.pex",
                                              cao::run::PublicationPolicy::NoReplace);
        if (result.state != cao::run::PublicationState::PublishedStillOwned) return 2;
        std::fputs("published-still-owned\n", stdout);
        std::fflush(stdout);
        return application.exec();
    }
    if (application.arguments().size() == 3 && application.arguments().at(1) == "--staging-crash") {
        const auto root = fs::canonical(fs::path(application.arguments().at(2).toStdWString()));
        cao::run::TemporaryArtifactRegistry registry;
        auto staged = registry.stageFileForPublication(root, root / "texture.dds");
        std::ofstream(staged.path()) << "converted";
        // The occupied snapshot name holds the old claim after destination publication.
        std::ofstream(root / ".cao-staging" / "ownership.manifest.next") << "occupied scratch";
        const auto result =
            staged.publish(root / "texture.dds", cao::run::PublicationPolicy::Replace);
        if (result.state != cao::run::PublicationState::PublishedStillOwned) return 2;
        std::fputs("published-still-owned\n", stdout);
        std::fflush(stdout);
        return application.exec();
    }
    DurableStagingTests tests;
    return QTest::qExec(&tests, argc, argv);
}
#include "DurableStagingTests.moc"
