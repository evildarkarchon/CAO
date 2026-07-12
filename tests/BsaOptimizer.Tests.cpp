#include "BsaOptimizer.h"

#include "AssetPathVisibility.h"

#include <catch2/catch_test_macros.hpp>

#include <QDir>
#include <QFileInfo>
#include <QHash>
#include <QSet>

#include <optional>
#include <stdexcept>

namespace {
QString normalized(const QString &path) { return QDir::cleanPath(path); }

class MemoryArchiveFiles final : public ArchiveFileOperations {
public:
  QSet<QString> files;
  QSet<QString> directories;
  QHash<QString, QByteArray> durableFiles;
  QHash<QString, QString> fileIdentities;
  QStringList events;
  int moveCount = 0;
  int removeCount = 0;
  int appendCount = 0;
  std::optional<int> failMove;
  std::optional<int> failRemove;
  std::optional<int> failAppend;
  bool failWrite = false;
  int nextFileIdentity = 0;

  void addFile(const QString &path) {
    const QString clean = normalized(path);
    files.insert(clean);
    if (!fileIdentities.contains(clean))
      fileIdentities.insert(clean,
                            QString("memory-file:%1").arg(nextFileIdentity++));
    QString parent = QFileInfo(normalized(path)).absolutePath();
    while (!parent.isEmpty() && !directories.contains(parent)) {
      directories.insert(parent);
      const QString next = QFileInfo(parent).absolutePath();
      if (next == parent)
        break;
      parent = next;
    }
  }

  QVector<ArchiveFileEntry>
  listRecursively(const QString &root) const override {
    QVector<ArchiveFileEntry> result;
    const QString prefix = normalized(root) + "/";
    for (const QString &file : files) {
      if (file.startsWith(prefix, Qt::CaseInsensitive))
        result.push_back({file, file.mid(prefix.size()), true, false});
    }
    return result;
  }

  bool exists(const QString &path) const override {
    return files.contains(normalized(path)) ||
           directories.contains(normalized(path)) ||
           durableFiles.contains(normalized(path));
  }

  void createDirectory(const QString &path) override {
    directories.insert(normalized(path));
  }

  QString identity(const QString &path) const override {
    if (!exists(path))
      throw std::runtime_error("identity requested for absent path");
    const QString clean = normalized(path);
    if (fileIdentities.contains(clean))
      return fileIdentities.value(clean);
    return QStringLiteral("memory:") + normalized(path).toLower();
  }

  void writeNewDurably(const QString &path,
                       const QByteArray &contents) override {
    if (failWrite)
      throw std::runtime_error("injected durable write failure");
    const QString clean = normalized(path);
    if (exists(clean))
      throw std::runtime_error("durable file already exists");
    durableFiles.insert(clean, contents);
  }

  void appendDurably(const QString &path, const QByteArray &contents) override {
    ++appendCount;
    if (failAppend == appendCount)
      throw std::runtime_error("injected journal append failure");
    const QString clean = normalized(path);
    if (!durableFiles.contains(clean))
      throw std::runtime_error("durable file is absent");
    durableFiles[clean].append(contents);
    events << "journal-append:" + clean;
  }

  QByteArray readAll(const QString &path) const override {
    const QString clean = normalized(path);
    if (!durableFiles.contains(clean))
      throw std::runtime_error("durable file is absent");
    return durableFiles.value(clean);
  }

  void flushFileDurably(const QString &path) override {
    if (!files.contains(normalized(path)))
      throw std::runtime_error("flush requested for absent staged file");
    events << "flush:" + normalized(path);
  }

  void createDirectories(const QString &path) override {
    directories.insert(normalized(path));
    events << "mkdir:" + normalized(path);
  }

  void move(const QString &source, const QString &destination) override {
    ++moveCount;
    events << QString("move:%1>%2")
                  .arg(normalized(source), normalized(destination));
    if (failMove == moveCount)
      throw std::runtime_error("injected move failure");
    const QString cleanSource = normalized(source);
    const QString cleanDestination = normalized(destination);
    if (!files.remove(cleanSource) || exists(cleanDestination))
      throw std::runtime_error("invalid virtual move");
    files.insert(cleanDestination);
    fileIdentities.insert(cleanDestination, fileIdentities.take(cleanSource));
  }

