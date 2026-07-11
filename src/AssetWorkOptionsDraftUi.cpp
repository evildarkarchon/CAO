/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#include "AssetWorkOptionsDraft.h"
#include "AssetWorkOptionsUiState.h"
#include "Profiles.h"
#include "ui_mainWindow.h"

#ifdef GUI
namespace {
AssetWorkOptionsUiContext contextFromUi(Ui::MainWindow *ui) {
  return AssetWorkOptionsUiContext{
      .bsaAvailable = Profiles::bsaEnabled(),
      .meshesAvailable = Profiles::meshesEnabled(),
      .animationsAvailable = Profiles::animationsEnabled(),
      .texturesAvailable = Profiles::texturesEnabled(),
      .advancedSettingsVisible = ui->advancedSettingsCheckbox->isChecked(),
      .advancedSettingsEditable = !Profiles::isBaseProfile()};
}

void setTabEnabled(Ui::MainWindow *ui, QWidget *tab, const bool enabled) {
  ui->tabWidget->setTabEnabled(ui->tabWidget->indexOf(tab), enabled);
}

AssetWorkOptionsUiState stateFromUi(Ui::MainWindow *ui) {
  AssetWorkOptionsUiState state;

  state.dryRun = ui->dryRunCheckBox->isChecked();
  state.debugLog = ui->actionEnable_debug_log->isChecked();
  state.userPath = QDir::cleanPath(ui->userPathTextEdit->text());
  state.mode = static_cast<AssetWorkMode>(
      ui->modeChooserComboBox->currentData().toInt());

  state.archive.tabEnabled = ui->bsaTab->isEnabled();
  state.archive.controlsEnabled = ui->bsaBaseGroupBox->isEnabled();
  state.archive.extract = ui->bsaExtractCheckBox->isChecked();
  state.archive.create = ui->bsaCreateCheckbox->isChecked();
  state.archive.deleteBackup = ui->bsaDeleteBackupsCheckbox->isChecked();
  state.archive.mergeIncompressible =
      !ui->bBsaCreateIncompressible->isChecked();
  state.archive.mergeTextures = !ui->bBsaCreateTexture->isChecked();
  state.archive.createDummies = ui->bsaCreateDummiesCheckbox->isChecked();
  state.archive.compress = ui->bsaCompressBsaCheckbox->isChecked();
  state.archive.deleteSource = ui->bsaDeleteSourceCheckbox->isChecked();

  state.textures.tabEnabled = ui->texturesTab->isEnabled();
  state.textures.enabled = ui->texturesGroupBox->isChecked();
  state.textures.necessary =
      ui->texturesNecessaryOptimizationCheckBox->isChecked();
  state.textures.compress = ui->texturesCompressCheckBox->isChecked();
  state.textures.mipmaps = ui->texturesMipmapCheckBox->isChecked();
  state.textures.resizingEnabled = ui->texturesResizingGroupBox->isChecked();
  state.textures.resizeBySize =
      ui->texturesResizingBySizeRadioButton->isChecked();
  state.textures.targetWidth =
      static_cast<size_t>(ui->texturesResizingBySizeWidth->value());
  state.textures.targetHeight =
      static_cast<size_t>(ui->texturesResizingBySizeHeight->value());
  state.textures.resizeByRatio =
      ui->texturesResizingByRatioRadioButton->isChecked();
  state.textures.targetWidthRatio =
      static_cast<uint>(ui->texturesResizingByRatioWidth->value());
  state.textures.targetHeightRatio =
      static_cast<uint>(ui->texturesResizingByRatioHeight->value());

  state.meshes.tabEnabled = ui->meshesTab->isEnabled();
  state.meshes.optimizationEnabled = ui->meshesGroupBox->isChecked();
  if (ui->meshesNecessaryOptimizationRadioButton->isChecked())
    state.meshes.optimizationLevel = 1;
  else if (ui->meshesMediumOptimizationRadioButton->isChecked())
    state.meshes.optimizationLevel = 2;
  else if (ui->meshesFullOptimizationRadioButton->isChecked())
    state.meshes.optimizationLevel = 3;
  state.meshes.mediumAndFullOptimizationEnabled =
      ui->meshesMediumOptimizationRadioButton->isEnabled() &&
      ui->meshesFullOptimizationRadioButton->isEnabled();
  state.meshes.processHeadparts = ui->meshesHeadpartsCheckBox->isChecked();
  state.meshes.resave = ui->meshesResaveCheckBox->isChecked();

  state.animations.tabEnabled = ui->AnimationsTab->isEnabled();
  state.animations.optimize =
      ui->animationsNecessaryOptimizationCheckBox->isChecked();

  state.advanced.visible = ui->bsaAdvancedGroupBox->isVisible();
  state.advanced.editable = ui->bsaAdvancedGroupBox->isEnabled();

  return state;
}

void applyStateToUi(Ui::MainWindow *ui, const AssetWorkOptionsUiState &state) {
  setTabEnabled(ui, ui->AnimationsTab, state.animations.tabEnabled);
  setTabEnabled(ui, ui->meshesTab, state.meshes.tabEnabled);
  setTabEnabled(ui, ui->bsaTab, state.archive.tabEnabled);
  setTabEnabled(ui, ui->texturesTab, state.textures.tabEnabled);

  ui->dryRunCheckBox->setChecked(state.dryRun);
  ui->actionEnable_debug_log->setChecked(state.debugLog);
  ui->modeChooserComboBox->setCurrentIndex(
      ui->modeChooserComboBox->findData(static_cast<int>(state.mode)));
  ui->userPathTextEdit->setText(state.userPath);

  ui->bsaBaseGroupBox->setEnabled(state.archive.controlsEnabled);
  ui->bsaExtractCheckBox->setEnabled(state.archive.controlsEnabled);
  ui->bsaCreateCheckbox->setEnabled(state.archive.controlsEnabled);
  ui->bsaDeleteBackupsCheckbox->setEnabled(state.archive.controlsEnabled);
  ui->bsaExtractCheckBox->setChecked(state.archive.extract);
  ui->bsaCreateCheckbox->setChecked(state.archive.create);
  ui->bsaDeleteBackupsCheckbox->setChecked(state.archive.deleteBackup);
  ui->bBsaCreateIncompressible->setChecked(!state.archive.mergeIncompressible);
  ui->bBsaCreateTexture->setChecked(!state.archive.mergeTextures);
  ui->bsaCreateDummiesCheckbox->setChecked(state.archive.createDummies);
  ui->bsaCompressBsaCheckbox->setChecked(state.archive.compress);
  ui->bsaDeleteSourceCheckbox->setChecked(state.archive.deleteSource);

  ui->texturesGroupBox->setChecked(state.textures.enabled);
  ui->texturesNecessaryOptimizationCheckBox->setChecked(
      state.textures.necessary);
  ui->texturesCompressCheckBox->setChecked(state.textures.compress);
  ui->texturesMipmapCheckBox->setChecked(state.textures.mipmaps);
  ui->texturesResizingGroupBox->setChecked(state.textures.resizingEnabled);
  ui->texturesResizingBySizeRadioButton->setChecked(
      state.textures.resizeBySize);
  ui->texturesResizingBySizeWidth->setValue(
      static_cast<int>(state.textures.targetWidth));
  ui->texturesResizingBySizeHeight->setValue(
      static_cast<int>(state.textures.targetHeight));
  ui->texturesResizingByRatioRadioButton->setChecked(
      state.textures.resizeByRatio);
  ui->texturesResizingByRatioWidth->setValue(
      static_cast<int>(state.textures.targetWidthRatio));
  ui->texturesResizingByRatioHeight->setValue(
      static_cast<int>(state.textures.targetHeightRatio));

  ui->meshesGroupBox->setChecked(state.meshes.optimizationEnabled);
  ui->meshesNecessaryOptimizationRadioButton->setChecked(
      state.meshes.optimizationLevel == 1);
  ui->meshesMediumOptimizationRadioButton->setChecked(
      state.meshes.optimizationLevel == 2);
  ui->meshesFullOptimizationRadioButton->setChecked(
      state.meshes.optimizationLevel == 3);
  ui->meshesMediumOptimizationRadioButton->setEnabled(
      state.meshes.mediumAndFullOptimizationEnabled);
  ui->meshesFullOptimizationRadioButton->setEnabled(
      state.meshes.mediumAndFullOptimizationEnabled);
  ui->meshesResaveCheckBox->setChecked(state.meshes.resave);
  ui->meshesHeadpartsCheckBox->setChecked(state.meshes.processHeadparts);

  ui->animationsNecessaryOptimizationCheckBox->setChecked(
      state.animations.optimize);

  QWidgetList advancedSettings = {
      ui->bsaAdvancedGroupBox, ui->meshesVeryAdvancedGroupBox,
      ui->texturesAdvancedGroupBox, ui->animationsAdvancedGroupBox};
  for (auto *window : advancedSettings) {
    window->setVisible(state.advanced.visible);
    window->setDisabled(!state.advanced.editable);
  }
}
} // namespace

void AssetWorkOptionsDraft::saveToUi(Ui::MainWindow *ui) {
  applyStateToUi(ui, AssetWorkOptionsUi::present(*this, contextFromUi(ui)));
}

void AssetWorkOptionsDraft::readFromUi(Ui::MainWindow *ui) {
  AssetWorkOptionsUi::capture(stateFromUi(ui), *this);
}
#endif
