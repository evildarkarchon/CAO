#include "ArchiveRecovery.h"

#include "ArchiveFileOperations.h"
#include "ArchiveTransactionWorkspace.h"

#include <catch2/catch_test_macros.hpp>

#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QProcess>
#include <QProcessEnvironment>
#include <QTemporaryDir>

#include <Windows.h>

#include <algorithm>
#include <cstdlib>
#include <memory>

namespace {
constexpr auto FirstTransactionId = "5e30ea4c-90a7-4af7-8381-67ecb234f4df";
constexpr auto SecondTransactionId = "a7262d88-3546-46b4-bcee-b41154784e63";
constexpr auto CrashHelperModEnvironment = "CAO_ARCHIVE_CRASH_HELPER_MOD";

QString currentExecutablePath() {
  std::wstring buffer(MAX_PATH, L'\0');
  const DWORD written = GetModuleFileNameW(nullptr, buffer.data(),
                                           static_cast<DWORD>(buffer.size()));
  if (written == 0 || written >= buffer.size())
    return {};
  buffer.resize(written);
  return QString::fromStdWString(buffer);
}

ArchiveTransactionManifest manifestFor(const QString &modPath,
                                       const QString &transactionId,
                                       ArchiveFileOperations &files) {
  ArchiveTransactionManifest manifest;
  manifest.transactionId = transactionId;
  manifest.kind = ArchiveTransactionKind::Packing;
  manifest.canonicalModPath = QDir::cleanPath(modPath);
  manifest.modIdentity = files.identity(manifest.canonicalModPath);
  manifest.canonicalAnchorPath = QDir::cleanPath(modPath);
  manifest.anchorIdentity = manifest.modIdentity;
  manifest.volumeIdentity = manifest.modIdentity;
  return manifest;
}

ArchiveTransactionWorkspace
createWorkspace(const QString &modPath, const QString &transactionId,
                const std::shared_ptr<QtArchiveFileOperations> &durability) {
  return ArchiveTransactionWorkspace::create(
      manifestFor(modPath, transactionId, *durability), durability);
}

void commitWorkspace(ArchiveTransactionWorkspace &workspace,
                     const QMap<QString, QString> &commitFields = {}) {
  workspace.append(ArchiveTransactionRecordKind::Intent,
                   {{"source", "textures/a.dds"}});
  workspace.append(ArchiveTransactionRecordKind::MutationComplete,
                   {{"destination", "published/a.dds"}});
  workspace.commit(commitFields);
}
} // namespace

TEST_CASE("Archive process-kill helper", "[.archive-crash-helper]") {
  const QString modPath = qEnvironmentVariable(CrashHelperModEnvironment);
  if (modPath.isEmpty()) {
    SUCCEED("helper is inert outside its parent process");
    return;
  }

  QtArchiveFileOperations files;
  const QString archivePath = QDir(modPath).filePath("Alpha.bsa");
  files.preflightTransaction(modPath);
  ArchiveTransactionManifest manifest;
  manifest.transactionId = "12345678-1234-1234-1234-123456789abc";
  manifest.kind = ArchiveTransactionKind::Extraction;
  manifest.canonicalModPath = QFileInfo(modPath).absoluteFilePath();
  manifest.modIdentity = files.identity(modPath);
  manifest.canonicalAnchorPath = archivePath;
  manifest.anchorIdentity = files.identity(archivePath);
  manifest.volumeIdentity = manifest.modIdentity;
  auto durability = std::shared_ptr<ArchiveTransactionDurability>(
      &files, [](ArchiveTransactionDurability *) {});
  auto workspace =
      ArchiveTransactionWorkspace::create(manifest, std::move(durability));
  const QString rollbackPath =
      QDir(workspace.path()).filePath("rollback/source-archive");
  files.createDirectories(QFileInfo(rollbackPath).absolutePath());
  workspace.append(ArchiveTransactionRecordKind::Intent,
                   {{QStringLiteral("operation"), QStringLiteral("move")},
                    {QStringLiteral("source"), archivePath},
                    {QStringLiteral("destination"), rollbackPath}});
  files.move(archivePath, rollbackPath);

  // Simulate process termination after the live mutation but before its
  // completion frame; no stack unwinding or runtime rollback is allowed.
  TerminateProcess(GetCurrentProcess(), 91);
  std::abort();
}

