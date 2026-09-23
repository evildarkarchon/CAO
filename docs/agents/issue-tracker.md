# Issue tracker: Local Markdown

Issues and specs for this repo live as markdown files in `.scratch/`.

## Conventions

- One feature per directory: `.scratch/<feature-slug>/`
- The spec is `.scratch/<feature-slug>/spec.md`
- New specs start with YAML frontmatter so their triage role is unambiguous:

```yaml
---
id: <feature-slug>
type: specification
status: open
triage: ready-for-agent
blocked_by: []
---
```

- Older specs with a plain `Status:` line are legacy; use YAML frontmatter for new specs.
- Implementation issues are one file per ticket at `.scratch/<feature-slug>/issues/<NN>-<slug>.md`, numbered from `01`, never a single combined tickets file
- Every implementation issue starts with canonical YAML frontmatter:

```yaml
---
id: <feature-slug>-<NN>
type: implementation
status: open
triage: ready-for-agent
blocked_by:
  - <feature-slug>-<blocking-NN>
---
```

- `id` is unique and stable. Dependencies use IDs rather than filenames, numbers, or prose titles.
- `status` is the issue or spec lifecycle: `open` or `closed`. Only open records carry `triage`.
- `triage` uses the role strings in `triage-labels.md`.
- `blocked_by` is always a YAML list. An issue is eligible only when it is open, has an actionable triage role, and every referenced issue is closed.
- Missing dependency IDs, duplicate IDs, self-dependencies, and dependency cycles are invalid. Agents must not claim or implement an invalid or blocked issue.
- Frontmatter is authoritative. Do not duplicate status or dependency fields in the Markdown body.
- Comments and conversation history append to the bottom of the file under a `## Comments` heading

## When a skill says "publish to the issue tracker"

Create the spec at `.scratch/<feature-slug>/spec.md` or an implementation ticket at `.scratch/<feature-slug>/issues/<NN>-<slug>.md`, creating directories as needed. Use the next unused ticket number within that feature directory for a ticket.

Use the spec frontmatter for a spec and the implementation frontmatter for a ticket. Resolve dependency IDs before publishing a ticket and reject cycles rather than encoding them.

## When a skill says "fetch the relevant ticket"

Read the file at the referenced path. Ticket numbers are scoped to their feature directory; resolve a bare number within the current feature, and ask for the feature or path if it is ambiguous.

Before implementing it, resolve every `blocked_by` ID and confirm each dependency has `status: closed`.

## Closing an implementation issue

After the implementation is verified and committed:

1. Check every satisfied acceptance item in the issue body.
2. Set `status: closed` and remove `triage`.
3. Add the implementation commit under `completion.commits`:

```yaml
completion:
  commits:
    - <full-commit-sha>
```

Do not close an issue before its acceptance criteria are satisfied. A metadata-only follow-up commit may record the implementation commit that completed the work.

## Wayfinding operations

Used by `/wayfinder`. The **map** is a file with one **child** file per ticket.

- **Map**: `.scratch/<effort>/map.md` (the Notes / Decisions-so-far / Fog body).
- **Child ticket**: `.scratch/<effort>/issues/NN-<slug>.md`, numbered from `01`, with the question in the body. A `Type:` line records the ticket type (`research`/`prototype`/`grilling`/`task`); a `Status:` line records `open`/`claimed`/`resolved`. These are wayfinding lifecycle states; ordinary implementation tickets use the YAML schema above.
- **Blocking**: a `Blocked by: NN, NN` line near the top. A ticket is unblocked when every file it lists is `resolved`.
- **Frontier**: scan `.scratch/<effort>/issues/` for files that are open, unblocked, and unclaimed; first by number wins.
- **Claim**: set `Status: claimed` and save before any work.
- **Resolve**: append the answer under an `## Answer` heading, set `Status: resolved`, then append a context pointer (gist + link) to the map's Decisions-so-far in `map.md`.
