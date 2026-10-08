# prr — Design

Date: 2026-10-07. Vocabulary is defined in [CONTEXT.md](../../../CONTEXT.md); decisions with lasting consequences are in [docs/adr](../../adr).

## Purpose

`prr` is a single Rust binary that an Agent invokes to run a human-in-the-loop pull request review. The Agent proposes Findings with Evidence; the Reviewer adjudicates each one in a browser page served by prr on localhost; prr composes the Review from the Verdicts and (optionally) submits it to the Code host. The Reviewer is the final authority on everything that is posted.

## Architecture

- **The Agent is the only analyzer** (ADR 0001). prr does no LLM inference. It holds state, resolves Evidence deterministically from git, renders, and composes.
- **Adjudication happens in the browser** (ADR 0002): axum serving server-rendered HTML (maud), plain HTML forms, vendored Mermaid.js, server-side syntax highlighting (syntect). Everything is embedded in the binary.
- **Host-agnostic core.** The core works on a local git range (`--base`/`--head`). Code hosts are adapters; GitHub (via `gh api`) is the first.

## Agent workflow and CLI

| Command | Purpose |
|---|---|
| `prr review --findings <file\|-> --base <ref> [--head <ref>] [--no-open] [--export-md <path>] [--replace]` | Validate Findings, open a Review session, block until the Reviewer finishes, print the result JSON on stdout. Run by the Agent as a background task. |
| `prr wait <session-id> [--no-open]` | Re-attach to a session (serves it again if no live prr process is serving it; otherwise waits for that process) and print its result. |
| `prr sessions [--open]` | List sessions as JSON. |
| `prr schema` | Print the JSON Schema of the findings file. |
| `prr guide` | Print the agent-facing usage guide (generated from the same Rust types as the schema). |

stderr carries `prr: session <id>` and `prr: open <url>` lines; stdout carries only the result JSON.

Exit codes: `0` submitted · `1` error · `10` invalid findings (JSON errors with JSON-pointer paths on stderr) · `11` abandoned · `12` a session for this change is already open · `130` interrupted (session left open). Codes avoid `2`, which clap uses for usage errors.

## Finding schema

`{"findings": [Finding]}`, unknown fields rejected. A Finding has: `id` (unique, not starting with `reviewer-`), `title`, `summary`, `location` (`{"kind":"lines","path","start","end"}` on the head commit, or `{"kind":"pr_wide"}`), `severity` (`critical|major|minor|info`), `confidence` (`{"level":"high|medium|low","rationale"}` — rationale required), `impact`, `recommendation`, optional `suggested_patch` (lines location only), `evidence[]`, `draft_comment`, optional `escalate_to`.

Evidence kinds: `code_excerpt` (`path`, `start`, `end`, optional `ref`, optional `caption`) whose text **prr resolves from git** — the Agent never supplies excerpt text; `diagram` (Mermaid source); `note`; `command_output`.

Validation rejects the whole batch before the browser opens: malformed JSON, missing/blank required text, duplicate or reserved ids, paths that are not repo-relative forward-slash paths, files missing at the ref, line ranges out of bounds, excerpts over 400 lines, unknown refs, non-UTF-8 files, invalid escalation handles. Invalid Mermaid is not rejected; it renders as an inline error on that Evidence item.

## Adjudication UI

- Guided: one Finding per screen, ordered by Severity then Confidence (descending, stable), with a sidebar of all Findings and their Verdicts.
- Shows severity, confidence + rationale, origin, location with highlighted code, summary, impact, recommendation, suggested patch, and each Evidence item.
- Verdict form: Block / Comment / Nit / Dismiss (reason required) / Escalate (`@user` or `@org/team` required). The comment to post is pre-filled from `draft_comment` and editable. Keyboard: `b c n d e` choose a Verdict, `j`/`k` move, Ctrl+Enter saves.
- The Reviewer can add Reviewer findings (location or PR-wide, plus comment), which then also need a Verdict.
- Submit screen: lists Findings still without a Verdict (submit is refused until there are none), stale-change warning with required acknowledgement, summary, Approve checkbox, preview of the composed Review, and Abandon.

## Verdicts and Review composition

