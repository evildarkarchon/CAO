## Platform scope

CAO targets Windows; some dependencies require porting for POSIX. Validate releases with Windows profile triplets and Windows runtime behavior. Treat POSIX-only findings as compatibility notes unless a task explicitly includes porting.

## Build workflow

- Use the project's profile triplets as the primary workflow for building the program and validating changes.
- Use alternate generators or direct CMake builds outside the profile-triplet workflow only for specific testing that requires them. State the testing purpose when choosing an alternate workflow.
- If a profile-triplet build is blocked, diagnose and report the blocker rather than silently switching to an alternate generator. An alternate-generator test does not replace validation through the profile triplets.

## Agent skills

### Issue tracker

Issues and specs are tracked in GitHub Issues for `evildarkarchon/CAO`. See `docs/agents/issue-tracker.md`.

### Triage labels

Triage uses the five canonical default label names. See `docs/agents/triage-labels.md`.

### Domain docs

Domain documentation uses the single-context layout. See `docs/agents/domain.md`.
