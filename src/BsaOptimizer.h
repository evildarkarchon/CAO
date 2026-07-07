/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "AssetWorkExecutionPolicy.h"
#include "FilesystemOperations.h"
#include "pch.h"

/*!
 * \brief Manages BSA : extract and create them
 */
class BSAOptimizer final : public QObject {
  Q_DECLARE_TR_FUNCTIONS(BsaOptimizer)

public:
  /*!
   * \brief Default constructor
   */
  BSAOptimizer() = default;
  /*!
   * \brief Extracts a BSA
   * \param bsaPath The path of the BSA to extract
   * \param deleteBackup Deletes the backup the existing bsa
   */
  void extract(QString bsaPath, const bool deleteBackup) const;
  /*!
   * \brief Creates a BSA containing all the files given as argument
   * \param bsa The BSA to create
   */
  int create(btu::bsa::ArchiveData &bsa, bool allowCompression,
             bool deleteSource) const;

  /*!
   * \brief Packs all the loose files in the directory into BSAs
   * \param folderPath The folder to process
   * \param policy The archive execution rules and resolved BSA settings.
   */
  void packAll(const QString &folderPath,
               const ArchiveExecutionPolicy &policy) const;

private:
  /*!
   * \brief Adds .bak to the bsa name. If a bak file already exist, their sizes
   * are compared. If the size is the same, the current bsa is removed.
   * Otherwise, the bak file is also renamed.
   * \param bsaPath The BSA to backup
   * \return a QString containing the name of the backup-ed bsa, or an empty
   * QString if the backup could not be created.
   */
  QString backup(const QString &bsaPath) const;
  /*!
   * \brief Checks if the file is present in the list filesToNotPack
   * \return a bool indicating the state of the file. True if is allowed, false
   * otherwise
   */
  bool isAllowedFile(const std::vector<std::u8string> &filesToNotPack,
                     const btu::Path &dir,
                     const std::filesystem::directory_entry &fileinfo) const;
};
