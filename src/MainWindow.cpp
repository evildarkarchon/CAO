/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "MainWindow.h"
#include "ApplicationLogging.h"
#include "GuiRunDispatch.h"
#include "Run/ApplicationRunSetup.h"
#include "Run/ApplicationRunWork.h"

#include <QStatusBar>
#include <limits>

MainWindow::MainWindow() : _ui(new Ui::MainWindow) {
    _ui->setupUi(this);
    setAcceptDrops(true);

    // Setting data for widgets

    // Profiles
    refreshProfiles();
    {
        // Mode chooser combo box
        _ui->modeChooserComboBox->setItemData(0, OptionsCAO::SingleMod);
        _ui->modeChooserComboBox->setItemData(1, OptionsCAO::SeveralMods);

        // Advanced BSA
        _ui->bsaGame->setItemData(0, QVariant::fromValue(btu::Game::SLE));
        _ui->bsaGame->setItemData(1, QVariant::fromValue(btu::Game::SSE));
        _ui->bsaGame->setItemData(2, QVariant::fromValue(btu::Game::FO4));

        // Advanced meshes
        _ui->meshesUser->setItemData(0, 11);
        _ui->meshesUser->setItemData(1, 12);

        _ui->meshesVersion->setItemData(0, nifly::V20_0_0_5);
        _ui->meshesVersion->setItemData(1, nifly::V20_2_0_7);

        _ui->meshesStream->setItemData(0, 82);
        _ui->meshesStream->setItemData(1, 83);
        _ui->meshesStream->setItemData(2, 100);
        _ui->meshesStream->setItemData(3, 130);

        _ui->texturesOutputFormat->setItemData(0, DXGI_FORMAT_BC7_UNORM);
        _ui->texturesOutputFormat->setItemData(1, DXGI_FORMAT_BC5_UNORM);
        _ui->texturesOutputFormat->setItemData(2, DXGI_FORMAT_BC3_UNORM);
        _ui->texturesOutputFormat->setItemData(3, DXGI_FORMAT_BC1_UNORM);
        _ui->texturesOutputFormat->setItemData(4, DXGI_FORMAT_R8G8B8A8_UNORM);
    }

    // Connecting widgets
    connect(_ui->dryRunCheckBox, &QCheckBox::clicked, this, [&](const bool& checked) {
        // Disabling BSA options if dry run is enabled
        _ui->bsaBaseGroupBox->setDisabled(checked);
        _ui->bsaExtractCheckBox->setDisabled(checked);
        _ui->bsaCreateCheckbox->setDisabled(checked);
        _ui->bsaDeleteBackupsCheckbox->setDisabled(checked);

        _ui->bsaExtractCheckBox->setChecked(false);
        _ui->bsaCreateCheckbox->setChecked(false);
        _ui->bsaDeleteBackupsCheckbox->setChecked(false);
    });

    connect(_ui->advancedSettingsCheckbox, &QCheckBox::clicked, this, [&](const bool& enabled) {
        this->showTutorialWindow(
            tr("Advanced settings"),
            tr("Advanced settings can only be modified when using custom profiles."));
        this->setAdvancedSettingsEnabled(enabled);
    });

    disconnect(_ui->presets, nullptr, nullptr, nullptr);  // resetting
    connect(_ui->presets, QOverload<int>::of(&QComboBox::activated), this,
            [&] { this->setGameMode(_ui->presets->currentText()); });

    connect(_ui->newProfilePushButton, &QPushButton::pressed, this, &MainWindow::createProfile);

    connect(_ui->modeChooserComboBox, QOverload<int>::of(&QComboBox::activated), this, [&] {
        const bool& severalModsEnabled =
            (_ui->modeChooserComboBox->currentData() == OptionsCAO::SeveralMods);

        // Disabling some meshes options when several mods mode is enabled
        _ui->meshesMediumOptimizationRadioButton->setDisabled(severalModsEnabled);
        _ui->meshesFullOptimizationRadioButton->setDisabled(severalModsEnabled);
        _ui->meshesNecessaryOptimizationRadioButton->setChecked(severalModsEnabled);

        if (severalModsEnabled) {
            this->showTutorialWindow(
                tr("Several mods option"),
                tr("You have selected the several mods option. This process may take a very long "
                   "time, "
                   "especially if you process BSA. ") +
                    '\n' +
                    tr("This process has only been tested on the Mod Organizer mods folder."));
        }
    });

    connect(_ui->userPathButton, &QPushButton::pressed, this, [&] {
        const QString& dir = QFileDialog::getExistingDirectory(
            this, tr("Open Directory"), _options.userPath,
            QFileDialog::ShowDirsOnly | QFileDialog::DontResolveSymlinks);
        if (!dir.isEmpty()) _ui->userPathTextEdit->setText(dir);
    });

    connect(_ui->processButton, &QPushButton::pressed, this, &MainWindow::initProcess);
    connect(&logTimer, &QTimer::timeout, this, &MainWindow::updateLog);

    texturesFormatDialog = new TexturesFormatSelectDialog(this);

    connect(_ui->texturesUnwantedFormatsEditButton, &QPushButton::pressed, this, [&] {
        QStringList unwantedFormats;
        for (int i = 0; i < _ui->texturesUnwantedFormatsList->count(); ++i)
            unwantedFormats << _ui->texturesUnwantedFormatsList->item(i)->text();
        _ui->texturesUnwantedFormatsList->clear();

        texturesFormatDialog->setCheckedItems(unwantedFormats);
        texturesFormatDialog->open();
    });

    connect(texturesFormatDialog, &QDialog::finished, this, [&] {
        for (const auto& itemText : texturesFormatDialog->getChoices()) {
            auto* item = new QListWidgetItem(itemText);
            item->setFlags(item->flags() & (~Qt::ItemIsUserCheckable));
            _ui->texturesUnwantedFormatsList->addItem(item);
        }
    });

    // Connecting menu buttons
    {
        connect(_ui->actionEnableDarkTheme, &QAction::triggered, this, &MainWindow::setDarkTheme);

        connect(_ui->actionShow_tutorials, &QAction::triggered, this,
                [this](const bool& checked) { this->_showTutorials = checked; });

        connect(_ui->actionEnable_debug_log, &QAction::triggered, this, [this] { this->saveUi(); });

        connect(_ui->actionOpen_log_file, &QAction::triggered, this, [] {
            QDesktopServices::openUrl(
                QUrl("file:///" + cao::application::configuredLogPath(), QUrl::TolerantMode));
        });

        connect(_ui->actionAbout, &QAction::triggered, this, [&] {
            QMessageBox::about(this, tr("About"),
                               QCoreApplication::applicationName() + ' ' +
                                   QCoreApplication::applicationVersion() +
                                   tr("\nMade by G'k\nThis program is distributed in the hope that "
                                      "it will be useful "
                                      "but WITHOUT ANY "
                                      "WARRANTLY. See the Mozilla Public License"));
        });
        connect(_ui->actionAbout_Qt, &QAction::triggered, this,
                [&] { QMessageBox::aboutQt(this); });

        connect(_ui->actionDocumentation, &QAction::triggered, this, [&] {
            QDesktopServices::openUrl(
                QUrl("https://www.nexusmods.com/skyrimspecialedition/mods/23316"));
        });

        connect(_ui->actionDiscord, &QAction::triggered, this,
                [&] { QDesktopServices::openUrl(QUrl("https://discordapp.com/invite/B9abN8d")); });
    }

    loadUi();

    // Loading remembered settings
    setGameMode(Profiles::currentProfile());

    firstStart();
}

