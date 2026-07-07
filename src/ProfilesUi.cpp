/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#include "Profiles.h"

#ifdef GUI
void Profiles::loadProfile(Ui::MainWindow *ui) {
  _currentProfile = uiToGame(ui);
  saveToUi(ui);
  loadProfile(_currentProfile);
}

QString Profiles::uiToGame(Ui::MainWindow *ui) {
  return ui->presets->currentText();
}

void Profiles::saveToUi(Ui::MainWindow *ui) {
  const auto iterateComboBox = [](QComboBox *box, const QVariant data) {
    for (int i = 0; i < box->count(); ++i) {
      if (box->itemData(i) == data) {
        box->setCurrentIndex(i);
        break;
      }
    }
  };

  ui->bsaMaximumSize->setValue(_maxBsaUncompressedSize / GigaByte);
  iterateComboBox(ui->bsaGame, QVariant::fromValue(_bsaGame));

  iterateComboBox(ui->meshesUser, _meshesUser);
  iterateComboBox(ui->meshesStream, _meshesStream);
  iterateComboBox(ui->meshesVersion, _meshesFileVersion);

  // Animation format is not working when converting from amd64, thus not added
  // to UI

  iterateComboBox(ui->texturesOutputFormat, _texturesFormat);
  ui->texturesTgaConversionCheckBox->setChecked(_texturesConvertTga);
  ui->texturesCompressInterfaceCheckBox->setChecked(_texturesCompressInterface);

  QStringList unwantedFormats;
  ui->texturesUnwantedFormatsList->clear();
  for (const QVariant &variant : _texturesUnwantedFormats) {
    const DXGI_FORMAT &format = variant.value<DXGI_FORMAT>();
    ui->texturesUnwantedFormatsList->addItem(dxgiFormatToString(format));
  }
}

void Profiles::readFromUi(Ui::MainWindow *ui) {
  _bsaGame = static_cast<btu::Game>(ui->bsaGame->currentData().toInt());
  _maxBsaUncompressedSize = ui->bsaMaximumSize->value() * GigaByte;

  _meshesUser = ui->meshesUser->currentData().toUInt();
  _meshesStream = ui->meshesStream->currentData().toUInt();
  _meshesFileVersion =
      ui->meshesVersion->currentData().value<nifly::NiFileVersion>();
  // Animation format is not working currently, thus not added to UI

  _texturesFormat =
      ui->texturesOutputFormat->currentData().value<DXGI_FORMAT>();
  _texturesConvertTga = ui->texturesTgaConversionCheckBox->isChecked();
  _texturesCompressInterface =
      ui->texturesCompressInterfaceCheckBox->isChecked();

  _texturesUnwantedFormats.clear();
  for (int i = 0; i < ui->texturesUnwantedFormatsList->count(); ++i) {
    const auto &entry = ui->texturesUnwantedFormatsList->item(i);
    const DXGI_FORMAT &format = stringToDxgiFormat(entry->text());
    if (!_texturesUnwantedFormats.contains(format) &&
        format != DXGI_FORMAT_UNKNOWN)
      _texturesUnwantedFormats += format;
  }
}
#endif
