# The Agent is the only analyzer; prr never calls an LLM

prr receives Findings as structured data from whatever Agent invokes it and does no LLM inference of its own. Embedding a model client would duplicate the calling Agent's capability and lock prr to one provider. prr's job is everything the Agent is unreliable at: holding state, resolving Evidence deterministically from git, presenting Findings and composing the Review. Deterministic analysis (call sites, test coverage of changed lines) may be added later, but it must stay non-LLM.