void MainWindow::saveUi() {
    Profiles::commonSettings()->setValue("bShowAdvancedSettings",
                                         _ui->advancedSettingsCheckbox->isChecked());
    Profiles::commonSettings()->setValue("bDarkMode", _ui->actionEnableDarkTheme->isChecked());
    Profiles::commonSettings()->setValue("showTutorial", _showTutorials);

    if (_bLockVariables) return;

    _options.readFromUi(_ui);
    _options.saveToIni(Profiles::optionsSettings());
    Profiles::getInstance().readFromUi(_ui);
    Profiles::getInstance().saveToIni();
}

void MainWindow::loadUi() {
    setDarkTheme(Profiles::commonSettings()->value("bDarkMode").toBool());
    _ui->advancedSettingsCheckbox->setChecked(
        Profiles::commonSettings()->value("bShowAdvancedSettings").toBool());
    _ui->presets->setCurrentIndex(
        _ui->presets->findText(Profiles::commonSettings()->value("profile").toString()));
    _showTutorials = Profiles::commonSettings()->value("showTutorial", true).toBool();
    _ui->actionShow_tutorials->setChecked(_showTutorials);

    _options.readFromIni(Profiles::optionsSettings());
    _options.saveToUi(_ui);

    Profiles::getInstance().saveToUi(_ui);
}

void MainWindow::resetUi() const {
    // Resetting the window
    for (int i = 0; i < _ui->tabWidget->count(); ++i) _ui->tabWidget->setTabEnabled(i, true);

    _ui->meshesFullOptimizationRadioButton->show();
    _ui->meshesMediumOptimizationRadioButton->show();
}

void MainWindow::refreshProfiles() {
    _ui->presets->clear();
    _ui->presets->addItems(Profiles::list());
}

