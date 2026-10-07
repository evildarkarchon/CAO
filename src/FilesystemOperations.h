/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "pch.h"

#include <span>
#include <stop_token>
#include <string>

/*!
 * \brief Manages filesystem operations : moving files, deleting empty dirs...
 */
class FilesystemOperations final : public QObject {
    Q_DECLARE_TR_FUNCTIONS(FilesystemOperations)

   public:
    /*!
     * \brief Delete empty directories in the given directory
     * Reserved staging paths remain owned by the run's Safety Cleanup and are never removed here.

     * * Preserves the selected root, its ancestors, and linked directories; removal is best
     * effort.
     * \param folderPath The path of the folder where empty dirs will be deleted
     * \return The number of child directories actually removed.
     */
    static std::size_t deleteEmptyDirectories(const QString& folderPath);
    /*!
     * \brief Compares if two folders have the same file structure. Currently only used for testing.
     * \param folder1 The first folder
     * \param folder2 The second folder
     * \param checkFileSize Whether file sizes will be checked or not
     * \return a bool : true if the folders are identical, false otherwise
     */
    static bool compareFolders(const QString& folder1, const QString& folder2,
                               const bool& checkFileSize);
    /*!
     * \brief Will copy all files from source folder into destination folder.Currently only used for
     * testing.
     * \param source The source directory
     * \param destination The destination directory
     * \param overwriteExisting If enabled, source files will overwrite destination files
     */
    static void copyDir(const QString& source, const QString& destination, bool overwriteExisting);

    static QStringList readFile(QFile& file, std::function<void(QString& line)> function);
    static QStringList readFile(QFile& file);

    /// Lists plugin paths while polling cancellation between directory entries.
    /// \param extensions The Profile game's plugin extensions (e.g. btu::bsa::Settings
    /// plugin_extensions), each with its leading dot; file names match them case-insensitively.
    /// Throws AssetInitializationCancelled before returning a partial list.
    static QStringList listPlugins(QDirIterator& it, std::span<const std::u8string> extensions,
                                   std::stop_token stop = {});
};