  void removeFile(const QString &path) override {
    ++removeCount;
    events << "remove:" + normalized(path);
    if (failRemove == removeCount)
      throw std::runtime_error("injected remove failure");
    const QString clean = normalized(path);
    files.remove(clean);
    fileIdentities.remove(clean);
  }

  void removeEmptyDirectory(const QString &path) override {
    const QString prefix = normalized(path) + "/";
    const bool occupied =
        std::any_of(files.begin(), files.end(), [&](const QString &file) {
          return file.startsWith(prefix, Qt::CaseInsensitive);
        });
    if (occupied)
      throw std::runtime_error("directory is not empty");
    directories.remove(normalized(path));
    events << "rmdir:" + normalized(path);
  }

  void removeTree(const QString &path) override {
    const QString root = normalized(path);
    const QString prefix = root + "/";
    for (auto it = files.begin(); it != files.end();) {
      if (*it == root || it->startsWith(prefix, Qt::CaseInsensitive)) {
        fileIdentities.remove(*it);
        it = files.erase(it);
      } else
        ++it;
    }
    for (auto it = directories.begin(); it != directories.end();) {
      if (*it == root || it->startsWith(prefix, Qt::CaseInsensitive))
        it = directories.erase(it);
      else
        ++it;
    }
    for (auto it = durableFiles.begin(); it != durableFiles.end();) {
      if (it.key() == root || it.key().startsWith(prefix, Qt::CaseInsensitive))
        it = durableFiles.erase(it);
      else
        ++it;
    }
    events << "remove-tree:" + root;
  }
};

class RecordedBootstrap final : public ArchiveTransactionBootstrap {
public:
  explicit RecordedBootstrap(MemoryArchiveFiles &files) : files(files) {}

  void begin(const QString &, const QString &,
             const QString &workspacePath) override {
    ++beginCount;
    rootExistedAtBegin = files.exists(QFileInfo(workspacePath).absolutePath());
  }

  void complete(const QString &, const QString &) override {
    ++completeCount;
    manifestExistedAtComplete = std::any_of(
        files.durableFiles.keyBegin(), files.durableFiles.keyEnd(),
        [](const QString &path) { return path.endsWith("/manifest.json"); });
  }

  MemoryArchiveFiles &files;
  int beginCount = 0;
  int completeCount = 0;
  bool rootExistedAtBegin = false;
  bool manifestExistedAtComplete = false;
};

class RecordedArchiveEngine final : public ArchiveEngine {
public:
  explicit RecordedArchiveEngine(MemoryArchiveFiles &files) : files(files) {}

  MemoryArchiveFiles &files;
  int extractCalls = 0;
  int packCalls = 0;
  bool failExtraction = false;
  QStringList extractedRelativeFiles;
  QVector<QPair<QString, StagedArchiveOutputKind>> packedOutputs;
  QStringList packedSources;
  QString lastExtractionStaging;
  QString lastPackingStaging;

  void extractTo(const QString &, const QString &stagingRoot) override {
    ++extractCalls;
    lastExtractionStaging = stagingRoot;
    if (failExtraction)
      throw std::runtime_error("injected engine failure");
    for (const QString &relative : extractedRelativeFiles)
      files.addFile(QDir(stagingRoot).filePath(relative));
  }

  StagedArchivePacking packTo(const QString &, const QString &stagingRoot,
                              const ArchiveExecutionPolicy &) override {
    ++packCalls;
    lastPackingStaging = stagingRoot;
    StagedArchivePacking result;
    for (const auto &[relative, kind] : packedOutputs) {
      const QString path = QDir(stagingRoot).filePath(relative);
      files.addFile(path);
      result.outputs.push_back({path, relative, kind});
    }
    result.packedSourceAssets = packedSources;
    return result;
  }
};

ArchiveExecutionPolicy packingPolicy(const bool deleteSource = true) {
  ArchiveExecutionPolicy policy;
  policy.deleteSource = deleteSource;
  policy.createDummies = false;
  return policy;
}
} // namespace