void MainWindow::createProfile() {
    showTutorialWindow(
        tr("New profile"),
        tr("You are about to create a new profile. It will create a new directory in "
           "'CAO/profiles'. "
           "Please check it out after creation, some files will be created inside it."));

    bool ok = false;
    const QString& text =
        QInputDialog::getText(this, tr("New profile"), tr("Name:"), QLineEdit::Normal, "", &ok);
    if (!ok || text.isEmpty()) return;

    // Choosing base profile

    QStringList profilesList;
    for (int i = 0; i < _ui->presets->count(); ++i) profilesList << _ui->presets->itemText(i);

    const QString& baseProfile = QInputDialog::getItem(
        this, tr("Base profile"), tr("Which profile do you want to use as a base?"), profilesList,
        _ui->presets->currentIndex(), false, &ok);

    if (!ok) return;

    Profiles::create(text, baseProfile);
    refreshProfiles();
    _ui->presets->setCurrentIndex(_ui->presets->findText(text));
    setGameMode(text);
}

void MainWindow::setDarkTheme(const bool& enabled) {
    _ui->actionEnableDarkTheme->setChecked(enabled);

    if (enabled) {
        QFile f(":qdarkstyle/style.qss");
        f.open(QFile::ReadOnly | QFile::Text);
        qApp->setStyleSheet(f.readAll());
        f.close();
    } else
        qApp->setStyleSheet("");
}

void MainWindow::initProcess() {
    if (_runView.state().active) {
        cancelRun();
        return;
    }
    if (!_runView.canStart()) return;
    saveUi();

    // saveUi() has just settled the profile and the debug-log toggle, so the run's log destination
    // and severity are only definitive from here on. This is a slot, so the redirect cannot be
    // allowed to escape as an exception.
    try {
        cao::application::applyRunLogging(Profiles::logPath(), _options.bDebugLog);
    } catch (const std::exception& e) {
        QMessageBox::critical(
            this, tr("Error"),
            tr("The log file for this run could not be opened: ") + QString(e.what()));
        return;
    }

    try {
        auto request = cao::run::makeApplicationRunRequest(_options);
        _runHandle.reset();
        _runService = std::make_unique<cao::run::OptimizationRunService>(
            cao::run::makeApplicationRunConfigurationProvider(),
            cao::run::makeApplicationRunWork(_options));
        auto observation =
            cao::gui::queuedObservation(this, [this](const cao::run::RunEvent& event) {
                const bool wasActive = _runView.state().active;
                if (!_runView.consume(event)) return;
                renderRun();
                if (wasActive && _runView.state().outcome) endProcess();
            });
        auto started = _runService->start(
            std::move(request), std::vector<cao::run::RunObservation>{std::move(observation)});
        if (!started.started()) {
            QMessageBox::critical(this, tr("Start Error"),
                                  tr("The run could not start (error %1).")
                                      .arg(static_cast<int>(*started.startError())));
            return;
        }
        _runHandle.emplace(std::move(*started.handle()));
        // Delivery is always queued, so the handle and run identity exist before any callback.
        _runView.begin(_runHandle->snapshot().runId());
        _renderedDetails = 0;
        _bLockVariables = true;
        _ui->processButton->setText(tr("Cancel"));
        _ui->tabWidget->setEnabled(false);
        _ui->presets->setEnabled(false);
        _ui->newProfilePushButton->setEnabled(false);
        logTimer.start(5000);  // Refresh log every 5 seconds
        renderRun();
        updateLog();
    } catch (const std::exception& e) {
        QMessageBox::critical(this, tr("Start Error"), QString::fromUtf8(e.what()));
    }
}

void MainWindow::endProcess() {
    logTimer.stop();
    if (_runView.state().closeRequested) {
        // Unwind terminal delivery before closing; keep controls locked so queued input cannot
        // start another run or mutate its dependencies in the meantime.
        QTimer::singleShot(0, this, [this] { close(); });
        return;
    }
    _ui->processButton->setDisabled(false);
    _ui->processButton->setText(tr("Run"));
    _ui->tabWidget->setEnabled(true);
    _ui->presets->setEnabled(true);
    _ui->newProfilePushButton->setEnabled(true);
    _bLockVariables = false;
    saveUi();
    updateLog();
}

void MainWindow::cancelRun() {
    if (!_runHandle || !_runView.state().active) return;
    _runHandle->requestCancellation();
    _runView.requestCancellation();
    _ui->processButton->setDisabled(true);
    renderRun();
}

