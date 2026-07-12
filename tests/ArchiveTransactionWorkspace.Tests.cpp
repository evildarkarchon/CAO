#include "ArchiveTransactionWorkspace.h"

#include "ArchiveFileOperations.h"

#include <catch2/catch_test_macros.hpp>

#include <QDir>
#include <QFileInfo>
#include <QHash>
#include <QSet>
#include <QTemporaryDir>

#include <algorithm>
#include <memory>
#include <stdexcept>

#ifdef Q_OS_WIN
#include <Windows.h>
#endif

namespace {
QString normalized(const QString &path) { return QDir::cleanPath(path); }

class MemoryDurability final : public ArchiveTransactionDurability {
public:
  QHash<QString, QByteArray> files;
  QSet<QString> directories;
  QStringList durableEvents;

  bool exists(const QString &path) const override {
    return files.contains(normalized(path)) ||
           directories.contains(normalized(path));
  }

  void createDirectory(const QString &path) override {
    const QString clean = normalized(path);
    directories.insert(clean);
    durableEvents.push_back("mkdir:" + clean);
  }

  QString identity(const QString &path) const override {
    const QString clean = normalized(path);
    if (!exists(clean))
      throw std::runtime_error("entry does not exist");
    return "memory:" + clean;
  }

  void writeNewDurably(const QString &path,
                       const QByteArray &contents) override {
    const QString clean = normalized(path);
    if (files.contains(clean))
      throw std::runtime_error("file already exists");
    files.insert(clean, contents);
    durableEvents.push_back("write:" + clean);
  }

  void appendDurably(const QString &path, const QByteArray &contents) override {
    const QString clean = normalized(path);
    if (!files.contains(clean))
      throw std::runtime_error("file does not exist");
    files[clean].append(contents);
    durableEvents.push_back("append:" + clean);
  }

  QByteArray readAll(const QString &path) const override {
    const QString clean = normalized(path);
    if (!files.contains(clean))
      throw std::runtime_error("file does not exist");
    return files.value(clean);
  }

  void removeTree(const QString &path) override {
    const QString clean = normalized(path);
    const QString prefix = clean + "/";
    for (auto it = files.begin(); it != files.end();) {
      if (it.key() == clean || it.key().startsWith(prefix))
        it = files.erase(it);
      else
        ++it;
    }
    for (auto it = directories.begin(); it != directories.end();) {
      if (*it == clean || it->startsWith(prefix))
        it = directories.erase(it);
      else
        ++it;
    }
    durableEvents.push_back("remove-tree:" + clean);
  }

  void removeEmptyDirectory(const QString &path) override {
    const QString clean = normalized(path);
    const QString prefix = clean + "/";
    const bool hasFile =
        std::any_of(files.keyBegin(), files.keyEnd(), [&](const QString &file) {
          return file.startsWith(prefix);
        });
    const bool hasDirectory =
        std::any_of(directories.cbegin(), directories.cend(),
                    [&](const QString &directory) {
                      return directory != clean && directory.startsWith(prefix);
                    });
    if (!hasFile && !hasDirectory)
      directories.remove(clean);
    durableEvents.push_back("rmdir:" + clean);
  }
};

ArchiveTransactionManifest manifest() {
  ArchiveTransactionManifest value;
  value.transactionId = "5e30ea4c-90a7-4af7-8381-67ecb234f4df";
  value.kind = ArchiveTransactionKind::Packing;
  value.canonicalModPath = "C:/Mods/Alpha";
  value.modIdentity = "volume-2:file-40";
  value.canonicalAnchorPath = "C:/Mods/Alpha";
  value.anchorIdentity = "volume-2:file-40";
  value.volumeIdentity = "volume-2";
  value.policyFacts.insert("deleteSource", "true");
  return value;
}

ArchiveTransactionManifest manifestForDisk(const QString &modPath,
                                           ArchiveFileOperations &files) {
  ArchiveTransactionManifest value;
  value.transactionId = "5e30ea4c-90a7-4af7-8381-67ecb234f4df";
  value.kind = ArchiveTransactionKind::Packing;
  value.canonicalModPath = QFileInfo(modPath).absoluteFilePath();
  value.modIdentity = files.identity(value.canonicalModPath);
  value.canonicalAnchorPath = value.canonicalModPath;
  value.anchorIdentity = value.modIdentity;
  value.volumeIdentity = value.modIdentity;
  return value;
}

void writeFile(const QString &path, const QByteArray &contents) {
  REQUIRE(QDir().mkpath(QFileInfo(path).absolutePath()));
  QFile file(path);
  REQUIRE(file.open(QIODevice::WriteOnly | QIODevice::NewOnly));
  REQUIRE(file.write(contents) == contents.size());
}
} // namespace

