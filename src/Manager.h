/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "AssetRouting/AssetRouter.h"
#include "FilesystemOperations.h"
#include "OptionsCAO.h"
#include "pch.h"

class Manager final : public QObject
{
  Q_OBJECT
public:
  /*!
   * \brief Creates a run and performs discovery only after routing setup has succeeded.
   * \param opt Application options whose lifetime must exceed this Manager.
   * \param routingPolicy Immutable validated policy retained for the entire run.
   */
  Manager(const OptionsCAO &opt, cao::routing::RoutingPolicy routingPolicy);
  /**
   * Runs Archive-first discovery, definitive routing, ordered execution, aggregate reporting,
   * and optional post-execution packing. Cancellation is observed between carried Asset attempts.
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
   * \brief List all the directories to process
   */
  void listDirectories();
  /*!
   * \brief Read ignoredMods.txt and store it to a list
   */
  void readIgnoredMods();
  /*!
   * \brief The number of completed files. Used to determine progress
   */
  int _numberCompletedFiles = 0;
  /*!
   * \brief The optimization options, that will be given to the MainOptimizer
   */
  const OptionsCAO& _options;
  /*!
   * \brief The immutable policy retained for the routing cutover later in this run.
   */
  const cao::routing::RoutingPolicy _routingPolicy;
  /*!
   * \brief The list of directories to process
   */
  QStringList _modsToProcess;
  /*!
   * \brief Mods on this list won't be processed
   */
  QStringList _ignoredMods;
  /*!
   * \brief Used to read the INI
   */
  QSettings* _settings;
  bool _isCancelled = false;

  signals:
  void progressBarTextChanged(QString, int, int);
  void end();
};