void MainWindow::renderRun() {
    const auto& state = _runView.state();
    QString text = QString::fromStdString(state.label);
    if (state.progress) {
        const auto& progress = *state.progress;
        text += tr(" - %1 / %2 attempts (%3 succeeded, %4 failed)")
                    .arg(static_cast<qulonglong>(progress.completed()))
                    .arg(static_cast<qulonglong>(progress.total()))
                    .arg(static_cast<qulonglong>(progress.succeeded()))
                    .arg(static_cast<qulonglong>(progress.failed()));
        // Qt's bar uses int; large totals stay textual rather than appearing complete at INT_MAX.
        const auto limit = static_cast<std::size_t>((std::numeric_limits<int>::max)());
        if (progress.total() > limit) {
            _ui->progressBar->setRange(0, state.active ? 0 : 1);
            _ui->progressBar->setValue(0);
        } else {
            _ui->progressBar->setRange(
                0, progress.total() == 0 ? 1 : static_cast<int>(progress.total()));
            _ui->progressBar->setValue(static_cast<int>(progress.completed()));
        }
    } else {
        _ui->progressBar->setRange(0, state.active ? 0 : 1);
        _ui->progressBar->setValue(0);
    }
    _ui->progressBar->setFormat(text);
    // Some Qt styles suppress progress-bar text during its indeterminate animation.
    statusBar()->showMessage(text);
    if (_renderedDetails != state.details.size()) {
        _renderedDetails = state.details.size();
        updateLog();
    }
}

void MainWindow::updateLog() const {
    QFile log(cao::application::configuredLogPath());
    if (log.open(QFile::Text | QFile::ReadOnly)) {
        _ui->logTextEdit->clear();
        QTextStream ts(&log);
        ts.setCodec(QTextCodec::codecForName("UTF-8"));
        while (!ts.atEnd()) _ui->logTextEdit->appendHtml(ts.readLine());
    } else {
        _ui->logTextEdit->clear();
    }
    // Structured run evidence remains visible when the periodic legacy log refreshes.
    for (const auto& detail : _runView.state().details)
        _ui->logTextEdit->appendPlainText(QString::fromStdString(detail));
}

void MainWindow::setGameMode(const QString& mode) {
    saveUi();

    // Resetting the window
    resetUi();

    // Actually setting the window mode
    Profiles::setCurrentProfile(mode);
    Profiles::getInstance().saveToUi(_ui);
    loadUi();

    const int& animTabIndex = _ui->tabWidget->indexOf(_ui->AnimationsTab);
    const int& meshesTabIndex = _ui->tabWidget->indexOf(_ui->meshesTab);
    const int& bsaTabIndex = _ui->tabWidget->indexOf(_ui->bsaTab);
    const int& TexturesTabIndex = _ui->tabWidget->indexOf(_ui->texturesTab);

    _ui->tabWidget->setTabEnabled(animTabIndex, Profiles::animationsEnabled());
    _ui->tabWidget->setTabEnabled(meshesTabIndex, Profiles::meshesEnabled());
    _ui->tabWidget->setTabEnabled(bsaTabIndex, Profiles::bsaEnabled());
    _ui->tabWidget->setTabEnabled(TexturesTabIndex, Profiles::texturesEnabled());

    setAdvancedSettingsEnabled(_ui->advancedSettingsCheckbox->isChecked());
}

void MainWindow::setAdvancedSettingsEnabled(const bool& value) {
    QWidgetList advancedSettings = {_ui->bsaAdvancedGroupBox, _ui->meshesVeryAdvancedGroupBox,
                                    _ui->texturesAdvancedGroupBox, _ui->animationsAdvancedGroupBox};

    const bool readOnly = Profiles::isBaseProfile();
    for (auto& window : advancedSettings) {
        window->setVisible(value);
        window->setDisabled(readOnly);
    }
}

void MainWindow::closeEvent(QCloseEvent* event) {
    if (!_runView.requestClose()) {
        cancelRun();
        event->ignore();
        return;
    }
    saveUi();
    event->accept();
}

void MainWindow::dragEnterEvent(QDragEnterEvent* e) {
    if (e->mimeData()->hasUrls()) e->acceptProposedAction();
}

void MainWindow::dropEvent(QDropEvent* e) {
    const QString& fileName = e->mimeData()->urls().at(0).toLocalFile();
    QDir dir;
    if (dir.exists(fileName)) _ui->userPathTextEdit->setText(QDir::cleanPath(fileName));
}

void MainWindow::showTutorialWindow(const QString& title, const QString& text) {
    if (_showTutorials) QMessageBox::information(this, title, text);
}

void MainWindow::firstStart() {
    if (!Profiles::commonSettings()->value("notFirstStart").toBool()) {
        QMessageBox(
            QMessageBox::Information,
            tr("Welcome to %1 %2")
                .arg(QCoreApplication::applicationName(), QCoreApplication::applicationVersion()),
            tr("It appears you are running CAO for the first time. All options have tooltips "
               "explaining what "
               "they "
               "do. If you need help, you can also join us on Discord. A dark theme is also "
               "available."))
            .exec();

        Profiles::commonSettings()->setValue("notFirstStart", true);
    }
}

MainWindow::~MainWindow() {
    // Unexpected owner destruction still joins before releasing widgets borrowed by observers.
    _runHandle.reset();
    _runService.reset();
    delete _ui;
}
