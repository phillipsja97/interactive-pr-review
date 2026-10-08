# prr — guide for agents

prr lets a human Reviewer adjudicate the Findings you propose for a change. You analyze; the Reviewer decides. prr shows your Evidence in a browser and returns the Reviewer's Verdicts to you as JSON. prr never posts anything the Reviewer did not approve, and you should not post review comments yourself.

## Workflow

1. Analyze the change, e.g. `git diff main...HEAD`.
2. Write your Findings to a JSON file. The exact format is `prr schema`; an example is below.
3. Start the session **as a background task**, because it blocks until the Reviewer finishes, which can take a long time:

       prr review --findings findings.json --base main

   prr prints `prr: session <id>` and `prr: open <url>` on stderr and opens the browser. Tell the Reviewer the URL.
4. When it exits, read the result JSON on stdout. Each Finding has the Reviewer's `verdict` (`block`, `comment`, `nit`, `dismiss`, `escalate`), the `comment` as they edited it, and any `dismiss_reason` or `escalate_to`. If the change is the Reviewer's own, offer to fix the Blocks. Treat dismiss reasons as feedback on your analysis.

If your process dies, re-attach with `prr wait <session-id>`; `prr sessions --open` lists open sessions. Verdicts already made are kept.

Options: `--head <ref>` (default `HEAD`), `--findings -` to read stdin, `--export-md <path>` to also write the composed Review as markdown, `--no-open` to only print the URL, `--replace` to abandon an open session for the same change.

## GitHub pull requests

Review a pull request with `--pr` instead of `--base`/`--head`:

    prr review --findings findings.json --pr 42

`--pr` takes the PR number (in this clone's GitHub repository) or its URL. It needs the GitHub CLI (`gh`) logged in, and the PR's commits in this clone; if they are missing prr exits 1 and says to run `gh pr checkout 42`. When the Reviewer submits, prr posts the Review to the PR: one review, inline comments on changed lines, everything else in the body with permalinks. Add `--pending` to create a pending review the Reviewer finishes on GitHub instead. On the Reviewer's own PR GitHub only allows a comment review, so prr posts Blocks as comments marked Blocking.

The result then also has `"posted": true`, `posted_url` and `posted_state`. If GitHub rejects the review, the Reviewer sees why and the session stays open.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Review submitted; result on stdout |
| 1 | Error; message on stderr |
| 10 | Findings file invalid; `{"errors": [{"path", "message"}]}` on stderr, each `path` a JSON pointer. Fix and rerun. |
| 11 | The Reviewer abandoned the session; result on stdout |
| 12 | A session for this change is already open; `prr wait <id>`, or rerun with `--replace` |
| 130 | Interrupted; the session stays open for `prr wait` |

## Writing good Findings

- One claim per Finding. Two problems in one function are two Findings.
- `location` lines are 1-based, on the head commit. Paths are repo-relative with forward slashes: `src/main.rs`, not `./src/main.rs` or an absolute path. Use `{"kind": "pr_wide"}` for concerns not tied to lines.
- Show code with `code_excerpt` Evidence rather than quoting it in prose. prr reads excerpts from git, so the Reviewer can trust them. Add `"ref"` to show another commit, e.g. the base version.
- `confidence.rationale` says *why*: "traced all 4 callers" vs "pattern match only, not verified".
- Use a `diagram` (Mermaid source) when a flow, sequence or dependency is easier to see than to read.
- `command_output` is for real output you ran, such as a failing test.
- `suggested_patch` replaces the location's lines exactly and needs a `lines` location.
- `draft_comment` is what gets posted. Write it for the change's author, not for the Reviewer.
- Order doesn't matter; prr sorts by severity and then confidence.

## Example

```json
{{EXAMPLE}}
```
