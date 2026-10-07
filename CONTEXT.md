# Interactive PR Review

A human-in-the-loop pull request review: an agent proposes findings with supporting evidence, and a human reviewer adjudicates each one before anything reaches the code host.

## Language

### Actors

**Agent**:
The LLM-driven program that analyzes a change and proposes Findings. It never decides what gets posted.
_Avoid_: AI, bot, model

**Reviewer**:
The human who adjudicates Findings and is the final authority on the Review.
_Avoid_: User, human, approver

### Findings and rulings

**Finding**:
A single claim by the Agent about a problem or concern in the change, with its Evidence, Impact, Recommendation, Severity and Confidence.
_Avoid_: Issue, comment, result

**Evidence**:
A typed item attached to a Finding that supports its claim: a Code excerpt, a diagram, a note, or command output.
_Avoid_: Proof, context, attachment

**Code excerpt**:
Evidence that references a file, ref and line range, with its text resolved from the repository by prr rather than supplied by the Agent.
_Avoid_: Snippet, quote

**Origin**:
Where a Finding came from: the Agent, the Reviewer, or an External reviewer.
_Avoid_: Source, author

**Reviewer finding**:
A Finding created by the Reviewer during Adjudication: a location and comment, without Evidence.
_Avoid_: Manual comment, ad-hoc comment

**External reviewer**:
An automated review bot (e.g. GitHub Copilot) whose comments already exist on the pull request.
_Avoid_: Bot, third-party reviewer

**External finding**:
A Finding whose Origin is an External reviewer's existing comment, which its Verdict acts on in place rather than posting anew.
_Avoid_: Imported comment, Copilot comment

**Corroboration**:
The link between an Agent Finding and an External finding that make the same claim; adjudicated as one Finding.
_Avoid_: Duplicate, merge

**Severity**:
The Agent's claim about how serious a Finding is. Advisory only; it is not a ruling.
_Avoid_: Priority, level

**Confidence**:
The Agent's stated certainty that a Finding is real.
_Avoid_: Score, probability

**Verdict**:
The Reviewer's ruling on a Finding: one of Block, Comment, Nit, Dismiss or Escalate.
_Avoid_: Decision, outcome, resolution

**Block**:
A Verdict that the Finding must be fixed before merge. Any Block makes the Review request changes.
_Avoid_: Request changes (that names the Review's state, not a Verdict)

**Comment**:
A Verdict that the Finding is worth raising but does not block merge.

**Nit**:
A Verdict that the Finding is minor or optional; raised as a low-priority suggestion.
_Avoid_: Downgrade

**Dismiss**:
A Verdict that the Finding will not be raised. Always carries a reason.
_Avoid_: Ignore, reject

**Escalate**:
A Verdict that the Finding needs judgment from a named person or team beyond the Reviewer, who is tagged on it.

**Adjudication**:
The act of the Reviewer assigning a Verdict to each Finding.
_Avoid_: Triage

### Sessions

**Review session**:
One Adjudication of one change, pinned to the change's head commit, from the Agent handing over Findings to the Review being submitted or abandoned.
_Avoid_: Run, job

**Stale session**:
A Review session whose change has gained new commits since its Findings were proposed.

### Output

**Review**:
The single submission sent to the code host, composed from all non-dismissed Findings and their Verdicts.
_Avoid_: Report, summary

**Review body**:
The top-level text of a Review, which also carries Findings that cannot be placed on a diff line.
_Avoid_: Summary comment

**Code host**:
The service hosting the pull request (e.g. GitHub) to which a Review is submitted.
_Avoid_: Forge, platform, provider