- Any Block → Review state `request_changes`; Approve is refused while anything is Blocked.
- Otherwise `approve` only if the Reviewer ticks it; else `comment`. If nothing survives Dismiss and there is no summary and no approval → nothing to post.
- Findings whose line range sits entirely inside one diff hunk of the head side become inline comments; others go into the Review body, grouped Blocking → Needs input → Comments → Nits, with a citation (permalinks once a Code host adapter provides them).
- Comment text by Verdict: Block → `**Blocking:** …`; Comment → as written; Nit → `**Nit (optional):** …`; Escalate → `**Needs input from @x:** …`. A `suggested_patch` is appended as a ```` ```suggestion ```` block on inline comments only.
- Local mode posts nothing; `--export-md` writes the composed Review as markdown.

## Sessions

- Stored as one JSON file per session under `$PRR_STATE_DIR` or the XDG state dir (`~/.local/state/prr/sessions/`), written atomically after every change, so Verdicts survive crashes.
- A session is pinned to the head SHA. If the head ref has moved by submit time the session is stale: the UI warns and requires acknowledgement. Findings are not re-mapped.
- One serving process per session, enforced with an exclusive file lock. A second `prr review` for the same change while one is open exits 12 unless `--replace` (refused if the old one is still being served).

## Security

Bind `127.0.0.1` on a random port; every route lives under a random per-session token path; reject requests whose `Host` is not `127.0.0.1:<port>`/`localhost:<port>` and non-GET requests with a foreign `Origin`; shut down when the session ends. All Agent-supplied text is HTML-escaped; Mermaid runs with `securityLevel: 'strict'`.

## Result contract (stdout)

```json
{"session_id": "…", "status": "submitted|abandoned", "review_state": "request_changes|comment|approve|null",
 "summary": "…", "export_path": null, "posted": false,
 "findings": [{"id": "F1", "origin": "agent|reviewer", "title": "…", "location": {…},
               "verdict": "block|comment|nit|dismiss|escalate|null", "comment": "…",
               "dismiss_reason": null, "escalate_to": null}]}
```

## GitHub adapter (plan 2)

- `prr review --pr <number|url>` (instead of `--base`/`--head`) identifies the PR; a bare number means the clone's own GitHub repository (`gh repo view`). Base/head SHAs come from `gh api`; the Review's base is their merge-base. Only open PRs can be reviewed.
- The PR's head and base commits must already be in the clone; otherwise prr exits 1 telling the Agent to run `gh pr checkout <n>` (prr never fetches).
- Staleness is checked against the PR's current head on GitHub (an unreachable GitHub counts as stale).
- Submit posts one review (`POST /pulls/{n}/reviews`): `commit_id` = head SHA, `event` from the Review state, inline comments with `line`, `side: RIGHT` and, for ranges, `start_line`/`start_side`. Body citations become permalinks at the head SHA. Nothing to post → no API call.
- `--pending` omits `event`, creating a pending review the Reviewer finishes on GitHub.
- On the Reviewer's own PR GitHub refuses REQUEST_CHANGES and APPROVE: prr posts COMMENT (Blocks keep their `**Blocking:**` label) and disables Approve.
- If GitHub rejects the review, the submit page shows GitHub's message and the session stays open.
- The result gains `posted: true`, `posted_url` and `posted_state` (GitHub's review state).
- prr talks to GitHub only through the `gh` CLI (its auth, its host); `PRR_GH` overrides the command (whitespace-separated), which is how tests substitute a fake.

## External reviewers (plan 3)

`prr import copilot --pr <n>` prints Copilot's review comments as draft Findings (Origin: External reviewer, carrying the comment/thread id) for the Agent to enrich with Evidence, Confidence and its own assessment. prr verifies that imported ids exist. Verdicts act on the existing thread: Block/Comment/Nit reply endorsing it (Block still drives the Review state); Dismiss resolves the thread, posting the reason only if the Reviewer opts in; Escalate replies tagging the handle. An Agent Finding may declare `corroborates: <comment id>`; it is adjudicated as one Finding with both origins, acting on the existing thread. Bot authors are configurable later; Copilot only at first. Human reviewers' comments are permanently out of scope.

## Out of scope for v1

Live Reviewer→Agent questions during Adjudication; persistent suppressions learned from Dismiss; CODEOWNERS suggestions for Escalate; bots other than Copilot; de-duplication against existing PR comments.
