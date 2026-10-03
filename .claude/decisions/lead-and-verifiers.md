# One writer, independent verifiers (7.0)

The owner asked for agents that work like a disciplined team, with separated
responsibilities, so that no agent is judge and party. The first proposal
(2026-10-03) modelled a human organisation chart: eight roles (CTO, architect,
developer, QA, security, performance, release, docs), a write zone each held by
a hook, and a deterministic workflow per kind of task. A pilot showed the
friction (new agent definitions and hooks load only at session start), and the
evidence gathered that day weighed against the shape:

- writing in parallel is where multi-agent setups fail — Google's 2026 study of
  180 configurations found every multi-agent variant 39–70% worse on sequential
  tasks, and Cognition's 2026 rule is that writes stay single-threaded while
  extra agents contribute judgement;
- role personas do not last (BMAD folded its QA and Scrum Master personas into
  the developer in 6.3.0); the documents and the maker/checker split do;
- a model rates its own work above others' and does not correct its reasoning
  without outside feedback, so the checker needs a fresh context;
- instruction files help little and cost context; what binds is held by tools.

So: the session the owner talks to writes and integrates; `qa` (the test
oracle) and `security` (the prover) are the only subagents, each held to its
zone by a hook; `/code-review` is Claude Code's own; architecture, release,
dependencies, documentation and performance are responsibilities with a
procedure (`/name`, `/release`, `/deps`), not agents. Review skills that only
reported (`/architecture`, and `/audit` rounds whose findings sat untriaged)
gave way to loops that fix: a finding is fixed with its test, re-checked once,
or brought to the owner. A role or a workflow is added when a measured problem
asks for it, and removed when it stops paying.
