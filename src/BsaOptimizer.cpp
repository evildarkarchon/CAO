/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "BsaOptimizer.h"
#include "PluginsOperations.h"

void BSAOptimizer::extract(QString bsaPath, const bool deleteBackup) const {
  if (!deleteBackup)
    bsaPath = backup(bsaPath);

  PLOG_VERBOSE << bsaPath;

  try {
    btu::bsa::unpack(btu::bsa::UnpackSettings{bsaPath.toStdU16String(),
                                              deleteBackup, false});
  } catch (const std::exception &e) {
    PLOG_ERROR << e.what();
    PLOG_ERROR << "An error occured during the extraction of: " + bsaPath +
                      '\n' +
                      "Please extract it manually. The BSA was not deleted.";
    return;
  }

  PLOG_INFO << "BSA successfully extracted: " + bsaPath;
}

void handle_errors(std::vector<std::pair<btu::Path, std::string>> errs) {
  if (errs.empty())
    return;

  PLOG_WARNING << "The following files failed to be packed. They will be "
                  "renamed to *.caobad:\n";
  for (auto &&[file, err] : errs) {
    PLOG_WARNING << file.native() << " : " << err;
    std::filesystem::rename(file, file.u8string() + u8".caobad");
  }
}

void BSAOptimizer::packAll(const QString &folderPath,
                           const ArchiveExecutionPolicy &policy) const {
  using dir_it = std::filesystem::directory_iterator;

  PLOG_VERBOSE << "Packing all loose files into BSAs";

  const auto game = policy.settings;
  const std::filesystem::path dir = folderPath.toStdU16String();

  auto plugins = btu::bsa::list_plugins(dir_it(dir), {}, game);
  btu::bsa::clean_dummy_plugins(plugins, game);

  auto bsas = btu::bsa::split(
      dir, game,
      [this, &policy](const btu::Path &dir,
                      btu::fs::directory_entry const &fileinfo) {
        return btu::bsa::default_is_allowed_path(dir, fileinfo) &&
               isAllowedFile(policy.filesToNotPack, dir, fileinfo);
      });

  if (policy.mergeIncompressible || policy.mergeTextures) {
    const auto msets = [&] {
      btu::bsa::MergeSettings sets = static_cast<btu::bsa::MergeSettings>(0);
      if (policy.mergeIncompressible)
        sets |= btu::bsa::MergeSettings::MergeIncompressible;
      if (policy.mergeTextures)
        sets |= btu::bsa::MergeSettings::MergeTextures;
      return sets;
    }();
    btu::bsa::merge(bsas, msets);
  }

  const auto default_plug =
      btu::bsa::FilePath(dir, dir.filename().u8string(), u8"", u8".esp",
                         btu::bsa::FileTypes::Plugin);
  if (plugins.empty()) // Used to find BSA name
    plugins.emplace_back(default_plug);

  for (auto &&bsa : bsas) {
    try {
      const auto files = std::vector(bsa.begin(), bsa.end());
      auto name = btu::bsa::find_archive_name(plugins, game, bsa.get_type());
      bsa.set_out_path(std::move(name).full_path());

      const auto errs =
          btu::bsa::write(policy.compress, std::move(bsa), dir);
      handle_errors(std::move(errs));
      if (policy.deleteSource) {
        std::for_each(files.begin(), files.end(), [](auto &&p) {
          try {
            std::filesystem::remove(p);
          } catch (const std::exception &) {
            PLOG_ERROR << "Failed to remove packed file: " << p.native();
          }
        });
      }

    } catch (const std::exception &e) {
      PLOG_ERROR << QString("An error occurred while packing BSAs: \n%2")
                        .arg(e.what());
    }
  }

  if (policy.createDummies) {
    const auto archives = btu::bsa::list_archive(dir_it(dir), {}, game);
    btu::bsa::make_dummy_plugins(archives, game);
  }
}

QString BSAOptimizer::backup(const QString &bsaPath) const {
  QFile bsaBackupFile(bsaPath + ".bak");
  const QFile bsaFile(bsaPath);

  while (bsaBackupFile.exists()) {
    if (bsaFile.size() == bsaBackupFile.size() &&
        QFile::remove(bsaBackupFile.fileName()))
      break;

    bsaBackupFile.setFileName(bsaBackupFile.fileName() + ".bak");
  }

  QFile::rename(bsaPath, bsaBackupFile.fileName());

  PLOG_VERBOSE << "Backuping BSA : " << bsaPath << " to "
               << bsaBackupFile.fileName();

  return bsaBackupFile.fileName();
}

bool BSAOptimizer::isAllowedFile(
    const std::vector<std::u8string> &filesToNotPack,
    [[maybe_unused]] btu::Path const &dir,
    btu::fs::directory_entry const &fileinfo) const {
  const auto &path = fileinfo.path().u8string();
  for (const auto &fileToNotPack : filesToNotPack) {
    if (btu::common::str_contain(path, fileToNotPack, false)) {
      PLOG_VERBOSE << btu::common::as_ascii(path)
                   << " ignored because of filesToNotPack. Rule: "
                   << btu::common::as_ascii(fileToNotPack);
      return false;
    }
  }

  return true;
}