TEST_CASE("BSA extraction Dry Run performs no staging or engine work") {
  MemoryArchiveFiles files;
  RecordedArchiveEngine engine(files);
  BSAOptimizer optimizer(engine, files);
  optimizer.extract("C:/Mods/Alpha/Alpha.bsa", false, true);
  REQUIRE(engine.extractCalls == 0);
  REQUIRE(files.events.isEmpty());
}

TEST_CASE("BSA workspace creation is bracketed by durable lock bootstrap") {
  MemoryArchiveFiles files;
  files.addFile("C:/Mods/Alpha/Alpha.bsa");
  RecordedArchiveEngine engine(files);
  engine.extractedRelativeFiles = QStringList({QStringLiteral("a.dds")});
  RecordedBootstrap bootstrap(files);
  BSAOptimizer optimizer(engine, files, bootstrap);

  optimizer.extract("C:/Mods/Alpha/Alpha.bsa", false);

  REQUIRE(bootstrap.beginCount == 1);
  REQUIRE_FALSE(bootstrap.rootExistedAtBegin);
  REQUIRE(bootstrap.completeCount == 1);
  REQUIRE(bootstrap.manifestExistedAtComplete);
}

TEST_CASE("BSA preserves bootstrap intent when manifest creation fails") {
  MemoryArchiveFiles files;
  files.addFile("C:/Mods/Alpha/Alpha.bsa");
  files.failWrite = true;
  RecordedArchiveEngine engine(files);
  RecordedBootstrap bootstrap(files);
  BSAOptimizer optimizer(engine, files, bootstrap);

  REQUIRE_THROWS_AS(optimizer.extract("C:/Mods/Alpha/Alpha.bsa", false),
                    ArchiveExecutionError);
  REQUIRE(bootstrap.beginCount == 1);
  REQUIRE(bootstrap.completeCount == 0);
}

TEST_CASE("BSA extraction publishes staged Assets and retains the archive") {
  MemoryArchiveFiles files;
  files.addFile("C:/Mods/Alpha/Alpha.bsa");
  RecordedArchiveEngine engine(files);
  engine.extractedRelativeFiles = QStringList(
      {QStringLiteral("textures/a.dds"), QStringLiteral("meshes/a.nif")});
  BSAOptimizer optimizer(engine, files);
  optimizer.extract("C:/Mods/Alpha/Alpha.bsa", false);
  REQUIRE(files.exists("C:/Mods/Alpha/textures/a.dds"));
  REQUIRE(files.exists("C:/Mods/Alpha/meshes/a.nif"));
  REQUIRE_FALSE(files.exists("C:/Mods/Alpha/Alpha.bsa"));
  REQUIRE(files.exists("C:/Mods/Alpha/Alpha.bsa.bak"));
  REQUIRE(AssetPathVisibility::isInternalPath(engine.lastExtractionStaging));
  REQUIRE_FALSE(files.exists("C:/Mods/Alpha/.cao-transactions"));
  for (qsizetype index = 1; index < files.events.size(); ++index) {
    if (files.events[index].startsWith("move:"))
      REQUIRE(files.events[index - 1].startsWith("journal-append:"));
  }
}

TEST_CASE("BSA extraction preserves loose Asset collisions") {
  MemoryArchiveFiles files;
  files.addFile("C:/Mods/Alpha/Alpha.bsa");
  files.addFile("C:/Mods/Alpha/textures/a.dds");
  RecordedArchiveEngine engine(files);
  engine.extractedRelativeFiles = QStringList(
      {QStringLiteral("textures/a.dds"), QStringLiteral("textures/b.dds")});
  BSAOptimizer optimizer(engine, files);
  optimizer.extract("C:/Mods/Alpha/Alpha.bsa", true);
  REQUIRE(files.exists("C:/Mods/Alpha/textures/a.dds"));
  REQUIRE(files.exists("C:/Mods/Alpha/textures/b.dds"));
}

