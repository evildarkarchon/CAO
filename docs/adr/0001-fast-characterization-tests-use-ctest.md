# Use CTest for the fast characterization suite

We will start the broader test effort with a fast characterization suite, not end-to-end asset-processing fixtures. CTest is the canonical entry point for this milestone, so existing and new fast tests should be runnable through one command, including the Windows build-helper PowerShell test. Small behavior-preserving production seams are allowed when they make characterization tests cheap, but the milestone should avoid real BSA, texture, mesh, and animation processing fixtures until the code has clearer test boundaries.

Milestone one is complete when CTest runs expanded `AssetWorkPlanner` coverage, fast `FilesystemOperations` coverage, `OptionsCAO` settings and validation coverage, a small Manager planning seam tested without running optimizers, and the existing PowerShell build-helper test. Profile globals and real BSA, texture, mesh, and animation processing are intentionally deferred unless they become necessary to support those fast checks.

Characterization tests should pin behavior that appears intentional or protects planned refactors. Suspicious behavior should not be blessed just because it is current behavior; record it as follow-up work until the intended behavior is decided.

Milestone-one fixtures should be created at runtime in temporary directories. Checked-in fixtures are deferred until realistic binary assets or larger integration scenarios are unavoidable.
