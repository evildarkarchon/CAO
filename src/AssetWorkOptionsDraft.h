/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "AssetWorkMode.h"
#include "pch.h"

namespace Ui {
class MainWindow;
}

/*
namespace plog
{
    Record& operator<<(Record& record, const
AssetWorkOptionsDraft& opt)
    {

    }
}*/

class AssetWorkOptionsDraft final {
public:
  /// Parses CLI arguments into this mutable input draft.
  ///
  /// \param args The complete process argument list, including the executable.
  /// \throws std::runtime_error When the argument shape, mode, or Profile is
  /// invalid.
  void parseArguments(const QStringList &args);

  /// Persists this draft to application settings.
  /// \param settings Non-null settings storage to update.
  void saveToIni(QSettings *settings);
  /// Replaces this draft's persisted fields from application settings.
  /// \param settings Non-null settings storage to read.
  void readFromIni(QSettings *settings);
  /// Copies this draft into the Main Window controls.
  /// \param ui Non-null generated Main Window UI to update.
  void saveToUi(Ui::MainWindow *ui);
  /// Replaces this draft's UI-controlled fields from the Main Window.
  /// \param ui Non-null generated Main Window UI to read.
  void readFromUi(Ui::MainWindow *ui);

  /*--------------VARS-------------------*/
  bool bBsaExtract = false;
  bool bBsaCreate = false;
  bool bBsaDeleteBackup = false;
  bool bBsaMergeIncomp = true;
  bool bBsaMergeTexture = false;
  bool bBsaProcessContent = false;
  bool bBsaCreateDummies = true;
  bool bBsaCompress = true;
  bool bBsaDeleteSource = true;

  bool bAnimationsOptimization = false;

  bool bDryRun = false;

  int iMeshesOptimizationLevel = 0;

  bool bMeshesHeadparts = true;
  bool bMeshesResave = false;

  bool bTexturesNecessary = true;
  bool bTexturesCompress = false;
  bool bTexturesMipmaps = false;

  bool bTexturesResizeSize = false;
  size_t iTexturesTargetHeight = 2048;
  size_t iTexturesTargetWidth = 2048;
  bool bTexturesResizeRatio = false;
  uint iTexturesTargetWidthRatio = 1;
  uint iTexturesTargetHeightRatio = 1;

  bool bDebugLog = false;

  /*!
   * \brief The optimization mode
   */
  AssetWorkMode mode = AssetWorkMode::SingleMod;

  /*!
   * \brief The path given by the user
   */
  QString userPath;
  /*-----------END OF VARS---------------*/
};
