#include "MeshesOptimizer.h"

#include <catch2/catch_test_macros.hpp>

#include <QDir>
#include <QFile>
#include <QTemporaryDir>

TEST_CASE("Mesh Asset transaction classifies a missing source as operational "
          "failure") {
  QTemporaryDir tempDir;
  REQUIRE(tempDir.isValid());
  MeshExecutionPolicy policy;
  policy.optimizationLevel = 1;
  MeshesOptimizer transaction(policy);

  const auto result =
      transaction.executeAsset(QDir(tempDir.path()).filePath("missing.nif"),
                               MeshAssetRole::Regular, false);

  REQUIRE(result.status == AssetTransactionStatus::OperationalFailure);
  REQUIRE(result.notices.size() == 1);
  REQUIRE(result.notices.front().code ==
          AssetTransactionNoticeCode::OperationalFailure);
}

TEST_CASE(
    "Mesh Asset transaction classifies readable invalid data as malformed") {
  QTemporaryDir tempDir;
  REQUIRE(tempDir.isValid());
  const QString meshPath = QDir(tempDir.path()).filePath("bad.nif");
  QFile mesh(meshPath);
  REQUIRE(mesh.open(QIODevice::WriteOnly));
  REQUIRE(mesh.write("not-a-nif") == 9);
  mesh.close();

  MeshExecutionPolicy policy;
  policy.optimizationLevel = 1;
  MeshesOptimizer transaction(policy);
  const auto result =
      transaction.executeAsset(meshPath, MeshAssetRole::Regular, false);

  REQUIRE(result.status == AssetTransactionStatus::MalformedAsset);
  REQUIRE(result.notices.front().code ==
          AssetTransactionNoticeCode::MalformedAsset);
}
