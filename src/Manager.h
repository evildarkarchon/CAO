/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "AssetWorkOptions.h"
#include "AssetWorkPlan.h"
#include "AssetWorkProfileSnapshot.h"
#include "MainOptimizer.h"
#include "pch.h"

#include <atomic>
#include <optional>

class Manager final : public QObject {
  Q_OBJECT
public:
  /*!
   * \brief Creates an Asset Work Plan Execution manager from owned run input.
   * \param options Validated immutable Asset Work Options.
   * \param selectedPath The selected Mod or parent path to process.
   * \param debugLog Whether verbose debug records should be written.
   */
  Manager(AssetWorkOptions options, QString selectedPath, bool debugLog);
  /*!
   * \brief The main process
   */
  void runOptimization();
  /*!
   * \brief Print the progress to stdout
   * \param text The text to display
   * \param total The total number of files to process
   */
  void printProgress(const int &total, const QString &text);

  void cancelProcess();

private:
  /*!
   * \brief Initializes logging, validates run context, and captures the
   * selected Profile exactly once.
   * \throws std::runtime_error When the selected path or Profile is invalid.
   */
  void init();
  /*!
   * \brief Read ignoredMods.txt and store it to a list
   */
  void readIgnoredMods();
  /*!
   * \brief The number of completed files. Used to determine progress
   */
  int _numberCompletedFiles = 0;
  /*!
   * \brief The optimization options used to build planning and execution
   * policy.
   */
  AssetWorkOptions _options;
  /*! \brief Selected Mod or parent path, kept outside Asset Work Options. */
  QString _selectedPath;
  /*! \brief Logging preference, kept outside Asset Work Options. */
  bool _debugLog = false;
  /*! \brief Validated immutable Profile facts captured during construction. */
  std::optional<AssetWorkProfileSnapshot> _profileSnapshot;
  /*!
   * \brief Mods on this list won't be processed
   */
  QStringList _ignoredMods;

  std::atomic<bool> _isCancelled = false;

signals:
  void progressBarTextChanged(QString, int, int);
};
