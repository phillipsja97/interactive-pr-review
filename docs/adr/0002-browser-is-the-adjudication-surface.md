# Adjudication happens in a localhost web UI, not the terminal or the Agent's chat

The Reviewer adjudicates Findings in a browser page served by prr on localhost, not in a TUI and not by relaying through the Agent's chat. The point of the tool is to get Adjudication out of the terminal so Evidence can be rendered properly: diagrams, syntax-highlighted excerpts, diffs. Agent harnesses generally give tool calls no TTY, which rules out an in-process TUI anyway.

## Consequences

The Agent cannot sit in a blocking call for the whole session (harness timeouts), so `prr review` is meant to run as a background task, and `prr wait <session>` lets a restarted Agent re-attach to a session already in progress. The UI is server-rendered (axum + maud, plain HTML forms) and embedded in the single binary, so no JS toolchain is needed to build or run it.
