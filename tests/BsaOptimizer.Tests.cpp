#include "BsaOptimizer.h"

#include <catch2/catch_test_macros.hpp>

#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QSet>
#include <QTemporaryDir>

#include <optional>
#include <stdexcept>

namespace {
QString normalized(const QString &path) { return QDir::cleanPath(path); }

class MemoryArchiveFiles final : public ArchiveFileOperations {
public:
  QSet<QString> files;
  QSet<QString> directories;
  QStringList events;
  int stagingCount = 0;
  int moveCount = 0;
  int removeCount = 0;
  std::optional<int> failMove;
  std::optional<int> failRemove;

  void addFile(const QString &path) {
    files.insert(normalized(path));
    QString parent = QFileInfo(normalized(path)).absolutePath();
    while (!parent.isEmpty() && !directories.contains(parent)) {
      directories.insert(parent);
      const QString next = QFileInfo(parent).absolutePath();
      if (next == parent)
        break;
      parent = next;
    }
  }

  QString createSiblingStagingDirectory(const QString &anchor,
                                        const QString &purpose) override {
    ++stagingCount;
    const QFileInfo info(anchor);
    const QString parent = info.absolutePath();
    const QString path = normalized(QDir(parent).filePath(
        ".stage-" + purpose + QString::number(stagingCount)));
    directories.insert(path);
    events << "stage:" + path;
    return path;
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
           directories.contains(normalized(path));
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
    if (!files.remove(normalized(source)) || exists(destination))
      throw std::runtime_error("invalid virtual move");
    files.insert(normalized(destination));
  }

  void removeFile(const QString &path) override {
    ++removeCount;
    events << "remove:" + normalized(path);
    if (failRemove == removeCount)
      throw std::runtime_error("injected remove failure");
    files.remove(normalized(path));
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
      if (*it == root || it->startsWith(prefix, Qt::CaseInsensitive))
        it = files.erase(it);
      else
        ++it;
    }
    for (auto it = directories.begin(); it != directories.end();) {
      if (*it == root || it->startsWith(prefix, Qt::CaseInsensitive))
        it = directories.erase(it);
      else
        ++it;
    }
    events << "remove-tree:" + root;
  }
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

  void extractTo(const QString &, const QString &stagingRoot) override {
    ++extractCalls;
    if (failExtraction)
      throw std::runtime_error("injected engine failure");
    for (const QString &relative : extractedRelativeFiles)
      files.addFile(QDir(stagingRoot).filePath(relative));
  }

  StagedArchivePacking packTo(const QString &, const QString &stagingRoot,
                              const ArchiveExecutionPolicy &) override {
    ++packCalls;
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
  REQUIRE(files.stagingCount == 0);
  REQUIRE(files.events.isEmpty());
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
  REQUIRE(files.stagingCount == 0);
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
  const auto remove = std::find(files.events.begin(), files.events.end(),
                                "remove:C:/Mods/Alpha/textures/a.dds");
  REQUIRE(publish != files.events.end());
  REQUIRE(remove > publish);
}

TEST_CASE("BSA packing retains replaced archives as backups") {
  MemoryArchiveFiles files;
  files.directories.insert("C:/Mods/Alpha");
  files.addFile("C:/Mods/Alpha/Alpha.bsa");
  RecordedArchiveEngine engine(files);
  engine.packedOutputs = {{"Alpha.bsa", StagedArchiveOutputKind::Archive}};
  BSAOptimizer optimizer(engine, files);
  optimizer.packAll("C:/Mods/Alpha", packingPolicy(false));
  REQUIRE(files.exists("C:/Mods/Alpha/Alpha.bsa"));
  REQUIRE(files.exists("C:/Mods/Alpha/Alpha.bsa.bak"));
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
  REQUIRE(files.exists("C:/Mods/Alpha/a.dds"));
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

TEST_CASE("Qt archive staging is created beside rather than inside a Mod") {
  QTemporaryDir temporary;
  REQUIRE(temporary.isValid());
  const QString modPath = QDir(temporary.path()).filePath("Mods/Alpha");
  REQUIRE(QDir().mkpath(modPath));
  const QString archivePath = QDir(modPath).filePath("Alpha.bsa");
  QFile archive(archivePath);
  REQUIRE(archive.open(QIODevice::WriteOnly));
  archive.close();

  QtArchiveFileOperations files;
  const QString staging =
      files.createSiblingStagingDirectory(archivePath, "extract");

  REQUIRE(QFileInfo(staging).dir().absolutePath() ==
          QFileInfo(modPath).dir().absolutePath());
  files.removeTree(staging);
}
