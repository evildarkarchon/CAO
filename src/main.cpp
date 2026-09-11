/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "Version.h"
#include "ApplicationLogging.h"
#ifdef GUI
#include "MainWindow.h"
#endif
#include "Run/ApplicationRunSetup.h"
#ifndef GUI
#include "CliRun.h"
#include "Run/ApplicationRunWork.h"
#endif

/// Presents bootstrap failures through the active application surface and existing logger.
void displayError(const std::string& err) {
#ifdef GUI
    QMessageBox box(QMessageBox::Critical, "Unknown error", QString::fromStdString(err));
    box.exec();
#else
    std::cerr << err << std::endl;
#endif

    PLOG_FATAL << err;
}

/// Collects application intent and keeps the CLI run alive through cooperative cancellation.
int main(int argc, char* argv[]) {
#ifdef GUI
    QApplication app(argc, argv);
#else
    QCoreApplication app(argc, argv);
#endif

    QCoreApplication::setApplicationName("Cathedral Assets Optimizer");
    QCoreApplication::setApplicationVersion(CAO_VERSION);

    QTranslator qtTranslator;
    qtTranslator.load(QLocale(), "qt", "_", "translations");
    QCoreApplication::installTranslator(&qtTranslator);

    QTranslator AssetsOptTranslator;
    qtTranslator.load(QLocale(), "AssetsOpt", "_", "translations");
    QCoreApplication::installTranslator(&AssetsOptTranslator);

    try {
        OptionsCAO options;
#ifdef GUI
        options.readFromIni(Profiles::optionsSettings());
#else
        options.parseArguments(QCoreApplication::arguments());
#endif
        cao::application::configureLogging(Profiles::logPath(), options.bDebugLog);

#ifdef GUI
        MainWindow* window = new MainWindow;
        window->show();
#else
        const cao::cli::ConsoleInterrupt interruption;
        cao::run::OptimizationRunService service(
            cao::run::makeApplicationRunConfigurationProvider(),
            cao::run::makeApplicationRunWork(options));
        // Standard output has process lifetime; the observer owns its stream reference until join.
        auto output = std::shared_ptr<std::ostream>(&std::cout, [](std::ostream*) {
            // The C++ runtime owns standard output; the run must not delete it.
        });
        return cao::cli::run(service, cao::run::makeApplicationRunRequest(options),
                             std::move(output), [&] { return interruption.requested(); });
#endif
    } catch (const std::exception& e) {
        displayError(e.what());
#ifdef GUI
        return 1;
#else
        return 2;
#endif
    }
#ifdef GUI
    return QApplication::exec();
#else
    return 0;
#endif
}
