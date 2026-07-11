/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#include "AssetWorkOptionsDraft.h"
#include "Profiles.h"

void AssetWorkOptionsDraft::saveToIni(QSettings *settings) {
  // General
  settings->setValue("bDryRun", bDryRun);
  settings->setValue("bDebugLog", bDebugLog);
  settings->setValue("mode", static_cast<int>(mode));
  settings->setValue("userPath", userPath);

  // BSA
  settings->beginGroup("BSA");
  settings->setValue("bBsaExtract", bBsaExtract);
  settings->setValue("bBsaCreate", bBsaCreate);
  settings->setValue("bBsaDeleteBackup", bBsaDeleteBackup);
  settings->setValue("bBsaMergeIncomp", bBsaMergeIncomp);
  settings->setValue("bBsaMergeTexture", bBsaMergeTexture);
  settings->setValue("bBsaProcessContent", bBsaProcessContent);
  settings->setValue("bBsaCreateDummies", bBsaCreateDummies);
  settings->setValue("bBsaCompress", bBsaCompress);
  settings->setValue("bBsaDeleteSource", bBsaDeleteSource);
  settings->endGroup();

  // Textures
  settings->beginGroup("Textures");
  settings->setValue("bTexturesNecessary", bTexturesNecessary);
  settings->setValue("bTexturesCompress", bTexturesCompress);
  settings->setValue("bTexturesMipmaps", bTexturesMipmaps);

  settings->setValue("bTexturesResizeSize", bTexturesResizeSize);
  settings->setValue("iTexturesTargetWidth", iTexturesTargetWidth);
  settings->setValue("iTexturesTargetHeight", iTexturesTargetHeight);

  settings->setValue("bTexturesResizeRatio", bTexturesResizeRatio);
  settings->setValue("iTexturesTargetHeightRatio", iTexturesTargetHeightRatio);
  settings->setValue("iTexturesTargetWidthRatio", iTexturesTargetWidthRatio);
  settings->endGroup();

  // Meshes
  settings->setValue("Meshes/iMeshesOptimizationLevel",
                     iMeshesOptimizationLevel);

  // Meshes advanced
  settings->setValue("Meshes/bMeshesHeadparts", bMeshesHeadparts);
  settings->setValue("Meshes/bMeshesResave", bMeshesResave);

  // Animations
  settings->setValue("Animations/bAnimationsOptimization",
                     bAnimationsOptimization);
}

void AssetWorkOptionsDraft::readFromIni(QSettings *settings) {
  if (!QFile(settings->fileName()).exists())
    return;

  // General
  bDryRun = settings->value("bDryRun").toBool();
  bDebugLog = settings->value("bDebugLog").toBool();
  mode = static_cast<AssetWorkMode>(settings->value("mode").toInt());
  if (!settings->value("userPath").toString().isEmpty())
    userPath = settings->value("userPath").toString();

  // BSA
  settings->beginGroup("BSA");
  bBsaExtract = settings->value("bBsaExtract").toBool();
  bBsaCreate = settings->value("bBsaCreate").toBool();
  bBsaDeleteBackup = settings->value("bBsaDeleteBackup").toBool();
  bBsaProcessContent = settings->value("bBsaProcessContent").toBool();
  bBsaMergeIncomp = settings->value("bBsaMergeIncomp").toBool();
  bBsaMergeTexture = settings->value("bBsaMergeTexture").toBool();
  bBsaCreateDummies = settings->value("bBsaCreateDummies").toBool();
  bBsaCompress = settings->value("bBsaCompress").toBool();
  bBsaDeleteSource = settings->value("bBsaDeleteSource").toBool();
  settings->endGroup();

  // Textures
  settings->beginGroup("Textures");
  bTexturesNecessary = settings->value("bTexturesNecessary").toBool();
  bTexturesCompress = settings->value("bTexturesCompress").toBool();
  bTexturesMipmaps = settings->value("bTexturesMipmaps").toBool();

  bTexturesResizeSize = settings->value("bTexturesResizeSize").toBool();
  iTexturesTargetWidth = settings->value("iTexturesTargetWidth").toUInt();
  iTexturesTargetHeight = settings->value("iTexturesTargetHeight").toUInt();

  bTexturesResizeRatio = settings->value("bTexturesResizeRatio").toBool();
  iTexturesTargetWidthRatio =
      settings->value("iTexturesTargetWidthRatio").toUInt();
  iTexturesTargetHeightRatio =
      settings->value("iTexturesTargetHeightRatio").toUInt();
  settings->endGroup();

  // Meshes
  iMeshesOptimizationLevel =
      settings->value("Meshes/iMeshesOptimizationLevel").toInt();

  // Meshes advanced
  bMeshesHeadparts = settings->value("Meshes/bMeshesHeadparts").toBool();
  bMeshesResave = settings->value("Meshes/bMeshesResave").toBool();

  // Animations
  bAnimationsOptimization =
      settings->value("Animations/bAnimationsOptimization").toBool();
}

