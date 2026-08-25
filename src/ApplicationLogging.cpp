/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "ApplicationLogging.h"

#include "Logger.h"

#include <plog/Initializers/RollingFileInitializer.h>

#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QString>

#include <stdexcept>

namespace {
QString applicationLogPath;
}

namespace cao::application {
void configureLogging(const QString& logPath, const bool debugLog) {
    // A defensive repeat must not redirect or reconfigure the process-wide logger.
    if (plog::get()) return;

    // Creating log folder
    const QDir dir;
    dir.mkpath(QFileInfo(logPath).path());

    // Creating log file
    QFile file(logPath);

    if (!file.open(QFile::ReadWrite | QFile::Append))
        throw std::runtime_error("Cannot open log file: " + logPath.toStdString());

    static plog::RollingFileAppender<plog::CustomDebugFormatter> debugAppender(qPrintable(logPath),
                                                                               250'000, 1'000);

    static plog::RollingFileAppender<plog::CustomInfoFormatter> infoAppender(qPrintable(logPath),
                                                                             250'000, 1'000);

    if (debugLog)
        plog::init(plog::Severity::verbose, &debugAppender);
    else
        plog::init(plog::Severity::info, &infoAppender);

    applicationLogPath = logPath;
}

QString configuredLogPath() { return applicationLogPath; }
}  // namespace cao::application