TEST_CASE("BSA extraction rolls back published Assets on publish failure") {
  MemoryArchiveFiles files;
  files.addFile("C:/Mods/Alpha/Alpha.bsa");
  RecordedArchiveEngine engine(files);
  engine.extractedRelativeFiles =
      QStringList({QStringLiteral("a.dds"), QStringLiteral("b.dds")});
  files.failMove = 2;
  BSAOptimizer optimizer(engine, files);
  REQUIRE_THROWS_AS(optimizer.extract("C:/Mods/Alpha/Alpha.bsa", false),
                    ArchiveExecutionError);
  REQUIRE(files.exists("C:/Mods/Alpha/Alpha.bsa"));
  REQUIRE_FALSE(files.exists("C:/Mods/Alpha/a.dds"));
  REQUIRE_FALSE(files.exists("C:/Mods/Alpha/b.dds"));
}

TEST_CASE("BSA extraction replays an intent when completion journaling fails") {
  MemoryArchiveFiles files;
  files.addFile("C:/Mods/Alpha/Alpha.bsa");
  RecordedArchiveEngine engine(files);
  engine.extractedRelativeFiles = QStringList({QStringLiteral("a.dds")});
  // The second append is the completion record after the staged Asset moved.
  files.failAppend = 2;
  BSAOptimizer optimizer(engine, files);

  REQUIRE_THROWS_AS(optimizer.extract("C:/Mods/Alpha/Alpha.bsa", false),
                    ArchiveExecutionError);
  REQUIRE(files.exists("C:/Mods/Alpha/Alpha.bsa"));
  REQUIRE_FALSE(files.exists("C:/Mods/Alpha/a.dds"));
  REQUIRE_FALSE(files.exists("C:/Mods/Alpha/.cao-transactions"));
}

TEST_CASE("BSA extraction restores exact pre-state after every journal append "
          "failure") {
  for (int failedAppend = 1; failedAppend <= 7; ++failedAppend) {
    INFO("failed append " << failedAppend);
    MemoryArchiveFiles files;
    files.addFile("C:/Mods/Alpha/Alpha.bsa");
    const QString originalIdentity = files.identity("C:/Mods/Alpha/Alpha.bsa");
    RecordedArchiveEngine engine(files);
    engine.extractedRelativeFiles =
        QStringList({QStringLiteral("a.dds"), QStringLiteral("b.dds")});
    files.failAppend = failedAppend;
    BSAOptimizer optimizer(engine, files);

    REQUIRE_THROWS_AS(optimizer.extract("C:/Mods/Alpha/Alpha.bsa", false),
                      ArchiveExecutionError);
    REQUIRE(files.exists("C:/Mods/Alpha/Alpha.bsa"));
    REQUIRE(files.identity("C:/Mods/Alpha/Alpha.bsa") == originalIdentity);
    REQUIRE_FALSE(files.exists("C:/Mods/Alpha/a.dds"));
    REQUIRE_FALSE(files.exists("C:/Mods/Alpha/b.dds"));
    REQUIRE_FALSE(files.exists("C:/Mods/Alpha/.cao-transactions"));
  }
}

TEST_CASE("BSA extraction restores exact pre-state after every move failure") {
  for (int failedMove = 1; failedMove <= 3; ++failedMove) {
    INFO("failed move " << failedMove);
    MemoryArchiveFiles files;
    files.addFile("C:/Mods/Alpha/Alpha.bsa");
    const QString originalIdentity = files.identity("C:/Mods/Alpha/Alpha.bsa");
    RecordedArchiveEngine engine(files);
    engine.extractedRelativeFiles =
        QStringList({QStringLiteral("a.dds"), QStringLiteral("b.dds")});
    files.failMove = failedMove;
    BSAOptimizer optimizer(engine, files);

    REQUIRE_THROWS_AS(optimizer.extract("C:/Mods/Alpha/Alpha.bsa", false),
                      ArchiveExecutionError);
    REQUIRE(files.exists("C:/Mods/Alpha/Alpha.bsa"));
    REQUIRE(files.identity("C:/Mods/Alpha/Alpha.bsa") == originalIdentity);
    REQUIRE_FALSE(files.exists("C:/Mods/Alpha/a.dds"));
    REQUIRE_FALSE(files.exists("C:/Mods/Alpha/b.dds"));
    REQUIRE_FALSE(files.exists("C:/Mods/Alpha/.cao-transactions"));
  }
}

