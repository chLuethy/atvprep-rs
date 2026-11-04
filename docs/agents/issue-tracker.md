# Issue tracker: Gitea

Issues and specs for this repo live as Gitea issues on `git.ch-luethy.ch`, repository `chl/atvprep-rs` (owner `chl`, repo `atvprep-rs`). Use the Gitea MCP tools (`mcp__gitea__*`); the `gitea-issues` skill also applies. Load deferred tool schemas with `ToolSearch` first if needed.

## Conventions

- **Create an issue**: `issue_write` with `method: create`, `owner: chl`, `repo: atvprep-rs`, `title`, `body`.
- **Read an issue**: `issue_read` (body, labels, comments).
- **List issues**: `list_issues` / `search_issues`, filtered by `state` and labels.
- **Comment on an issue**: `issue_write` with the comment method.
- **Apply / remove labels**: `issue_write` (edit) with the label list; manage label definitions with `label_read` / `label_write`.
- **Close**: `issue_write` (edit) with `state: closed`, after leaving a closing comment.

Gitea shares one number space across issues and PRs, so a bare `#42` may be either.

## Pull requests as a triage surface

**PRs as a request surface: no.** _(Set to `yes` if this repo treats external PRs as feature requests; `/triage` reads this flag.)_

## When a skill says "publish to the issue tracker"

Create a Gitea issue in `chl/atvprep-rs`.

## When a skill says "fetch the relevant ticket"

Read it with `issue_read`, including comments.

## Wayfinding operations

Used by `/wayfinder`. The **map** is a single issue with **child** issues as tickets.

- **Map**: one issue labelled `wayfinder:map`, holding Notes / Decisions-so-far / Fog.
- **Child ticket**: an issue with `Part of #<map>` at the top of its body, and listed in a task list in the map body. Labels: `wayfinder:<type>` (`research`/`prototype`/`grilling`/`task`). Once claimed, assign it to the driving dev.
- **Blocking**: a `Blocked by: #<n>, #<n>` line at the top of the child body. Unblocked when every blocker is closed.
- **Frontier query**: list the map's open children, drop any with an open blocker or an assignee; first in map order wins.
- **Claim**: assign the issue to yourself, as the session's first write.
- **Resolve**: comment with the answer, close the issue, then append a pointer to the map's Decisions-so-far.
