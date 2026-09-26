# Issue tracker: GitHub

Issues and specs for this repo live as GitHub issues in `evildarkarchon/CAO`. Use the `gh` CLI for issue operations.

## Conventions

- Create an issue with `gh issue create --title "..." --body-file <path>` for a multiline body.
- Read an issue with `gh issue view <number> --comments`; use `--json number,title,body,labels,comments` when structured data is needed.
- List issues with `gh issue list --state open --json number,title,body,labels,comments`, adding label or state filters as needed.
- Comment with `gh issue comment <number> --body-file <path>` for multiline text.
- Apply or remove labels with `gh issue edit <number> --add-label "..."` or `--remove-label "..."`. Use the state label names in `docs/agents/triage-labels.md`.
- Close an issue with `gh issue close <number> --comment "..."`.

Run `gh` inside this clone so it resolves the repository from the Git remote, or pass `--repo evildarkarchon/CAO` explicitly.

## Pull requests as a triage surface

**PRs as a request surface: no.** Set this to `yes` if external PRs should enter the triage queue.

When set to `yes`, use the corresponding `gh pr` commands and the same triage labels. For discovery, include open PRs whose `authorAssociation` is `CONTRIBUTOR`, `FIRST_TIME_CONTRIBUTOR`, or `NONE`. Read a PR's comments and diff before triaging it. GitHub issues and PRs share a number space; resolve an ambiguous `#<number>` before acting.

## When a skill says "publish to the issue tracker"

Create a GitHub issue.

## When a skill says "fetch the relevant ticket"

Run `gh issue view <number> --comments`.

## Wayfinding operations

Used by `/wayfinder`. The map is one issue, with child issues as tickets.

- **Map**: an issue labelled `wayfinder:map`, holding the Notes / Decisions-so-far / Fog body.
- **Child ticket**: a GitHub sub-issue of the map, labelled `wayfinder:<type>` (`research`, `prototype`, `grilling`, or `task`). Where sub-issues are unavailable, link it in a task list in the map body and put `Part of #<map>` at the top of the child body.
- **Blocking**: use GitHub's native issue dependencies. Add a blocker through `gh api --method POST repos/<owner>/<repo>/issues/<child>/dependencies/blocked_by -F issue_id=<blocker-db-id>`. Obtain the blocker's database ID with `gh api repos/<owner>/<repo>/issues/<number> --jq .id`. Where dependencies are unavailable, use a `Blocked by: #<number>` line in the child body. A child is unblocked when every blocker is closed.
- **Frontier**: inspect the map's open children in map order; the first unassigned child with no open blocker wins.
- **Claim**: assign the child with `gh issue edit <number> --add-assignee @me`.
- **Resolve**: comment with the answer, close the child, and append a gist and link to the map's Decisions-so-far.