TEST_CASE("Archive Transaction workspace is owned inside its Mod") {
  auto durability = std::make_shared<MemoryDurability>();
  const auto workspace =
      ArchiveTransactionWorkspace::create(manifest(), durability);

  REQUIRE(workspace.path() == "C:/Mods/Alpha/.cao-transactions/"
                              "5e30ea4c-90a7-4af7-8381-67ecb234f4df");
  REQUIRE(workspace.manifest().policyFacts.value("deleteSource") == "true");
  REQUIRE(workspace.manifest().workspaceIdentity ==
          "memory:" + workspace.path());
  REQUIRE(durability->files.contains(workspace.path() + "/manifest.json"));
  REQUIRE(durability->files.contains(workspace.path() + "/journal.log"));
}

TEST_CASE(
    "Archive Transaction journal reopens committed human-readable records") {
  auto durability = std::make_shared<MemoryDurability>();
  auto workspace = ArchiveTransactionWorkspace::create(manifest(), durability);
  REQUIRE(workspace.append(ArchiveTransactionRecordKind::Intent,
                           {{"source", "textures/a.dds"},
                            {"destination", "rollback/a.dds"}}) == 1);
  REQUIRE(workspace.append(ArchiveTransactionRecordKind::MutationComplete,
                           {{"destination", "rollback/a.dds"}}) == 2);
  workspace.commit({{"publishedOutputs", "1"}});

  const QByteArray journal =
      durability->files.value(workspace.path() + "/journal.log");
  REQUIRE(journal.contains("sequence=1"));
  REQUIRE(journal.contains("checksum="));
  REQUIRE(journal.contains("textures/a.dds"));

  const auto reopened =
      ArchiveTransactionWorkspace::reopen(workspace.path(), durability);
  REQUIRE(reopened.records().size() == 3);
  REQUIRE(reopened.records().front().fields.value("source") ==
          "textures/a.dds");
  REQUIRE(reopened.isCommitted());
}

TEST_CASE("Archive Transaction journal ignores only a torn final frame") {
  auto durability = std::make_shared<MemoryDurability>();
  auto workspace = ArchiveTransactionWorkspace::create(manifest(), durability);
  workspace.append(ArchiveTransactionRecordKind::Intent,
                   {{"source", "Alpha.bsa"}});
  const QString journalPath = workspace.path() + "/journal.log";
  durability->files[journalPath].append("@frame sequence=2 length=100");

  const auto reopened =
      ArchiveTransactionWorkspace::reopen(workspace.path(), durability);
  REQUIRE(reopened.records().size() == 1);
  REQUIRE_FALSE(reopened.isCommitted());
}

TEST_CASE(
    "Archive Transaction journal rejects corruption before a later frame") {
  auto durability = std::make_shared<MemoryDurability>();
  auto workspace = ArchiveTransactionWorkspace::create(manifest(), durability);
  workspace.append(ArchiveTransactionRecordKind::Intent,
                   {{"source", "Alpha.bsa"}});
  workspace.append(ArchiveTransactionRecordKind::MutationComplete,
                   {{"destination", "rollback/Alpha.bsa"}});
  const QString journalPath = workspace.path() + "/journal.log";
  QByteArray &journal = durability->files[journalPath];
  const qsizetype payload = journal.indexOf("{\"fields\"");
  REQUIRE(payload >= 0);
  const int corruptByte = static_cast<int>(payload + 3);
  journal[corruptByte] = journal[corruptByte] == 'x' ? 'y' : 'x';

  REQUIRE_THROWS_AS(
      ArchiveTransactionWorkspace::reopen(workspace.path(), durability),
      std::runtime_error);
}

TEST_CASE("Archive Transaction workspace rejects unknown manifest versions") {
  auto durability = std::make_shared<MemoryDurability>();
  const auto workspace =
      ArchiveTransactionWorkspace::create(manifest(), durability);
  QByteArray &storedManifest =
      durability->files[workspace.path() + "/manifest.json"];
  REQUIRE(storedManifest.contains("\"schemaVersion\": 1"));
  storedManifest.replace("\"schemaVersion\": 1", "\"schemaVersion\": 2");

  REQUIRE_THROWS_AS(
      ArchiveTransactionWorkspace::reopen(workspace.path(), durability),
      std::runtime_error);
}

