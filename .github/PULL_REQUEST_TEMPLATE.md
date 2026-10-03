<!-- Thanks for contributing to NestRS! Keep PRs focused — one logical change each. -->

## What & why

<!-- What does this change do, and why? Link the issue it closes: "Closes #123". -->

## How I verified it

<!--
For HTTP / GraphQL / MCP changes, `just verify` is necessary but not sufficient —
routing and wiring bugs don't surface in unit tests. Describe the live checks
you ran (curl, an MCP client, the GraphQL playground). See CONTRIBUTING.md.
-->

## Checklist

- [ ] One logical, focused change (unrelated cleanups go in their own PR)
- [ ] `just verify` passes
- [ ] Added/updated tests (regression test for a fix, coverage for a feature)
- [ ] Updated docs (README, crate docs, the docs site)
- [ ] For HTTP/GraphQL/MCP: verified the behaviour live
- [ ] Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/)