TEST_CASE("Archive Recovery restores pre-state after a real process kill") {
  QTemporaryDir temporary;
  REQUIRE(temporary.isValid());
  const QString modPath = QDir(temporary.path()).filePath("Alpha");
  REQUIRE(QDir().mkpath(modPath));
  const QString archivePath = QDir(modPath).filePath("Alpha.bsa");
  QFile archive(archivePath);
  REQUIRE(archive.open(QIODevice::WriteOnly));
  REQUIRE(archive.write("original-archive") == 16);
  archive.close();

  QProcess helper;
  QProcessEnvironment environment = QProcessEnvironment::systemEnvironment();
  environment.insert(CrashHelperModEnvironment, modPath);
  helper.setProcessEnvironment(environment);
  helper.start(currentExecutablePath(),
               {QStringLiteral("Archive process-kill helper")});
  const bool finished = helper.waitForFinished(30000);
  INFO(helper.errorString().toStdString());
  INFO(helper.readAllStandardError().toStdString());
  INFO(helper.readAllStandardOutput().toStdString());
  REQUIRE(finished);
  REQUIRE(helper.exitCode() == 91);
  REQUIRE_FALSE(QFileInfo::exists(archivePath));

  QtArchiveFileOperations files;
  const auto result = ArchiveRecovery::recover({modPath}, false, files);
  REQUIRE(result.outcome == ArchiveRecoveryOutcome::Recovered);
  QFile restored(archivePath);
  REQUIRE(restored.open(QIODevice::ReadOnly));
  REQUIRE(restored.readAll() == QByteArray("original-archive"));
  REQUIRE_FALSE(QFileInfo::exists(
      QDir(modPath).filePath(ArchiveTransactionWorkspace::ReservedRootName)));
}

TEST_CASE(
    "Archive Recovery reports a clean locked Mod without creating files") {
  QTemporaryDir temporary;
  REQUIRE(temporary.isValid());
  const QString modPath = QDir(temporary.path()).filePath("Alpha");
  REQUIRE(QDir().mkpath(modPath));
  auto durability = std::make_shared<QtArchiveFileOperations>();

  const ArchiveRecoveryResult result =
      ArchiveRecovery::recover({modPath}, false, *durability);

  REQUIRE(result.outcome == ArchiveRecoveryOutcome::Clean);
  REQUIRE(result.workspaces.isEmpty());
  REQUIRE_FALSE(QFileInfo::exists(
      QDir(modPath).filePath(ArchiveTransactionWorkspace::ReservedRootName)));
}

TEST_CASE(
    "Archive Recovery Dry Run reports committed cleanup without mutation") {
  QTemporaryDir temporary;
  REQUIRE(temporary.isValid());
  const QString modPath = QDir(temporary.path()).filePath("Alpha");
  REQUIRE(QDir().mkpath(modPath));
  auto durability = std::make_shared<QtArchiveFileOperations>();
  auto workspace = createWorkspace(modPath, FirstTransactionId, durability);
  commitWorkspace(workspace);

  const ArchiveRecoveryResult result =
      ArchiveRecovery::recover({modPath}, true, *durability);

  REQUIRE(result.outcome == ArchiveRecoveryOutcome::RecoveryRequired);
  REQUIRE(result.workspaces.size() == 1);
  REQUIRE(result.workspaces.front().action ==
          ArchiveRecoveryAction::CommittedCleanup);
  REQUIRE(QFileInfo::exists(workspace.path()));
}

TEST_CASE(
    "Archive Recovery completes committed cleanup and removes empty root") {
  QTemporaryDir temporary;
  REQUIRE(temporary.isValid());
  const QString modPath = QDir(temporary.path()).filePath("Alpha");
  REQUIRE(QDir().mkpath(modPath));
  auto durability = std::make_shared<QtArchiveFileOperations>();
  auto workspace = createWorkspace(modPath, FirstTransactionId, durability);
  const QString deferredDeletion =
      QDir(workspace.path()).filePath("rollback/deferred-delete.bsa");
  REQUIRE(QDir().mkpath(QFileInfo(deferredDeletion).absolutePath()));
  QFile rollbackCopy(deferredDeletion);
  REQUIRE(rollbackCopy.open(QIODevice::WriteOnly));
  REQUIRE(rollbackCopy.write("rollback copy") > 0);
  rollbackCopy.close();
  commitWorkspace(workspace, {{"cleanup-file.0", deferredDeletion}});
  const QString workspacePath = workspace.path();
  const QString rootPath = QFileInfo(workspacePath).absolutePath();

  const ArchiveRecoveryResult result =
      ArchiveRecovery::recover({modPath}, false, *durability);

  REQUIRE(result.outcome == ArchiveRecoveryOutcome::Recovered);
  REQUIRE(result.workspaces.size() == 1);
  REQUIRE_FALSE(QFileInfo::exists(deferredDeletion));
  REQUIRE_FALSE(QFileInfo::exists(workspacePath));
  REQUIRE_FALSE(QFileInfo::exists(rootPath));

  const ArchiveRecoveryResult second =
      ArchiveRecovery::recover({modPath}, false, *durability);
  REQUIRE(second.outcome == ArchiveRecoveryOutcome::Clean);
}

