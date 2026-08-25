/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#pragma once

class QString;

namespace cao::application {
/**
 * Configures the process-wide application logger if it has not already been configured.
 * The first successful call owns the bootstrap path and severity; only applyRunLogging may
 * redirect the logger afterwards.
 * @throws std::runtime_error when the log file cannot be opened.
 */
void configureLogging(const QString& logPath, bool debugLog);

/**
 * Redirects the application logger to the run's own profile log path and severity.
 * The GUI snapshots both at startup, so a profile switch or a debug-log toggle made before the
 * first run would otherwise keep writing to the startup profile's file at the startup severity.
 * Bootstraps the logger when it has not been configured yet.
 * @throws std::runtime_error when the log file cannot be opened.
 */
void applyRunLogging(const QString& logPath, bool debugLog);

/** Returns the path the logger is currently writing to. */
[[nodiscard]] QString configuredLogPath();
}  // namespace cao::application
