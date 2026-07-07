/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "AssetWorkPlan.h"
#include "MainOptimizer.h"
#include "OptionsCAO.h"
#include "pch.h"

#include <atomic>

class Manager final : public QObject {
  Q_OBJECT
public:
  /*!
   * \brief Constructor that will perform a number of functions
   */
  explicit Manager(const OptionsCAO &opt);
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
   * \brief Initializes the manager
   */
  void init();
  /*!
   * \brief Read ignoredMods.txt and store it to a list
   */
  void readIgnoredMods();
  /*!
   * \brief Builds the planner request from the current options and profile.
   * \return The request used to plan asset work.
   */
  AssetWorkPlanRequest createAssetWorkPlanRequest() const;
  /*!
   * \brief The number of completed files. Used to determine progress
   */
  int _numberCompletedFiles = 0;
  /*!
   * \brief The optimization options used to build planning and execution
   * policy.
   */
  const OptionsCAO &_options;
  /*!
   * \brief Mods on this list won't be processed
   */
  QStringList _ignoredMods;

  std::atomic<bool> _isCancelled = false;

signals:
  void progressBarTextChanged(QString, int, int);
  void end();
};
