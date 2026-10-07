#pragma once

namespace cao::execution {
/// Filesystem effects on durable Assets, excluding registry-owned temporary bytes.
enum class MutationState { None, Committed, PartialOrUnknown };
}  // namespace cao::execution
