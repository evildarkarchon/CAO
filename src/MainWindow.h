/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "GuiRun.h"
#include "OptionsCAO.h"
#include "Run/OptimizationRunService.h"
#include "TexturesFormatSelectDialog.h"
#include "pch.h"
#include "ui_mainWindow.h"

namespace Ui {
class MainWindow;
}

class MainWindow final : public QMainWindow {
    Q_DECLARE_TR_FUNCTIONS(MainWindow)

   public:
    MainWindow();
    /// Joins any retained run before releasing presentation widgets.
    ~MainWindow();

   private:
    Ui::MainWindow* _ui;

    bool _bLockVariables = false;

    void saveUi();
    void loadUi();
    void refreshProfiles();
    void createProfile();

    void setDarkTheme(const bool& enabled);

    void resetUi() const;

    void setGameMode(const QString& mode);

    void showTutorialWindow(const QString& title, const QString& text);

    /// Refreshes the legacy log while retaining structured observations as plain text.
    void updateLog() const;
    /// Captures user intent and retains a run whose observations are queued to this window.
    void initProcess();
    /// Restores controls after terminal delivery, then completes any deferred close.
    void endProcess();
    /// Renders authoritative phase counts and terminal labels without calculating run progress.
    void renderRun();
    /// Requests cooperative cancellation and leaves the window alive for terminal delivery.
    void cancelRun();

    void setAdvancedSettingsEnabled(const bool& value);

    /// Defers destruction of an active run until its queued terminal observation is rendered.
    void closeEvent(QCloseEvent* event) override;
    void dragEnterEvent(QDragEnterEvent* e);
    void dropEvent(QDropEvent* e);

    void firstStart();

    OptionsCAO _options;
    std::unique_ptr<cao::run::OptimizationRunService> _runService;
    std::optional<cao::run::RunHandle> _runHandle;
    cao::gui::RunViewModel _runView;
    std::size_t _renderedDetails{};
    bool _showTutorials;
    TexturesFormatSelectDialog* texturesFormatDialog;
    QTimer logTimer;
};
