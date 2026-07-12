#include "LegacyArchiveWorkspaceDiscovery.h"

#include <catch2/catch_test_macros.hpp>

#include <QDir>
#include <QFileInfo>
#include <QTemporaryDir>

TEST_CASE("Legacy Archive Workspace Discovery reports exact old workspaces for "
          "manual resolution") {
  QTemporaryDir temporary;
  REQUIRE(temporary.isValid());
  const QDir root(temporary.path());
  const QString packing =
      root.filePath(".Alpha.cao-pack-01234567-89ab-cdef-0123-456789abcdef");
  const QString extraction = root.filePath(
      ".Alpha.bsa.cao-extract-fedcba98-7654-3210-fedc-ba9876543210");
  const QString ordinaryMod = root.filePath("Alpha");
  REQUIRE(QDir().mkpath(packing));
  REQUIRE(QDir().mkpath(extraction));
  REQUIRE(QDir().mkpath(ordinaryMod));

  const auto result = LegacyArchiveWorkspaceDiscovery::discover(
      {extraction, ordinaryMod, packing});

  REQUIRE(result.candidateModPaths == QStringList{ordinaryMod});
  REQUIRE(result.legacyWorkspacePaths == QStringList{extraction, packing});
  const auto failure = result.manualResolutionFailure();
  REQUIRE(failure.has_value());
  REQUIRE(failure->operation() == ArchiveOperation::Recovery);
  REQUIRE(failure->stage() == ArchiveFailureStage::RecoveryDiscovery);
  REQUIRE(failure->assetPath() == extraction);
  REQUIRE(failure->workspacePaths() == QStringList{extraction, packing});
  REQUIRE(failure->diagnostic() ==
          QString("Legacy Archive Workspaces require manual resolution: %1; %2")
              .arg(extraction, packing));

  // Discovery is deliberately read-only, including for candidates that a
  // caller later identifies as ignored or unrelated to the requested scope.
  REQUIRE(QFileInfo::exists(packing));
  REQUIRE(QFileInfo::exists(extraction));
}

TEST_CASE("Legacy Archive Workspace Discovery keeps legacy-looking Mods in the "
          "candidate scope") {
  QTemporaryDir temporary;
  REQUIRE(temporary.isValid());
  const QDir root(temporary.path());
  const QString uuid = "01234567-89ab-cdef-0123-456789abcdef";
  const QStringList lookalikes{
      root.filePath("Alpha.cao-pack-" + uuid),
      root.filePath(".cao-pack-" + uuid),
      root.filePath(".Alpha.cao-pack-{" + uuid + "}"),
      root.filePath(".Alpha.cao-pack-" + uuid + "-suffix"),
      root.filePath(".Alpha.cao-packing-" + uuid),
      root.filePath(".Alpha.cao-extract-not-a-uuid"),
      root.filePath("Alpha.cao-pack-output")};
  for (const QString &path : lookalikes)
    REQUIRE(QDir().mkpath(path));

  const auto result = LegacyArchiveWorkspaceDiscovery::discover(lookalikes);

  REQUIRE(result.candidateModPaths == lookalikes);
  REQUIRE(result.legacyWorkspacePaths.isEmpty());
  REQUIRE_FALSE(result.manualResolutionFailure().has_value());
}

TEST_CASE("Legacy Archive Workspace Discovery omits workspaces owned by "
          "ignored Mods") {
  const QString packing =
      "C:/Mods/.Alpha.cao-pack-01234567-89ab-cdef-0123-456789abcdef";
  const QString extraction =
      "C:/Mods/.Alpha.bsa.cao-extract-fedcba98-7654-3210-fedc-ba9876543210";

  const auto result = LegacyArchiveWorkspaceDiscovery::discover(
      {packing, extraction}, {QStringLiteral("alpha")});

  REQUIRE(result.candidateModPaths.isEmpty());
  REQUIRE(result.legacyWorkspacePaths.isEmpty());
  REQUIRE(result.ignoredLegacyWorkspacePaths ==
          QStringList{packing, extraction});
  REQUIRE_FALSE(result.manualResolutionFailure().has_value());
}
