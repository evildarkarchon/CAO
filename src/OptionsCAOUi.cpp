/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#include "OptionsCAO.h"

#ifdef GUI
void OptionsCAO::saveToUi(Ui::MainWindow *ui) {
  // BSA
  ui->bsaExtractCheckBox->setChecked(bBsaExtract);
  ui->bsaCreateCheckbox->setChecked(bBsaCreate);
  ui->bsaDeleteBackupsCheckbox->setChecked(bBsaDeleteBackup);
  ui->bBsaCreateIncompressible->setChecked(!bBsaMergeIncomp);
  ui->bBsaCreateTexture->setChecked(!bBsaMergeTexture);
  ui->bsaCreateDummiesCheckbox->setChecked(bBsaCreateDummies);
  ui->bsaCompressBsaCheckbox->setChecked(bBsaCompress);
  ui->bsaDeleteSourceCheckbox->setChecked(bBsaDeleteSource);

  // Textures
  const bool texturesOpt =
      bTexturesMipmaps || bTexturesCompress || bTexturesNecessary;
  if (!texturesOpt)
    ui->texturesGroupBox->setChecked(false);
  else {
    ui->texturesGroupBox->setChecked(true);
    ui->texturesNecessaryOptimizationCheckBox->setChecked(bTexturesNecessary);
    ui->texturesCompressCheckBox->setChecked(bTexturesCompress);
    ui->texturesMipmapCheckBox->setChecked(bTexturesMipmaps);
  }

  // Textures resizing
  ui->texturesResizingGroupBox->setChecked(bTexturesResizeSize ||
                                           bTexturesResizeRatio);

  ui->texturesResizingBySizeRadioButton->setChecked(bTexturesResizeSize);
  ui->texturesResizingBySizeWidth->setValue(
      static_cast<int>(iTexturesTargetWidth));
  ui->texturesResizingBySizeHeight->setValue(
      static_cast<int>(iTexturesTargetHeight));

  ui->texturesResizingByRatioRadioButton->setChecked(bTexturesResizeRatio);
  ui->texturesResizingByRatioWidth->setValue(
      static_cast<int>(iTexturesTargetWidthRatio));
  ui->texturesResizingByRatioHeight->setValue(
      static_cast<int>(iTexturesTargetHeightRatio));

  // Meshes

  ui->meshesGroupBox->setChecked(true);
  switch (iMeshesOptimizationLevel) {
  case 0:
    ui->meshesGroupBox->setChecked(false);
    break;
  case 1:
    ui->meshesNecessaryOptimizationRadioButton->setChecked(true);
    break;
  case 2:
    ui->meshesMediumOptimizationRadioButton->setChecked(true);
    break;
  case 3:
    ui->meshesFullOptimizationRadioButton->setChecked(true);
    break;
  }

  ui->meshesResaveCheckBox->setChecked(bMeshesResave);
  ui->meshesHeadpartsCheckBox->setChecked(bMeshesHeadparts);

  // Animations
  ui->animationsNecessaryOptimizationCheckBox->setChecked(
      bAnimationsOptimization);

  // Log level
  ui->actionEnable_debug_log->setChecked(bDebugLog);

  // General and GUI
  ui->dryRunCheckBox->setChecked(bDryRun);
  ui->modeChooserComboBox->setCurrentIndex(
      ui->modeChooserComboBox->findData(mode));
  ui->userPathTextEdit->setText(userPath);
}

void OptionsCAO::readFromUi(Ui::MainWindow *ui) {
  // BSA
  const bool bsaEnabled =
      ui->bsaTab->isEnabled() && ui->bsaBaseGroupBox->isEnabled();
  bBsaExtract = bsaEnabled && ui->bsaExtractCheckBox->isChecked();
  bBsaCreate = bsaEnabled && ui->bsaCreateCheckbox->isChecked();
  bBsaDeleteBackup = bsaEnabled && ui->bsaDeleteBackupsCheckbox->isChecked();
  bBsaMergeIncomp = bsaEnabled && !ui->bBsaCreateIncompressible->isChecked();
  bBsaMergeTexture = bsaEnabled && !ui->bBsaCreateTexture->isChecked();
  bBsaCreateDummies = bsaEnabled && ui->bsaCreateDummiesCheckbox->isChecked();
  bBsaCompress = bsaEnabled && ui->bsaCompressBsaCheckbox->isChecked();
  bBsaDeleteSource = bsaEnabled && ui->bsaDeleteSourceCheckbox->isChecked();

  // Textures
  const bool texturesEnabled =
      ui->texturesGroupBox->isChecked() && ui->texturesTab->isEnabled();
  bTexturesNecessary =
      texturesEnabled && ui->texturesNecessaryOptimizationCheckBox->isChecked();
  bTexturesMipmaps = texturesEnabled && ui->texturesMipmapCheckBox->isChecked();
  bTexturesCompress =
      texturesEnabled && ui->texturesCompressCheckBox->isChecked();

  // Textures resizing
  const bool texturesResizing =
      ui->texturesResizingGroupBox->isChecked() && ui->texturesTab->isEnabled();
  bTexturesResizeSize =
      ui->texturesResizingBySizeRadioButton->isChecked() && texturesResizing;
  iTexturesTargetWidth =
      static_cast<size_t>(ui->texturesResizingBySizeWidth->value());
  iTexturesTargetHeight =
      static_cast<size_t>(ui->texturesResizingBySizeHeight->value());

  bTexturesResizeRatio =
      ui->texturesResizingByRatioRadioButton->isChecked() && texturesResizing;
  iTexturesTargetWidthRatio =
      static_cast<size_t>(ui->texturesResizingByRatioWidth->value());
  iTexturesTargetHeightRatio =
      static_cast<size_t>(ui->texturesResizingByRatioHeight->value());

  // Meshes base
  const bool meshesEnabled = ui->meshesTab->isEnabled();
  if (ui->meshesNecessaryOptimizationRadioButton->isChecked())
    iMeshesOptimizationLevel = 1;
  else if (ui->meshesMediumOptimizationRadioButton->isChecked())
    iMeshesOptimizationLevel = 2;
  else if (ui->meshesFullOptimizationRadioButton->isChecked())
    iMeshesOptimizationLevel = 3;
  if (!ui->meshesGroupBox->isChecked() || !meshesEnabled)
    iMeshesOptimizationLevel = 0;

  // Meshes advanced
  bMeshesHeadparts = meshesEnabled && ui->meshesHeadpartsCheckBox->isChecked();
  bMeshesResave = meshesEnabled && ui->meshesResaveCheckBox->isChecked();

  // Animations
  bAnimationsOptimization =
      ui->AnimationsTab->isEnabled() &&
      ui->animationsNecessaryOptimizationCheckBox->isChecked();

  // General
  bDryRun = ui->dryRunCheckBox->isChecked();
  userPath = QDir::cleanPath(ui->userPathTextEdit->text());
  mode = ui->modeChooserComboBox->currentData().value<OptimizationMode>();
  bDebugLog = ui->actionEnable_debug_log->isChecked();
}
#endif