TEST_CASE("BSA extraction never replaces an existing retained backup") {
  MemoryArchiveFiles files;
  files.addFile("C:/Mods/Alpha/Alpha.bsa");
  files.addFile("C:/Mods/Alpha/Alpha.bsa.bak");
  RecordedArchiveEngine engine(files);
  engine.extractedRelativeFiles = QStringList({QStringLiteral("a.dds")});
  BSAOptimizer optimizer(engine, files);
  optimizer.extract("C:/Mods/Alpha/Alpha.bsa", false);
  REQUIRE(files.exists("C:/Mods/Alpha/Alpha.bsa.bak"));
  REQUIRE(files.exists("C:/Mods/Alpha/Alpha.bsa.bak.1"));
}

TEST_CASE("BSA packing Dry Run performs no staging or engine work") {
  MemoryArchiveFiles files;
  files.directories.insert("C:/Mods/Alpha");
  RecordedArchiveEngine engine(files);
  BSAOptimizer optimizer(engine, files);
  optimizer.packAll("C:/Mods/Alpha", packingPolicy(), true);
  REQUIRE(engine.packCalls == 0);
}

TEST_CASE("BSA packing commits output before deleting loose sources") {
  MemoryArchiveFiles files;
  files.directories.insert("C:/Mods/Alpha");
  files.addFile("C:/Mods/Alpha/textures/a.dds");
  RecordedArchiveEngine engine(files);
  engine.packedOutputs = {{"Alpha.bsa", StagedArchiveOutputKind::Archive}};
  engine.packedSources =
      QStringList({QStringLiteral("C:/Mods/Alpha/textures/a.dds")});
  BSAOptimizer optimizer(engine, files);
  optimizer.packAll("C:/Mods/Alpha", packingPolicy(true));
  REQUIRE(files.exists("C:/Mods/Alpha/Alpha.bsa"));
  REQUIRE_FALSE(files.exists("C:/Mods/Alpha/textures/a.dds"));
  const auto publish = std::find_if(
      files.events.begin(), files.events.end(), [](const QString &event) {
        return event.startsWith("move:") &&
               event.endsWith(">C:/Mods/Alpha/Alpha.bsa");
      });
  const auto sourceMove = std::find_if(
      files.events.begin(), files.events.end(), [](const QString &event) {
        return event.startsWith("move:C:/Mods/Alpha/textures/a.dds>") &&
               event.contains("/.cao-transactions/");
      });
  const auto remove = std::find_if(
      files.events.begin(), files.events.end(), [](const QString &event) {
        return event.startsWith("remove:") &&
               event.contains("/rollback/source-0");
      });
  REQUIRE(publish != files.events.end());
  REQUIRE(sourceMove > publish);
  REQUIRE(remove > publish);
  REQUIRE(AssetPathVisibility::isInternalPath(engine.lastPackingStaging));
  REQUIRE_FALSE(files.exists("C:/Mods/Alpha/.cao-transactions"));
}

TEST_CASE("BSA packing removes rollback-only replaced archives after commit") {
  MemoryArchiveFiles files;
  files.directories.insert("C:/Mods/Alpha");
  files.addFile("C:/Mods/Alpha/Alpha.bsa");
  RecordedArchiveEngine engine(files);
  engine.packedOutputs = {{"Alpha.bsa", StagedArchiveOutputKind::Archive}};
  BSAOptimizer optimizer(engine, files);
  optimizer.packAll("C:/Mods/Alpha", packingPolicy(false));
  REQUIRE(files.exists("C:/Mods/Alpha/Alpha.bsa"));
  REQUIRE_FALSE(files.exists("C:/Mods/Alpha/Alpha.bsa.bak"));
}

