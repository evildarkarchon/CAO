# Archive Finalization Owns Loading Plugin Mutations

Durable Loading Plugin creation and removal belong in Archive Finalization so publication safety and an authoritative mutation fact for each action stay behind one run phase boundary. CAO uses the `cao-archive` port of bethutil over `ba2` for selected-profile settings, read-only Archive and plugin discovery, and naming. bethutil's void mutators exposed neither publication receipts nor mutation facts, so later directory inventories could not classify their effects reliably; `cao-archive` therefore supplies Dummy Plugin bytes but performs no Loading Plugin mutation itself. Creation moves to this boundary in #435; guarded removal follows in #436.

Amended 2026-11-19 (audit FS-4): Archive Finalization no longer removes Dummy Plugins. Turning Dummy Plugins off now means CAO creates and removes no plugin, since a Dummy Plugin an earlier run created may be all that loads an Archive it packed. Creation stays at this boundary.