TEST_CASE("Archive Transaction commit is durable and cannot be duplicated") {
  auto durability = std::make_shared<MemoryDurability>();
  auto workspace = ArchiveTransactionWorkspace::create(manifest(), durability);
  REQUIRE_THROWS_AS(workspace.commit(), std::runtime_error);
  workspace.append(ArchiveTransactionRecordKind::Intent);
  workspace.append(ArchiveTransactionRecordKind::MutationComplete);
  workspace.commit();

  REQUIRE(workspace.isCommitted());
  REQUIRE_THROWS_AS(workspace.commit(), std::runtime_error);
  REQUIRE_THROWS_AS(workspace.append(ArchiveTransactionRecordKind::Intent),
                    std::runtime_error);
}

TEST_CASE("Archive Transaction workspace cleanup is idempotent") {
  auto durability = std::make_shared<MemoryDurability>();
  auto workspace = ArchiveTransactionWorkspace::create(manifest(), durability);
  const QString workspacePath = workspace.path();
  const QString rootPath = QFileInfo(workspacePath).absolutePath();

  workspace.cleanup();
  workspace.cleanup();

  REQUIRE_FALSE(durability->exists(workspacePath));
  REQUIRE_FALSE(durability->exists(rootPath));
}

TEST_CASE("Archive Transaction workspace preserves incomplete rollback truth") {
  auto durability = std::make_shared<MemoryDurability>();
  auto workspace = ArchiveTransactionWorkspace::create(manifest(), durability);
  workspace.append(ArchiveTransactionRecordKind::Intent,
                   {{"source", "Alpha.bsa"}});

  REQUIRE_THROWS_AS(workspace.cleanup(), std::runtime_error);
  REQUIRE(durability->exists(workspace.path()));
}

TEST_CASE("Archive publication never overwrites an existing Asset") {
  QTemporaryDir temporary;
  REQUIRE(temporary.isValid());
  const QString source = QDir(temporary.path()).filePath("source.bsa");
  const QString destination =
      QDir(temporary.path()).filePath("destination.bsa");
  writeFile(source, "new archive");
  writeFile(destination, "existing archive");
  QtArchiveFileOperations files;

  REQUIRE_THROWS_AS(files.move(source, destination), std::runtime_error);

  QFile unchanged(destination);
  REQUIRE(unchanged.open(QIODevice::ReadOnly));
  REQUIRE(unchanged.readAll() == QByteArray("existing archive"));
  REQUIRE(QFileInfo::exists(source));
}

TEST_CASE("Archive Transaction replay rejects a path outside its Mod before "
          "restoring any Asset") {
  QTemporaryDir temporary;
  REQUIRE(temporary.isValid());
  const QString modPath = QDir(temporary.path()).filePath("Alpha");
  REQUIRE(QDir().mkpath(modPath));
  auto files = std::make_shared<QtArchiveFileOperations>();
  auto workspace = ArchiveTransactionWorkspace::create(
      manifestForDisk(modPath, *files), files);

  const QString liveArchive = QDir(modPath).filePath("Alpha.bsa");
  const QString rollbackArchive =
      QDir(workspace.path()).filePath("rollback/Alpha.bsa");
  writeFile(rollbackArchive, "rollback archive");
  workspace.append(ArchiveTransactionRecordKind::Intent,
                   {{"operation", "move"},
                    {"source", liveArchive},
                    {"destination", rollbackArchive}});
  const QString escapedSource =
      QDir(QFileInfo(modPath).absolutePath()).filePath("outside.txt");
  writeFile(escapedSource, "outside");
  workspace.append(ArchiveTransactionRecordKind::Intent,
                   {{"operation", "move"},
                    {"source", escapedSource},
                    {"destination",
                     QDir(workspace.path()).filePath("rollback/outside.txt")}});

  const QStringList failures = workspace.replay(*files);

  REQUIRE_FALSE(failures.isEmpty());
  REQUIRE_FALSE(QFileInfo::exists(liveArchive));
  REQUIRE(QFileInfo::exists(rollbackArchive));
  REQUIRE(QFileInfo::exists(escapedSource));
}

