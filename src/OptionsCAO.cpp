/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#include "OptionsCAO.h"

void OptionsCAO::saveToIni(QSettings* settings) {
    // General
    settings->setValue("bDryRun", bDryRun);
    settings->setValue("bDebugLog", bDebugLog);
    settings->setValue("mode", mode);
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
    settings->setValue("Meshes/iMeshesOptimizationLevel", iMeshesOptimizationLevel);

    // Meshes advanced
    settings->setValue("Meshes/bMeshesHeadparts", bMeshesHeadparts);
    settings->setValue("Meshes/bMeshesResave", bMeshesResave);

    // Animations
    settings->setValue("Animations/bAnimationsOptimization", bAnimationsOptimization);
}

void OptionsCAO::readFromIni(QSettings* settings) {
    if (!QFile(settings->fileName()).exists()) return;

    // General
    bDryRun = settings->value("bDryRun").toBool();
    bDebugLog = settings->value("bDebugLog").toBool();
    mode = settings->value("mode").value<OptimizationMode>();
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
    iTexturesTargetWidthRatio = settings->value("iTexturesTargetWidthRatio").toUInt();
    iTexturesTargetHeightRatio = settings->value("iTexturesTargetHeightRatio").toUInt();
    settings->endGroup();

    // Meshes
    iMeshesOptimizationLevel = settings->value("Meshes/iMeshesOptimizationLevel").toInt();

    // Meshes advanced
    bMeshesHeadparts = settings->value("Meshes/bMeshesHeadparts").toBool();
    bMeshesResave = settings->value("Meshes/bMeshesResave").toBool();

    // Animations
    bAnimationsOptimization = settings->value("Animations/bAnimationsOptimization").toBool();
}

#ifdef GUI
void OptionsCAO::saveToUi(Ui::MainWindow* ui) {
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
    const bool texturesOpt = bTexturesMipmaps || bTexturesCompress || bTexturesNecessary;
    if (!texturesOpt)
        ui->texturesGroupBox->setChecked(false);
    else {
        ui->texturesGroupBox->setChecked(true);
        ui->texturesNecessaryOptimizationCheckBox->setChecked(bTexturesNecessary);
        ui->texturesCompressCheckBox->setChecked(bTexturesCompress);
        ui->texturesMipmapCheckBox->setChecked(bTexturesMipmaps);
    }

    // Textures resizing
    ui->texturesResizingGroupBox->setChecked(bTexturesResizeSize || bTexturesResizeRatio);

    ui->texturesResizingBySizeRadioButton->setChecked(bTexturesResizeSize);
    ui->texturesResizingBySizeWidth->setValue(static_cast<int>(iTexturesTargetWidth));
    ui->texturesResizingBySizeHeight->setValue(static_cast<int>(iTexturesTargetHeight));

    ui->texturesResizingByRatioRadioButton->setChecked(bTexturesResizeRatio);
    ui->texturesResizingByRatioWidth->setValue(static_cast<int>(iTexturesTargetWidthRatio));
    ui->texturesResizingByRatioHeight->setValue(static_cast<int>(iTexturesTargetHeightRatio));

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
    ui->animationsNecessaryOptimizationCheckBox->setChecked(bAnimationsOptimization);

    // Log level
    ui->actionEnable_debug_log->setChecked(bDebugLog);

    // General and GUI
    ui->dryRunCheckBox->setChecked(bDryRun);
    ui->modeChooserComboBox->setCurrentIndex(ui->modeChooserComboBox->findData(mode));
    ui->userPathTextEdit->setText(userPath);
}

