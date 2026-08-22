/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#pragma once

class QString;

namespace cao::application
{
/**
 * Configures the process-wide application logger if it has not already been configured.
 * The first successful call owns the logger path and severity for the lifetime of the process.
 * @throws std::runtime_error when the log file cannot be opened.
 */
void configureLogging(const QString &logPath, bool debugLog);

/** Returns the immutable path selected by the successful bootstrap configuration. */
[[nodiscard]] QString configuredLogPath();
}