TEST_CASE(
    "Archive Transaction replay rejects a replaced move source identity") {
  QTemporaryDir temporary;
  REQUIRE(temporary.isValid());
  const QString modPath = QDir(temporary.path()).filePath("Alpha");
  REQUIRE(QDir().mkpath(modPath));
  auto files = std::make_shared<QtArchiveFileOperations>();
  auto workspace = ArchiveTransactionWorkspace::create(
      manifestForDisk(modPath, *files), files);
  const QString source = QDir(modPath).filePath("Alpha.bsa");
  const QString destination =
      QDir(workspace.path()).filePath("rollback/Alpha.bsa");
  writeFile(source, "original archive");
  workspace.append(ArchiveTransactionRecordKind::Intent,
                   {{"operation", "move"},
                    {"source", source},
                    {"destination", destination}});
  const QString retainedOriginal =
      QDir(modPath).filePath("retained-original.bsa");
  files->move(source, retainedOriginal);
  writeFile(source, "replacement archive");

  const QStringList failures = workspace.replay(*files);

  REQUIRE(failures.join('\n').contains("identity has changed"));
  REQUIRE(QFileInfo::exists(source));
  REQUIRE(QFileInfo::exists(retainedOriginal));
  REQUIRE_FALSE(QFileInfo::exists(destination));
}

#ifdef Q_OS_WIN
TEST_CASE("Archive Transaction rejects a reparse-point reserved root") {
  QTemporaryDir temporary;
  REQUIRE(temporary.isValid());
  const QString modPath = QDir(temporary.path()).filePath("Alpha");
  const QString redirectPath = QDir(temporary.path()).filePath("redirect");
  REQUIRE(QDir().mkpath(modPath));
  REQUIRE(QDir().mkpath(redirectPath));
  const QString rootPath =
      QDir(modPath).filePath(ArchiveTransactionWorkspace::ReservedRootName);
  if (!CreateSymbolicLinkW(reinterpret_cast<LPCWSTR>(rootPath.utf16()),
                           reinterpret_cast<LPCWSTR>(redirectPath.utf16()),
                           SYMBOLIC_LINK_FLAG_DIRECTORY |
                               SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE)) {
    SUCCEED("Symbolic-link creation is unavailable on this Windows host");
    return;
  }
  auto files = std::make_shared<QtArchiveFileOperations>();

  REQUIRE_THROWS_AS(ArchiveTransactionWorkspace::create(
                        manifestForDisk(modPath, *files), files),
                    std::runtime_error);
  REQUIRE(QDir(redirectPath)
              .entryList(QDir::AllEntries | QDir::NoDotAndDotDot)
              .isEmpty());
}

TEST_CASE("Archive Transaction replay validates a missing destination through "
          "its nearest existing ancestor") {
  QTemporaryDir temporary;
  REQUIRE(temporary.isValid());
  const QString modPath = QDir(temporary.path()).filePath("Alpha");
  const QString outsidePath = QDir(temporary.path()).filePath("outside");
  REQUIRE(QDir().mkpath(modPath));
  REQUIRE(QDir().mkpath(outsidePath));
  auto files = std::make_shared<QtArchiveFileOperations>();
  auto workspace = ArchiveTransactionWorkspace::create(
      manifestForDisk(modPath, *files), files);
  const QString redirectPath = QDir(modPath).filePath("redirect");
  if (!CreateSymbolicLinkW(reinterpret_cast<LPCWSTR>(redirectPath.utf16()),
                           reinterpret_cast<LPCWSTR>(outsidePath.utf16()),
                           SYMBOLIC_LINK_FLAG_DIRECTORY |
                               SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE)) {
    SUCCEED("Symbolic-link creation is unavailable on this Windows host");
    return;
  }
  const QString rollbackArchive =
      QDir(workspace.path()).filePath("rollback/Alpha.bsa");
  writeFile(rollbackArchive, "rollback archive");
  workspace.append(ArchiveTransactionRecordKind::Intent,
                   {{"operation", "move"},
                    {"source", QDir(redirectPath).filePath("new/Alpha.bsa")},
                    {"destination", rollbackArchive}});

  const QStringList failures = workspace.replay(*files);

  REQUIRE_FALSE(failures.isEmpty());
  REQUIRE(QFileInfo::exists(rollbackArchive));
  REQUIRE_FALSE(QFileInfo::exists(QDir(outsidePath).filePath("new/Alpha.bsa")));
}
#endif
