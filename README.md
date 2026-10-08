# prr

Human-in-the-loop pull request review. An AI agent proposes Findings, each with Evidence, Impact, a Recommendation and its Confidence. You give each one a Verdict (Block, Comment, Nit, Dismiss or Escalate) in a local browser page, and prr composes the Review from your decisions.

```bash
cargo install --path .
prr guide          # how an agent drives it
prr schema         # findings file format
prr review --findings findings.json --base main
prr review --findings findings.json --pr 42   # GitHub PR, posts on submit
```

To install the Claude Code skill: `cp -r skills/prr ~/.claude/skills/`.

Vocabulary: [CONTEXT.md](CONTEXT.md). Design: [docs/superpowers/specs/2026-10-07-prr-design.md](docs/superpowers/specs/2026-10-07-prr-design.md). Decisions: [docs/adr](docs/adr).
