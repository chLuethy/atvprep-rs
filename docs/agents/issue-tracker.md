# Issue tracker: GitHub

Issues and specs for this repo live as GitHub issues in `chLuethy/atvprep-rs`. Use the `gh` CLI (`gh issue …`, `gh api …`). Gitea (`chl/atvprep-rs`) is only the release mirror and takes no issues (ADR 0009).

## Conventions

- **Sprache**: Titel, Body und Kommentare auf Deutsch schreiben (siehe `docs/agents/language.md`). Label-Namen und Marker wie `Part of #<n>` / `Blocked by: #<n>` bleiben englisch.
- **Create an issue**: `gh issue create --title "…" --body-file -` (pass the body on stdin).
- **Read an issue**: `gh issue view <n> --comments`.
- **List issues**: `gh issue list --state <open|closed|all> --label <label> --json number,title,labels`.
- **Comment on an issue**: `gh issue comment <n> --body-file -`.
- **Apply / remove labels**: `gh issue edit <n> --add-label <l> --remove-label <l>`; manage label definitions with `gh label`.
- **Close**: `gh issue close <n> --comment "…"` (`--reason "not planned"` for wontfix).

GitHub shares one number space across issues and PRs, so a bare `#42` may be either. Issues #1–#58 were imported from Gitea; their first line names the original Gitea number. `#n` in commit messages from before the move refers to Gitea numbers.

## Pull requests as a triage surface

**PRs as a request surface: no.** _(Set to `yes` if this repo treats external PRs as feature requests; `/triage` reads this flag.)_

## When a skill says "publish to the issue tracker"

Create a GitHub issue in `chLuethy/atvprep-rs`.

## When a skill says "fetch the relevant ticket"

Read it with `gh issue view <n> --comments`.

## Wayfinding operations

Used by `/wayfinder`. The **map** is a single issue with **child** issues as tickets.

- **Map**: one issue labelled `wayfinder:map`, holding Notes / Decisions-so-far / Fog.
- **Child ticket**: an issue with `Part of #<map>` at the top of its body, and listed in a task list in the map body. Labels: `wayfinder:<type>` (`research`/`prototype`/`grilling`/`task`). Once claimed, assign it to the driving dev.
- **Blocking**: a `Blocked by: #<n>, #<n>` line at the top of the child body. Unblocked when every blocker is closed.
- **Frontier query**: list the map's open children, drop any with an open blocker or an assignee; first in map order wins.
- **Claim**: assign the issue to yourself, as the session's first write.
- **Resolve**: comment with the answer, close the issue, then append a pointer to the map's Decisions-so-far.
