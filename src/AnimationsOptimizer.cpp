/*!
 * Stripped down version of this file
 * https://github.com/aerisarn/ck-cmd/blob/master/src/commands/hkx/Convert.cpp
 */

#include "AnimationsOptimizer.h"

AnimationsOptimizer::AnimationsOptimizer(const bool dryRun)
    : _transaction(AnimationAssetTransaction::createProduction(dryRun)) {}

AssetTransactionResult AnimationsOptimizer::convert(const QString &filePath) {
  return _transaction->execute(filePath);
}
