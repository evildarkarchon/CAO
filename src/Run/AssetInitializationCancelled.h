#pragma once

#include <stop_token>

namespace cao::run {
/// Signals that cancellation stopped backend initialization before any Routed Asset operation or
/// filesystem mutation began. Asset Run records cancellation without creating an Asset attempt.
struct AssetInitializationCancelled final {};

/// Polls cancellation only while setup is read-only, so the run can omit attempt evidence safely.
inline void throwIfAssetInitializationCancelled(std::stop_token stop) {
    if (stop.stop_requested()) throw AssetInitializationCancelled{};
}
}  // namespace cao::run
