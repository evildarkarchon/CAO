# Archive Finalization Owns Loading Plugin Mutations

Durable Loading Plugin creation and removal belong in Archive Finalization so publication safety and an authoritative mutation fact for each action stay behind one run phase boundary. CAO uses bethutil for selected-profile settings, read-only Archive and plugin discovery, and naming; bethutil's void mutators expose neither publication receipts nor mutation facts, so later directory inventories cannot classify their effects reliably. Creation moves to this boundary in #435; guarded removal follows in #436.
