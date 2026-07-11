# Issue tracker: GitHub

Issues and PRDs for this repository live as GitHub issues in `evildarkarchon/CAO`. Use the `gh` CLI for all operations.

## Conventions

- **Create an issue**: `gh issue create --title "..." --body "..."`.
- **Read an issue**: `gh issue view <number> --comments`, also fetching its labels when structured output is needed.
- **List issues**: `gh issue list --state open --json number,title,body,labels,comments --jq '[.[] | {number, title, body, labels: [.labels[].name], comments: [.comments[].body]}]'`, with appropriate `--label` and `--state` filters.
- **Comment on an issue**: `gh issue comment <number> --body "..."`.
- **Apply or remove labels**: `gh issue edit <number> --add-label "..."` or `gh issue edit <number> --remove-label "..."`.
- **Close an issue**: `gh issue close <number> --comment "..."`.

Run commands inside this clone, whose `origin` remote points to `evildarkarchon/CAO`. Add `--repo evildarkarchon/CAO` when repository inference could be ambiguous because this clone also has a GitLab remote.

## Pull requests as a triage surface

**PRs as a request surface: no.**

Do not include pull requests in the triage request queue. GitHub shares one number space across issues and pull requests, so if a bare `#42` is ambiguous, resolve it with `gh pr view 42` and fall back to `gh issue view 42`.

## When a skill says “publish to the issue tracker”

Create a GitHub issue.

## When a skill says “fetch the relevant ticket”

Run `gh issue view <number> --comments`.

## Wayfinding operations

Used by `/wayfinder`. The **map** is a single issue with **child** issues as tickets.

- **Map**: a single issue labelled `wayfinder:map`, holding the Notes, Decisions-so-far, and Fog body. Create it with `gh issue create --label wayfinder:map`.
- **Child ticket**: an issue linked to the map as a GitHub sub-issue through the sub-issues API. Where sub-issues are unavailable, add the child to a task list in the map body and put `Part of #<map>` at the top of the child body. Use `wayfinder:<type>` labels (`research`, `prototype`, `grilling`, or `task`). Once claimed, assign the ticket to the driving developer.
- **Blocking**: use GitHub’s native issue dependencies. Add an edge with `gh api --method POST repos/evildarkarchon/CAO/issues/<child>/dependencies/blocked_by -F issue_id=<blocker-db-id>`, where `<blocker-db-id>` is the blocker’s numeric database ID from `gh api repos/evildarkarchon/CAO/issues/<number> --jq .id`, not its issue number or node ID. If dependencies are unavailable, put `Blocked by: #<number>` at the top of the child body.
- **Frontier query**: list the map’s open children, exclude tickets with open blockers or an assignee, and select the first remaining ticket in map order.
- **Claim**: `gh issue edit <number> --add-assignee @me`; claiming is the session’s first write.
- **Resolve**: comment with the answer, close the child, then append a context pointer and link to the map’s Decisions-so-far.
