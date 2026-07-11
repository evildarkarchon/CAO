#include "TextureAssetTransaction.h"

#include <catch2/catch_test_macros.hpp>

#include <QDir>
#include <QFile>
#include <QTemporaryDir>

#include <memory>

namespace {
class ScriptedTextureTransformEngine final : public TextureTransformEngine {
public:
  TextureTransformResult result;
  QVector<TextureTransformRequest> requests;

  TextureTransformResult
  transform(const TextureTransformRequest &request) override {
    requests.push_back(request);
    return result;
  }
};

class RecordedTextureOutputCommitter final : public TextureOutputCommitter {
public:
  QVector<TextureOutputCommitRequest> requests;

  AssetTransactionResult
  commit(const TextureOutputCommitRequest &request) override {
    requests.push_back(request);
    return {AssetTransactionStatus::Completed,
            {{AssetTransactionNoticeCode::CompletedAction, request.sourcePath,
              request.targetPath, "Committed texture output"}}};
  }
};
} // namespace

TEST_CASE("Texture Asset transaction inspects Dry Run work without committing "
          "output") {
  auto engine = std::make_unique<ScriptedTextureTransformEngine>();
  engine->result = {
      AssetTransactionStatus::Completed,
      QByteArray("encoded-dds"),
      {{AssetTransactionNoticeCode::IntendedAction, "textures/source.tga",
        "textures/source.dds", "Would convert TGA to DDS"}}};
  auto *recordedEngine = engine.get();
  auto committer = std::make_unique<RecordedTextureOutputCommitter>();
  auto *recordedCommitter = committer.get();
  TextureAssetTransaction transaction(true, std::move(engine),
                                      std::move(committer));

  const auto result =
      transaction.execute("textures/source.tga", TextureSourceKind::Tga);

  REQUIRE(recordedEngine->requests.size() == 1);
  REQUIRE(recordedEngine->requests.front().dryRun);
  REQUIRE(recordedCommitter->requests.isEmpty());
  REQUIRE(result.status == AssetTransactionStatus::Completed);
  REQUIRE(result.notices.size() == 1);
  REQUIRE(result.notices.front().code ==
          AssetTransactionNoticeCode::IntendedAction);
}

TEST_CASE("Atomic texture output committer replaces a DDS only after bytes are "
          "staged") {
  QTemporaryDir tempDir;
  REQUIRE(tempDir.isValid());
  const QString texturePath = QDir(tempDir.path()).filePath("source.dds");
  {
    QFile original(texturePath);
    REQUIRE(original.open(QIODevice::WriteOnly));
    REQUIRE(original.write("original-dds") == 12);
  }

  AtomicTextureOutputCommitter committer;
  const auto result = committer.commit(TextureOutputCommitRequest{
      texturePath, texturePath, QByteArray("replacement-dds"), false});

  REQUIRE(result.status == AssetTransactionStatus::Completed);
  QFile committed(texturePath);
  REQUIRE(committed.open(QIODevice::ReadOnly));
  REQUIRE(committed.readAll() == QByteArray("replacement-dds"));
  REQUIRE_FALSE(QFile::exists(texturePath + ".caotmp"));
}

TEST_CASE("Atomic texture output committer publishes DDS before removing TGA") {
  QTemporaryDir tempDir;
  REQUIRE(tempDir.isValid());
  const QDir root(tempDir.path());
  const QString sourcePath = root.filePath("source.tga");
  const QString targetPath = root.filePath("source.dds");
  {
    QFile source(sourcePath);
    REQUIRE(source.open(QIODevice::WriteOnly));
    REQUIRE(source.write("source-tga") == 10);
  }
  {
    QFile previousTarget(targetPath);
    REQUIRE(previousTarget.open(QIODevice::WriteOnly));
    REQUIRE(previousTarget.write("previous-dds") == 12);
  }

  AtomicTextureOutputCommitter committer;
  const auto result = committer.commit(TextureOutputCommitRequest{
      sourcePath, targetPath, QByteArray("converted-dds"), true});

  REQUIRE(result.status == AssetTransactionStatus::Completed);
  REQUIRE_FALSE(QFile::exists(sourcePath));
  QFile target(targetPath);
  REQUIRE(target.open(QIODevice::ReadOnly));
  REQUIRE(target.readAll() == QByteArray("converted-dds"));
  REQUIRE_FALSE(QFile::exists(targetPath + ".caorollback"));
}
