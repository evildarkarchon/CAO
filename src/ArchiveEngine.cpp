/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "ArchiveEngine.h"
#include "AssetPathVisibility.h"

#include "btu/bsa/pack.hpp"
#include "btu/bsa/plugin.hpp"
#include "btu/bsa/unpack.hpp"

#include <QDir>
#include <QFileInfo>

#include <filesystem>
#include <stdexcept>

namespace {
QString fromPath(const std::filesystem::path &path) {
  return QString::fromStdU16String(path.u16string());
}

bool isAllowedFile(const ArchiveExecutionPolicy &policy, const btu::Path &dir,
                   const btu::fs::directory_entry &fileInfo) {
  if (AssetPathVisibility::isInternalPath(fromPath(fileInfo.path())))
    return false;

  if (!btu::bsa::default_is_allowed_path(dir, fileInfo))
    return false;

  const auto &path = fileInfo.path().u8string();
  return std::none_of(policy.filesToNotPack.begin(),
                      policy.filesToNotPack.end(),
                      [&](const std::u8string &rule) {
                        return btu::common::str_contain(path, rule, false);
                      });
}
} // namespace

void BtuArchiveEngine::extractTo(const QString &archivePath,
                                 const QString &stagingRoot) {
  const btu::Path archive(archivePath.toStdU16String());
  const btu::Path staging(stagingRoot.toStdU16String());
  // Source ownership belongs to the transaction. bethutil may only write into
  // staging so an engine failure cannot remove the live archive.
  btu::bsa::unpack(btu::bsa::UnpackSettings{archive, false, false, &staging});
}

StagedArchivePacking
BtuArchiveEngine::packTo(const QString &modPath, const QString &stagingRoot,
                         const ArchiveExecutionPolicy &policy) {
  using DirectoryIterator = std::filesystem::directory_iterator;

  const btu::Path mod(modPath.toStdU16String());
  const btu::Path staging(stagingRoot.toStdU16String());
  auto plugins =
      btu::bsa::list_plugins(DirectoryIterator(mod), {}, policy.settings);
  btu::bsa::clean_dummy_plugins(plugins, policy.settings);

  auto archives = btu::bsa::split(
      mod, policy.settings,
      [&policy](const btu::Path &root, const btu::fs::directory_entry &entry) {
        return isAllowedFile(policy, root, entry);
      });

  if (policy.mergeIncompressible || policy.mergeTextures) {
    auto mergeSettings = static_cast<btu::bsa::MergeSettings>(0);
    if (policy.mergeIncompressible)
      mergeSettings |= btu::bsa::MergeSettings::MergeIncompressible;
    if (policy.mergeTextures)
      mergeSettings |= btu::bsa::MergeSettings::MergeTextures;
    btu::bsa::merge(archives, mergeSettings);
  }

  if (plugins.empty()) {
    plugins.emplace_back(mod, mod.filename().u8string(), u8"", u8".esp",
                         btu::bsa::FileTypes::Plugin);
  }

  StagedArchivePacking result;
  for (auto &&archive : archives) {
    const auto sourcePaths = std::vector(archive.begin(), archive.end());
    auto output = btu::bsa::find_archive_name(plugins, policy.settings,
                                              archive.get_type());
    const auto outputName = output.full_name();
    const btu::Path stagedPath = staging / outputName;
    archive.set_out_path(stagedPath);

    const auto errors =
        btu::bsa::write(policy.compress, std::move(archive), mod);
    if (!errors.empty()) {
      QStringList diagnostics;
      for (const auto &[path, error] : errors) {
        diagnostics << QString("%1: %2").arg(fromPath(path),
                                             QString::fromStdString(error));
      }
      throw std::runtime_error(
          QString("bethutil could not pack one or more Assets:\n%1")
              .arg(diagnostics.join('\n'))
              .toStdString());
    }

    for (const auto &source : sourcePaths)
      result.packedSourceAssets << fromPath(source);
    result.outputs.push_back(
        {fromPath(stagedPath),
         QString::fromStdU16String(
             std::filesystem::path(outputName).u16string()),
         StagedArchiveOutputKind::Archive});
  }

  if (policy.createDummies) {
    const auto stagedArchives =
        btu::bsa::list_archive(DirectoryIterator(staging), {}, policy.settings);
    btu::bsa::make_dummy_plugins(stagedArchives, policy.settings);

    const QDir stagingDir(stagingRoot);
    for (const QFileInfo &entry :
         stagingDir.entryInfoList(QDir::Files | QDir::NoDotAndDotDot)) {
      const bool alreadyKnown = std::any_of(
          result.outputs.begin(), result.outputs.end(),
          [&](const StagedArchiveOutput &known) {
            return QFileInfo(known.stagingPath).absoluteFilePath() ==
                   entry.absoluteFilePath();
          });
      if (!alreadyKnown) {
        result.outputs.push_back({entry.absoluteFilePath(), entry.fileName(),
                                  StagedArchiveOutputKind::DummyPlugin});
      }
    }
  }

  result.packedSourceAssets.removeDuplicates();
  return result;
}