void OptionsCAO::readFromUi(Ui::MainWindow* ui) {
    // BSA
    const bool bsaEnabled = ui->bsaTab->isEnabled() && ui->bsaBaseGroupBox->isEnabled();
    bBsaExtract = bsaEnabled && ui->bsaExtractCheckBox->isChecked();
    bBsaCreate = bsaEnabled && ui->bsaCreateCheckbox->isChecked();
    bBsaDeleteBackup = bsaEnabled && ui->bsaDeleteBackupsCheckbox->isChecked();
    bBsaMergeIncomp = bsaEnabled && !ui->bBsaCreateIncompressible->isChecked();
    bBsaMergeTexture = bsaEnabled && !ui->bBsaCreateTexture->isChecked();
    bBsaCreateDummies = bsaEnabled && ui->bsaCreateDummiesCheckbox->isChecked();
    bBsaCompress = bsaEnabled && ui->bsaCompressBsaCheckbox->isChecked();
    bBsaDeleteSource = bsaEnabled && ui->bsaDeleteSourceCheckbox->isChecked();

    // Textures
    const bool texturesEnabled = ui->texturesGroupBox->isChecked() && ui->texturesTab->isEnabled();
    bTexturesNecessary = texturesEnabled && ui->texturesNecessaryOptimizationCheckBox->isChecked();
    bTexturesMipmaps = texturesEnabled && ui->texturesMipmapCheckBox->isChecked();
    bTexturesCompress = texturesEnabled && ui->texturesCompressCheckBox->isChecked();

    // Textures resizing
    const bool texturesResizing =
        ui->texturesResizingGroupBox->isChecked() && ui->texturesTab->isEnabled();
    bTexturesResizeSize = ui->texturesResizingBySizeRadioButton->isChecked() && texturesResizing;
    iTexturesTargetWidth = static_cast<size_t>(ui->texturesResizingBySizeWidth->value());
    iTexturesTargetHeight = static_cast<size_t>(ui->texturesResizingBySizeHeight->value());

    bTexturesResizeRatio = ui->texturesResizingByRatioRadioButton->isChecked() && texturesResizing;
    iTexturesTargetWidthRatio = static_cast<size_t>(ui->texturesResizingByRatioWidth->value());
    iTexturesTargetHeightRatio = static_cast<size_t>(ui->texturesResizingByRatioHeight->value());

    // Meshes base
    const bool meshesEnabled = ui->meshesTab->isEnabled();
    if (ui->meshesNecessaryOptimizationRadioButton->isChecked())
        iMeshesOptimizationLevel = 1;
    else if (ui->meshesMediumOptimizationRadioButton->isChecked())
        iMeshesOptimizationLevel = 2;
    else if (ui->meshesFullOptimizationRadioButton->isChecked())
        iMeshesOptimizationLevel = 3;
    if (!ui->meshesGroupBox->isChecked() || !meshesEnabled) iMeshesOptimizationLevel = 0;

    // Meshes advanced
    bMeshesHeadparts = meshesEnabled && ui->meshesHeadpartsCheckBox->isChecked();
    bMeshesResave = meshesEnabled && ui->meshesResaveCheckBox->isChecked();

    // Animations
    bAnimationsOptimization =
        ui->AnimationsTab->isEnabled() && ui->animationsNecessaryOptimizationCheckBox->isChecked();

    // General
    bDryRun = ui->dryRunCheckBox->isChecked();
    userPath = QDir::cleanPath(ui->userPathTextEdit->text());
    mode = ui->modeChooserComboBox->currentData().value<OptimizationMode>();
    bDebugLog = ui->actionEnable_debug_log->isChecked();
}
#endif

namespace {
/// Reads one unsigned command line dimension and rejects values Qt would silently coerce to zero.
/// The ratio and size fields differ in width, so the caller widens the returned uint as needed.
uint readDimension(const QCommandLineParser& parser, const QString& name) {
    bool ok = false;
    const auto value = parser.value(name).toUInt(&ok);
    if (!ok) {
        throw std::runtime_error("Invalid value for -" + name.toStdString() + ": '" +
                                 parser.value(name).toStdString() + "'");
    }
    return value;
}
}  // namespace

