---
name: prr
description: Use when reviewing a pull request or local branch with a human in the loop — propose findings with evidence, have the Reviewer adjudicate them in the browser with the prr CLI, then act on their verdicts. Use instead of posting a list of review comments directly.
---

# Interactive review with prr

1. Run `prr guide` and follow it. Use `prr schema` when unsure of the findings format.
2. Analyze the change yourself; prr does no analysis.
3. Write the findings file, then run `prr review --findings <file> --base <base>` with the Bash tool's `run_in_background: true`. Tell the user the `prr: open` URL from its output. For a GitHub pull request use `--pr <number>` instead of `--base` (check out its commits first with `gh pr checkout <number>`).
4. If prr exits 10, fix the reported paths in the findings file and rerun.
5. When it finishes, summarise the Verdicts for the user. If the change is theirs, offer to fix the Blocks.
6. Never post review comments yourself; the Reviewer decides what gets posted.
7. If you lose track of a session, `prr sessions --open` lists sessions and `prr wait <id>` re-attaches.