TEST_CASE("BSA packing restores archives when later publishing fails") {
  MemoryArchiveFiles files;
  files.directories.insert("C:/Mods/Alpha");
  files.addFile("C:/Mods/Alpha/Alpha.bsa");
  files.addFile("C:/Mods/Alpha/Alpha - Textures.bsa");
  RecordedArchiveEngine engine(files);
  engine.packedOutputs = {
      {"Alpha.bsa", StagedArchiveOutputKind::Archive},
      {"Alpha - Textures.bsa", StagedArchiveOutputKind::Archive}};
  files.failMove = 4;
  BSAOptimizer optimizer(engine, files);
  REQUIRE_THROWS_AS(optimizer.packAll("C:/Mods/Alpha", packingPolicy(false)),
                    ArchiveExecutionError);
  REQUIRE(files.exists("C:/Mods/Alpha/Alpha.bsa"));
  REQUIRE(files.exists("C:/Mods/Alpha/Alpha - Textures.bsa"));
  REQUIRE_FALSE(files.exists("C:/Mods/Alpha/Alpha.bsa.bak"));
}

TEST_CASE("BSA packing never replaces a live plugin with a dummy") {
  MemoryArchiveFiles files;
  files.directories.insert("C:/Mods/Alpha");
  files.addFile("C:/Mods/Alpha/Alpha.esp");
  RecordedArchiveEngine engine(files);
  engine.packedOutputs = {{"Alpha.bsa", StagedArchiveOutputKind::Archive},
                          {"Alpha.esp", StagedArchiveOutputKind::DummyPlugin}};
  BSAOptimizer optimizer(engine, files);
  optimizer.packAll("C:/Mods/Alpha", packingPolicy(false));
  REQUIRE(files.exists("C:/Mods/Alpha/Alpha.esp"));
  REQUIRE_FALSE(files.exists("C:/Mods/Alpha/Alpha.esp.bak"));
}

TEST_CASE("BSA packing reports source cleanup failure after commit") {
  MemoryArchiveFiles files;
  files.directories.insert("C:/Mods/Alpha");
  files.addFile("C:/Mods/Alpha/a.dds");
  RecordedArchiveEngine engine(files);
  engine.packedOutputs = {{"Alpha.bsa", StagedArchiveOutputKind::Archive}};
  engine.packedSources = QStringList({QStringLiteral("C:/Mods/Alpha/a.dds")});
  files.failRemove = 1;
  BSAOptimizer optimizer(engine, files);
  try {
    optimizer.packAll("C:/Mods/Alpha", packingPolicy(true));
    FAIL("Expected source cleanup failure");
  } catch (const ArchiveExecutionError &error) {
    REQUIRE(error.operation() == ArchiveOperation::Packing);
    REQUIRE(error.stage() == ArchiveFailureStage::SourceCleanup);
  }
  REQUIRE(files.exists("C:/Mods/Alpha/Alpha.bsa"));
  REQUIRE_FALSE(files.exists("C:/Mods/Alpha/a.dds"));
}

TEST_CASE("BSA packing rejects sources outside the live Mod manifest") {
  MemoryArchiveFiles files;
  files.directories.insert("C:/Mods/Alpha");
  files.addFile("C:/Mods/Alpha/textures/a.dds");
  files.addFile("C:/Users/Public/unrelated.dds");
  RecordedArchiveEngine engine(files);
  engine.packedOutputs = {{"Alpha.bsa", StagedArchiveOutputKind::Archive}};
  engine.packedSources =
      QStringList({QStringLiteral("C:/Users/Public/unrelated.dds")});
  BSAOptimizer optimizer(engine, files);

  REQUIRE_THROWS_AS(optimizer.packAll("C:/Mods/Alpha", packingPolicy(true)),
                    ArchiveExecutionError);
  REQUIRE(files.exists("C:/Users/Public/unrelated.dds"));
  REQUIRE_FALSE(files.exists("C:/Mods/Alpha/Alpha.bsa"));
}
