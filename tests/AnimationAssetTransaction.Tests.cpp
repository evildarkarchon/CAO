#include "AnimationAssetTransaction.h"

#include <catch2/catch_test_macros.hpp>

#include <QDir>
#include <QFile>
#include <QTemporaryDir>

#include <memory>

namespace {
void writeFile(const QString &path, const QByteArray &contents) {
  QFile file(path);
  REQUIRE(file.open(QIODevice::WriteOnly));
  REQUIRE(file.write(contents) == contents.size());
}

class ScriptedAnimationConversionTool final : public AnimationConversionTool {
public:
  AnimationToolResult result;
  QStringList requests;

  AnimationToolResult convert(const QString &sourcePath) override {
    requests.push_back(sourcePath);
    return result;
  }
};

class ScriptedAnimationOutputCommitter final : public AnimationOutputCommitter {
public:
  AssetTransactionResult result{AssetTransactionStatus::Completed,
                                {{AssetTransactionNoticeCode::CompletedAction,
                                  {},
                                  {},
                                  "Committed animation output"}}};
  QVector<AnimationOutputCommitRequest> requests;

  AssetTransactionResult
  commit(const AnimationOutputCommitRequest &request) override {
    requests.push_back(request);
    return result;
  }
};
} // namespace

TEST_CASE("Animation Asset transaction reports Dry Run intent without invoking "
          "hkxcmd") {
  QTemporaryDir tempDir;
  REQUIRE(tempDir.isValid());
  const QString sourcePath = QDir(tempDir.path()).filePath("idle.hkx");
  writeFile(sourcePath, "oldrim-animation");

  auto tool = std::make_unique<ScriptedAnimationConversionTool>();
  auto *recordedTool = tool.get();
  auto committer = std::make_unique<ScriptedAnimationOutputCommitter>();
  auto *recordedCommitter = committer.get();
  AnimationAssetTransaction transaction(true, std::move(tool),
                                        std::move(committer));

  const auto result = transaction.execute(sourcePath);

  REQUIRE(result.status == AssetTransactionStatus::Completed);
  REQUIRE(recordedTool->requests.isEmpty());
  REQUIRE(recordedCommitter->requests.isEmpty());
  REQUIRE(result.notices.size() == 1);
  REQUIRE(result.notices.front().code ==
          AssetTransactionNoticeCode::IntendedAction);
}

TEST_CASE("Animation Asset transaction classifies a missing source as "
          "operational failure") {
  QTemporaryDir tempDir;
  REQUIRE(tempDir.isValid());
  const QString sourcePath = QDir(tempDir.path()).filePath("missing.hkx");

  auto tool = std::make_unique<ScriptedAnimationConversionTool>();
  auto *recordedTool = tool.get();
  auto committer = std::make_unique<ScriptedAnimationOutputCommitter>();
  AnimationAssetTransaction transaction(false, std::move(tool),
                                        std::move(committer));

  const auto result = transaction.execute(sourcePath);

  REQUIRE(result.status == AssetTransactionStatus::OperationalFailure);
  REQUIRE(recordedTool->requests.isEmpty());
}

TEST_CASE(
    "Animation Asset transaction leaves an already converted Asset unchanged") {
  QTemporaryDir tempDir;
  REQUIRE(tempDir.isValid());
  const QString sourcePath = QDir(tempDir.path()).filePath("idle.hkx");
  writeFile(sourcePath, "sse-animation");

  auto tool = std::make_unique<ScriptedAnimationConversionTool>();
  tool->result = {AnimationToolStatus::AlreadyConverted,
                  {},
                  "Animation is already converted"};
  auto committer = std::make_unique<ScriptedAnimationOutputCommitter>();
  auto *recordedCommitter = committer.get();
  AnimationAssetTransaction transaction(false, std::move(tool),
                                        std::move(committer));

  const auto result = transaction.execute(sourcePath);

  REQUIRE(result.status == AssetTransactionStatus::Unchanged);
  REQUIRE(recordedCommitter->requests.isEmpty());
}

TEST_CASE("Animation Asset transaction reports tool and commit failures as "
          "operational") {
  QTemporaryDir tempDir;
  REQUIRE(tempDir.isValid());
  const QString sourcePath = QDir(tempDir.path()).filePath("idle.hkx");
  writeFile(sourcePath, "original-animation");

  SECTION("tool failure") {
    auto tool = std::make_unique<ScriptedAnimationConversionTool>();
    tool->result = {
        AnimationToolStatus::OperationalFailure, {}, "hkxcmd could not start"};
    auto committer = std::make_unique<ScriptedAnimationOutputCommitter>();
    auto *recordedCommitter = committer.get();
    AnimationAssetTransaction transaction(false, std::move(tool),
                                          std::move(committer));

    const auto result = transaction.execute(sourcePath);

    REQUIRE(result.status == AssetTransactionStatus::OperationalFailure);
    REQUIRE(recordedCommitter->requests.isEmpty());
  }

  SECTION("commit failure") {
    auto tool = std::make_unique<ScriptedAnimationConversionTool>();
    tool->result = {
        AnimationToolStatus::Converted, QByteArray("converted-animation"), {}};
    auto committer = std::make_unique<ScriptedAnimationOutputCommitter>();
    committer->result = {
        AssetTransactionStatus::OperationalFailure,
        {{AssetTransactionNoticeCode::OperationalFailure, sourcePath,
          sourcePath, "Could not save converted animation"}}};
    AnimationAssetTransaction transaction(false, std::move(tool),
                                          std::move(committer));

    const auto result = transaction.execute(sourcePath);

    REQUIRE(result.status == AssetTransactionStatus::OperationalFailure);
    QFile original(sourcePath);
    REQUIRE(original.open(QIODevice::ReadOnly));
    REQUIRE(original.readAll() == QByteArray("original-animation"));
  }
}

TEST_CASE("Animation Asset transaction commits converted bytes through its "
          "output seam") {
  QTemporaryDir tempDir;
  REQUIRE(tempDir.isValid());
  const QString sourcePath = QDir(tempDir.path()).filePath("idle.hkx");
  writeFile(sourcePath, "oldrim-animation");

  auto tool = std::make_unique<ScriptedAnimationConversionTool>();
  tool->result = {
      AnimationToolStatus::Converted, QByteArray("converted-animation"), {}};
  auto committer = std::make_unique<ScriptedAnimationOutputCommitter>();
  auto *recordedCommitter = committer.get();
  AnimationAssetTransaction transaction(false, std::move(tool),
                                        std::move(committer));

  const auto result = transaction.execute(sourcePath);

  REQUIRE(result.status == AssetTransactionStatus::Completed);
  REQUIRE(recordedCommitter->requests.size() == 1);
  REQUIRE(recordedCommitter->requests.front().targetPath == sourcePath);
  REQUIRE(recordedCommitter->requests.front().convertedHkx ==
          QByteArray("converted-animation"));
}

TEST_CASE(
    "Atomic animation output committer replaces the original only at commit") {
  QTemporaryDir tempDir;
  REQUIRE(tempDir.isValid());
  const QString sourcePath = QDir(tempDir.path()).filePath("idle.hkx");
  writeFile(sourcePath, "original-animation");

  AtomicAnimationOutputCommitter committer;
  const auto result = committer.commit(AnimationOutputCommitRequest{
      sourcePath, QByteArray("converted-animation")});

  REQUIRE(result.status == AssetTransactionStatus::Completed);
  QFile converted(sourcePath);
  REQUIRE(converted.open(QIODevice::ReadOnly));
  REQUIRE(converted.readAll() == QByteArray("converted-animation"));
}
