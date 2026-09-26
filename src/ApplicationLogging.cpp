/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "ApplicationLogging.h"

#include "Logger.h"

#include <plog/Appenders/RollingFileAppender.h>
#include <plog/Init.h>

#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QString>

#include <cstddef>
#include <memory>
#include <stdexcept>

namespace {
constexpr size_t MaxLogFileSize = 250'000;
constexpr int MaxLogFiles = 1'000;

QString applicationLogPath;

/// Forwards each record to exactly one owned rolling file appender.
///
/// plog::Logger can add appenders but never remove them, so both formatters are owned here and
/// selected internally. That is what lets a run redirect the logger without leaving the previous
/// profile's log file permanently attached.
class SelectableLogAppender final : public plog::IAppender {
   public:
    SelectableLogAppender(const QString& logPath, const bool debugLog)
        : _debugAppender(qPrintable(logPath), MaxLogFileSize, MaxLogFiles),
          _infoAppender(qPrintable(logPath), MaxLogFileSize, MaxLogFiles),
          _logPath(logPath),
          _debugLog(debugLog) {}

    /// Points both owned appenders at the supplied file and selects the formatter for later
    /// records. The file is only re-opened when the path actually changes, so an unchanged path
    /// never re-emits the HTML header.
    void select(const QString& logPath, const bool debugLog) {
        if (logPath != _logPath) {
            _debugAppender.setFileName(qPrintable(logPath));
            _infoAppender.setFileName(qPrintable(logPath));
            _logPath = logPath;
        }
        _debugLog = debugLog;
    }

    void write(const plog::Record& record) override {
        if (_debugLog)
            _debugAppender.write(record);
        else
            _infoAppender.write(record);
    }

   private:
    plog::RollingFileAppender<plog::CustomDebugFormatter> _debugAppender;
    plog::RollingFileAppender<plog::CustomInfoFormatter> _infoAppender;
    QString _logPath;
    bool _debugLog;
};

std::unique_ptr<SelectableLogAppender> applicationAppender;

/// Returns the plog severity that the debug-log choice selects.
plog::Severity severityFor(const bool debugLog) {
    return debugLog ? plog::Severity::verbose : plog::Severity::info;
}

/// Creates the log folder and proves the file is writable before plog takes ownership of it.
void prepareLogFile(const QString& logPath) {
    // Creating log folder
    const QDir dir;
    dir.mkpath(QFileInfo(logPath).path());

    // Creating log file
    QFile file(logPath);

    if (!file.open(QFile::ReadWrite | QFile::Append))
        throw std::runtime_error("Cannot open log file: " + logPath.toStdString());
}
}  // namespace

namespace cao::application {
void configureLogging(const QString& logPath, const bool debugLog) {
    // A defensive repeat must not redirect or reconfigure the process-wide logger.
    if (plog::get()) return;

    prepareLogFile(logPath);

    applicationAppender = std::make_unique<SelectableLogAppender>(logPath, debugLog);
    plog::init(severityFor(debugLog), applicationAppender.get());

    applicationLogPath = logPath;
}

void applyRunLogging(const QString& logPath, const bool debugLog) {
    if (!plog::get() || !applicationAppender) {
        configureLogging(logPath, debugLog);
        return;
    }

    if (logPath != applicationLogPath) prepareLogFile(logPath);
    applicationAppender->select(logPath, debugLog);
    plog::get()->setMaxSeverity(severityFor(debugLog));
    applicationLogPath = logPath;
}

QString configuredLogPath() { return applicationLogPath; }
}  // namespace cao::application
