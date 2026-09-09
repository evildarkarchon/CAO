/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "pch.h"

/*!
 * \brief The AnimationsOptimizer class will handle all operations related to animations (hkx files)
 */

class AnimationsOptimizer final : public QObject {
   public:
    /// Ports an Oldrim Animation with the Havok post-processor and reports every execution failure.
    /// Reads sourcePath without changing it and writes only to the caller's registered staging
    /// outputPath. Returns true only after the process succeeds and produces a nonempty file;
    /// the caller retains responsibility for committing or cleaning the staged output.
    [[nodiscard]] bool convert(const QString& sourcePath, const QString& outputPath);

   private:
    bool hkxcmdFound = false;
    std::once_flag onceFlag;

    constexpr static inline auto hkxcmdPath = "bin/hkxcmd.exe";
};