TEST_CASE("Archive Recovery replays one incomplete history before cleanup") {
  QTemporaryDir temporary;
  REQUIRE(temporary.isValid());
  const QString modPath = QDir(temporary.path()).filePath("Alpha");
  REQUIRE(QDir().mkpath(modPath));
  auto durability = std::make_shared<QtArchiveFileOperations>();
  auto committed = createWorkspace(modPath, FirstTransactionId, durability);
  commitWorkspace(committed);
  auto workspace = createWorkspace(modPath, SecondTransactionId, durability);
  QFile liveArchive(QDir(modPath).filePath("Alpha.bsa"));
  REQUIRE(liveArchive.open(QIODevice::WriteOnly));
  REQUIRE(liveArchive.write("archive") > 0);
  liveArchive.close();
  workspace.append(
      ArchiveTransactionRecordKind::Intent,
      {{"operation", "move"},
       {"source", QDir(modPath).filePath("Alpha.bsa")},
       {"destination", QDir(workspace.path()).filePath("rollback/Alpha.bsa")}});

  const ArchiveRecoveryResult result =
      ArchiveRecovery::recover({modPath}, false, *durability);

  REQUIRE(result.outcome == ArchiveRecoveryOutcome::Recovered);
  REQUIRE(result.workspaces.size() == 2);
  REQUIRE(std::any_of(result.workspaces.cbegin(), result.workspaces.cend(),
                      [&](const auto &candidate) {
                        return candidate.path == workspace.path() &&
                               candidate.action ==
                                   ArchiveRecoveryAction::IncompleteRollback;
                      }));
  REQUIRE_FALSE(QFileInfo::exists(committed.path()));
  REQUIRE_FALSE(QFileInfo::exists(workspace.path()));
}

TEST_CASE(
    "Archive Recovery rejects multiple incomplete histories before mutation") {
  QTemporaryDir temporary;
  REQUIRE(temporary.isValid());
  const QString modPath = QDir(temporary.path()).filePath("Alpha");
  REQUIRE(QDir().mkpath(modPath));
  auto durability = std::make_shared<QtArchiveFileOperations>();
  auto committed = createWorkspace(modPath, FirstTransactionId, durability);
  commitWorkspace(committed);
  auto incomplete = createWorkspace(modPath, SecondTransactionId, durability);
  incomplete.append(ArchiveTransactionRecordKind::Intent,
                    {{"source", "Alpha.bsa"}});
  auto third = createWorkspace(modPath, "046a4db9-0eed-4800-8468-8c36a4fb9095",
                               durability);
  third.append(ArchiveTransactionRecordKind::Intent,
               {{"source", "Alpha - Textures.bsa"}});

  try {
    static_cast<void>(ArchiveRecovery::recover({modPath}, false, *durability));
    FAIL("Expected ambiguous recovery histories to fail");
  } catch (const ArchiveExecutionError &error) {
    REQUIRE(error.operation() == ArchiveOperation::Recovery);
    REQUIRE(error.stage() == ArchiveFailureStage::RecoveryValidation);
    REQUIRE(error.assetPath() == QDir::cleanPath(modPath));
    REQUIRE(error.diagnostic().contains("multiple incomplete"));
    REQUIRE(error.workspacePaths().size() == 2);
  }

  REQUIRE(QFileInfo::exists(committed.path()));
  REQUIRE(QFileInfo::exists(incomplete.path()));
  REQUIRE(QFileInfo::exists(third.path()));
}

TEST_CASE("Archive Recovery rejects an unowned reserved root with its path") {
  QTemporaryDir temporary;
  REQUIRE(temporary.isValid());
  const QString modPath = QDir(temporary.path()).filePath("Alpha");
  const QString rootPath =
      QDir(modPath).filePath(ArchiveTransactionWorkspace::ReservedRootName);
  REQUIRE(QDir().mkpath(rootPath));
  const QString obstructionPath = QDir(rootPath).filePath("legacy-state.txt");
  QFile obstruction(obstructionPath);
  REQUIRE(obstruction.open(QIODevice::WriteOnly));
  REQUIRE(obstruction.write("unknown owner") > 0);
  obstruction.close();
  auto durability = std::make_shared<QtArchiveFileOperations>();

  try {
    static_cast<void>(ArchiveRecovery::recover({modPath}, false, *durability));
    FAIL("Expected an unowned root to require manual resolution");
  } catch (const ArchiveExecutionError &error) {
    REQUIRE(error.operation() == ArchiveOperation::Recovery);
    REQUIRE(error.stage() == ArchiveFailureStage::RecoveryDiscovery);
    REQUIRE(error.assetPath() == QDir::cleanPath(obstructionPath));
    REQUIRE(error.diagnostic().contains(QDir::cleanPath(obstructionPath)));
  }

  REQUIRE(QFileInfo::exists(rootPath));
}

