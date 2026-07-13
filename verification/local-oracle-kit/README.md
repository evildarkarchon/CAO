# Local oracle kit

This directory is reserved for the untracked behavioral oracle and licensed, private, or otherwise non-redistributable parity inputs. Everything below this directory is ignored except this file.

Acquire the exact archive named in `../baseline/implementation-inputs.json`, then verify it without moving or copying it:

```powershell
python tools/verify_baseline.py --verify-input "behavioral-oracle=verification\local-oracle-kit\Cathedral Assets Optimizer 64-23316-5-3-15-1687526925.7z"
```

Do not commit, redistribute, or place local oracle kit payloads in fixtures, evidence bundles, build artifacts, release packages, or logs. Committed records may contain only reviewed hashes, provenance, acquisition/generation instructions, and normalized evidence that is independently cleared for redistribution.