void AssetWorkOptionsDraft::parseArguments(const QStringList &args) {
  if (args.count() < 4)
    throw std::runtime_error("Not enough arguments");
  QCommandLineParser parser;

  parser.addHelpOption();

  parser.addPositionalArgument(
      "folder", "The folder to process, surrounded with quotes.");
  parser.addPositionalArgument("mode",
                               "Either om (one mod) or sm (several mods)");
  parser.addPositionalArgument("profile",
                               "One of the profile located in CAO/profiles");

  parser.addOptions({
      {"dr", "Enables dry run"},
      {"l", "Enables debug log"},
      {"m",
       "Mesh processing level: 0 (default) to disable optimization, 1 for "
       "necessary optimization, "
       "2 for medium optimization, 3 for full optimization.",
       "value", "0"},

      {"t0", "Enables textures necessary optimization"},
      {"t1", "Enables textures compression"},
      {"t2", "Enables textures mipmaps generation"},

      {"trr", "Enables textures resizing by ratio"},
      {"trrw", "The width ratio"},
      {"trrh", "The height ratio"},

      {"trs", "Enables textures resizing by fixed size"},
      {"trsw", "The width size"},
      {"trsh", "The height size"},

      {"a", "Enables animations processing"},
      {"mh", "Enables headparts detection and processing"},
      {"mr", "Enables meshes resaving"},
      {"be", "Enables BSA extraction."},
      {"bc", "Enables BSA creation."},
      {"bd", "Enables deletion of BSA backups."},
      /*{"bo",
       "NOT WORKING. Enables BSA optimization. The files inside the "
       "BSA will be extracted to memory and processed according to the provided
       settings "},*/
  });

  parser.process(args);

  const QStringList positionalArguments = parser.positionalArguments();
  if (positionalArguments.size() < 3)
    throw std::runtime_error("Not enough arguments");

  const QString path = QDir::cleanPath(positionalArguments.at(0));
  userPath = path;

  const QString readMode = positionalArguments.at(1);
  if (readMode == "om")
    mode = AssetWorkMode::SingleMod;
  else if (readMode == "sm")
    mode = AssetWorkMode::SeveralMods;
  else
    throw std::runtime_error("Invalid argument for mode");

  const QString readGame = positionalArguments.at(2);
  Profiles::setCurrentProfile(readGame);

  bDryRun = parser.isSet("dr");
  bDebugLog = parser.isSet("l");

  iMeshesOptimizationLevel = parser.value("m").toInt();
  bMeshesHeadparts = parser.isSet("mh");
  bMeshesResave = parser.isSet("mr");

  bTexturesNecessary = parser.isSet("t0");
  bTexturesCompress = parser.isSet("t1");
  bTexturesMipmaps = parser.isSet("t2");

  bTexturesResizeRatio = parser.isSet("trr");
  iTexturesTargetWidthRatio = parser.value("trrw").toUInt();
  iTexturesTargetHeightRatio = parser.value("trrh").toUInt();

  bTexturesResizeSize = parser.isSet("trs");
  iTexturesTargetWidth = parser.value("trsw").toUInt();
  iTexturesTargetHeight = parser.value("trsh").toUInt();

  bAnimationsOptimization = parser.isSet("a");

  bBsaExtract = parser.isSet("be");
  bBsaCreate = parser.isSet("bc");
  bBsaDeleteBackup = parser.isSet("bd");
  bBsaProcessContent = parser.isSet("bo");
}