TEST_CASE(
    "Archive Recovery wraps an invalid owned history with its exact path") {
  QTemporaryDir temporary;
  REQUIRE(temporary.isValid());
  const QString modPath = QDir(temporary.path()).filePath("Alpha");
  REQUIRE(QDir().mkpath(modPath));
  auto durability = std::make_shared<QtArchiveFileOperations>();
  auto workspace = createWorkspace(modPath, FirstTransactionId, durability);
  QFile journal(QDir(workspace.path()).filePath("journal.log"));
  REQUIRE(journal.open(QIODevice::WriteOnly | QIODevice::Truncate));
  REQUIRE(journal.write("unknown journal") > 0);
  journal.close();

  try {
    static_cast<void>(ArchiveRecovery::recover({modPath}, false, *durability));
    FAIL("Expected invalid recovery history to fail");
  } catch (const ArchiveExecutionError &error) {
    REQUIRE(error.operation() == ArchiveOperation::Recovery);
    REQUIRE(error.stage() == ArchiveFailureStage::RecoveryValidation);
    REQUIRE(error.assetPath() == workspace.path());
    REQUIRE(error.diagnostic().contains(workspace.path()));
  }

  REQUIRE(QFileInfo::exists(workspace.path()));
}

TEST_CASE("Archive Recovery reports the exact legacy workspace path") {
  QTemporaryDir temporary;
  REQUIRE(temporary.isValid());
  const QString modPath = QDir(temporary.path()).filePath("Alpha");
  const QString legacyPath =
      QDir(
          QDir(modPath).filePath(ArchiveTransactionWorkspace::ReservedRootName))
          .filePath("legacy-workspace");
  REQUIRE(QDir().mkpath(legacyPath));
  auto files = std::make_shared<QtArchiveFileOperations>();

  try {
    static_cast<void>(ArchiveRecovery::recover({modPath}, false, *files));
    FAIL("Expected a legacy workspace to require manual resolution");
  } catch (const ArchiveExecutionError &error) {
    REQUIRE(error.operation() == ArchiveOperation::Recovery);
    REQUIRE(error.stage() == ArchiveFailureStage::RecoveryValidation);
    REQUIRE(error.assetPath() == QDir::cleanPath(legacyPath));
    REQUIRE(error.workspacePaths() == QStringList{QDir::cleanPath(legacyPath)});
  }

  REQUIRE(QFileInfo::exists(legacyPath));
}

TEST_CASE("Archive Recovery rejects a changed Mod identity before mutation") {
  QTemporaryDir temporary;
  REQUIRE(temporary.isValid());
  const QString modPath = QDir(temporary.path()).filePath("Alpha");
  REQUIRE(QDir().mkpath(modPath));
  auto files = std::make_shared<QtArchiveFileOperations>();
  auto workspace = createWorkspace(modPath, FirstTransactionId, files);
  commitWorkspace(workspace);
  const QString manifestPath = QDir(workspace.path()).filePath("manifest.json");
  QFile manifestFile(manifestPath);
  REQUIRE(manifestFile.open(QIODevice::ReadOnly));
  QByteArray manifest = manifestFile.readAll();
  manifestFile.close();
  const QByteArray currentIdentity = files->identity(modPath).toUtf8();
  REQUIRE(manifest.contains(currentIdentity));
  manifest.replace(currentIdentity, "replaced-mod-identity");
  REQUIRE(manifestFile.open(QIODevice::WriteOnly | QIODevice::Truncate));
  REQUIRE(manifestFile.write(manifest) == manifest.size());
  manifestFile.close();

  try {
    static_cast<void>(ArchiveRecovery::recover({modPath}, false, *files));
    FAIL("Expected changed Mod ownership to fail closed");
  } catch (const ArchiveExecutionError &error) {
    REQUIRE(error.operation() == ArchiveOperation::Recovery);
    REQUIRE(error.stage() == ArchiveFailureStage::RecoveryValidation);
    REQUIRE(error.assetPath() == workspace.path());
    REQUIRE(error.workspacePaths() == QStringList{workspace.path()});
    REQUIRE(error.diagnostic().contains("identity has changed"));
  }

  REQUIRE(QFileInfo::exists(workspace.path()));
}