void OptionsCAO::parseArguments(const QStringList& args) {
    QCommandLineParser parser;

    parser.addHelpOption();

    parser.addPositionalArgument("folder", "The folder to process, surrounded with quotes.");
    parser.addPositionalArgument("mode", "Either om (one mod) or sm (several mods)");
    parser.addPositionalArgument("profile", "One of the profile located in CAO/profiles");

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
        // These must declare a value name, otherwise QCommandLineParser treats them as boolean
        // switches and parser.value() returns an empty string that converts to zero. A ratio
        // default of 1 keeps --trr without explicit ratios a no-op instead of a division by zero.
        {"trrw", "The width ratio", "value", "1"},
        {"trrh", "The height ratio", "value", "1"},

        {"trs", "Enables textures resizing by fixed size"},
        {"trsw", "The width size", "value", "0"},
        {"trsh", "The height size", "value", "0"},

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

    // process() owns --help, --version, and malformed-option reporting, so positional validation
    // has to run after it. Checking args.count() first would reject "--help" before Qt could print
    // usage, and would accept three flag-only arguments that carry no positional value at all.
    parser.process(args);

    const QStringList positionalArguments = parser.positionalArguments();
    if (positionalArguments.count() != 3) {
        throw std::runtime_error(
            "Expected exactly three positional arguments: folder, mode and profile.");
    }

    const QString& path = QDir::cleanPath(positionalArguments.at(0));
    userPath = path;

    const QString readMode = positionalArguments.at(1);
    if (readMode == "om")
        mode = SingleMod;
    else if (readMode == "sm")
        mode = SeveralMods;
    else
        throw std::runtime_error("Invalid argument for mode");

    // Profiles::loadProfile silently substitutes the default profile for an unknown name, which is
    // the right behaviour for persisted GUI settings but would let a typo here process a mod with
    // an entirely different game's texture, mesh, and archive settings.
    const QString& readGame = positionalArguments.at(2);
    if (!Profiles::exists(readGame))
        throw std::runtime_error("This profile does not exist: " + readGame.toStdString());
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
    iTexturesTargetWidthRatio = readDimension(parser, "trrw");
    iTexturesTargetHeightRatio = readDimension(parser, "trrh");

    bTexturesResizeSize = parser.isSet("trs");
    iTexturesTargetWidth = readDimension(parser, "trsw");
    iTexturesTargetHeight = readDimension(parser, "trsh");

    bAnimationsOptimization = parser.isSet("a");

    bBsaExtract = parser.isSet("be");
    bBsaCreate = parser.isSet("bc");
    bBsaDeleteBackup = parser.isSet("bd");
    bBsaProcessContent = parser.isSet("bo");
}

QString OptionsCAO::isValid() const {
    if (!QDir(userPath).exists() || userPath.size() < 5)
        return ("This path does not exist or is shorter than 5 characters. Path: '" + userPath +
                "'");

    if (mode != SingleMod && mode != SeveralMods) return "This mode does not exist.";

    if (iMeshesOptimizationLevel < 0 || iMeshesOptimizationLevel > 3)
        return ("This meshes optimization level does not exist. Level: " +
                QString::number(iMeshesOptimizationLevel));

    if (iTexturesTargetWidth % 2 != 0 || iTexturesTargetHeight % 2 != 0)
        return ("Textures target size has to be a power of two");

    // A zero ratio divides the source dimensions in MainOptimizer::optimizeTexture, and a zero
    // target size silently requests an empty texture, so both are rejected before a run starts.
    if (bTexturesResizeRatio && (iTexturesTargetWidthRatio == 0 || iTexturesTargetHeightRatio == 0))
        return "Textures resizing by ratio requires non-zero width and height ratios.";

    if (bTexturesResizeSize && (iTexturesTargetWidth == 0 || iTexturesTargetHeight == 0))
        return "Textures resizing by size requires non-zero width and height.";

    return QString();
}
