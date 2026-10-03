# Contributing to NestRS

First off — thank you. NestRS ships under a semver contract, so you are
contributing to a codebase whose public API is frozen for the current major.
This guide is the shortest path from *I want to help* to *my change is merged*.

New here? Browse the
[`good first issue`](https://github.com/YV17labs/NestRS/labels/good%20first%20issue)
label, or open a thread in
[Discussions](https://github.com/YV17labs/NestRS/discussions) and say hi.

## Ways to contribute

You don't have to write Rust to help.

- **Report a bug** — open an [issue](https://github.com/YV17labs/NestRS/issues/new/choose)
  with a minimal reproduction.
- **Request a feature** — open an issue describing the problem first, not just a
  proposed solution.
- **Improve the docs** — typos, unclear passages, missing examples. The README
  and crate docs are as important as the code.
- **Answer questions** in [Discussions](https://github.com/YV17labs/NestRS/discussions).
- **Send a pull request** — see below.

## Before you start

For anything beyond a small fix, **open an issue or a discussion first**. It
saves you from building something that doesn't fit the project's direction, and
lets a maintainer flag overlap or design constraints early. Drafts and questions
are welcome — you don't need a finished idea to start the conversation.

Read **[CLAUDE.md](CLAUDE.md)** before a non-trivial change: the layout, the
commands and the conventions. Two rules matter most:

- **Reach for the macros first.** Application code stays declarative through
  `#[injectable]`, `#[module]`, `#[controller]`, `#[resolver]` and friends. When a
  pattern recurs and no macro covers it, the answer is usually *write a new
  decorator macro*, not hand-rolled boilerplate.
- **The DI container is ours.** Don't propose adopting an external DI crate — if
  ergonomics fall short, we extend our own.

## Development setup

The fastest path is the dev container — see
[Contributing → Get the dev container running](README.md#contributing) in the
README. It provisions the Rust toolchain, every tool CI runs, and Postgres, Redis
and S3 (RustFS) with their URLs already wired.

Prefer a local toolchain? Install Rust (stable, see
[`rust-toolchain.toml`](rust-toolchain.toml)) and the CLI:

```bash
cargo install --locked nest-rs-cli      # `nestrs run` bootstraps just, bacon, and cargo-nextest on first use
cargo install --locked cargo-llvm-cov   # only for `nestrs run test cov`
rustup component add llvm-tools-preview
```

## The workflow

The framework's checks are recipes of the root `Justfile`, the same ones CI
runs. `demo/` is a separate project and drives itself with `nestrs run`.

```bash
just lint   # fmt check + clippy
just test   # every test, against the dev container's Postgres, Redis and S3
just verify # every check: lint, docs and tests
```

Before opening a PR, `just verify` passes. For **HTTP, GraphQL, or MCP changes**,
close the loop on a real socket too: start an app (`nestrs run dev <app>` in
`demo/`), exercise the affected endpoints (`curl`, an MCP client, the GraphQL
playground), and note it in the PR.

## Pull requests

1. **Fork and branch.** Branch off the integration branch — `main`, or the
   `release/<x.y>` a major is prepared on — and name it for the change
   (`feat/query-param-schemas`, `fix/access-graph-diamond`).
2. **Keep it focused.** One logical change per PR. Unrelated cleanups belong in
   their own PR.
3. **Add tests.** A bug fix gets a regression test; a feature gets coverage of
   the new behaviour. Each crate has one suite, `tests/integration/main.rs`,
   whose tests run in process or against the dev container's services — no
   mocks. Unit tests sit in `#[cfg(test)] mod tests` beside the code.
4. **Update the docs.** If you change behaviour, update the crate README and
   the docs site. Crate READMEs stay tight (the `Cargo.toml` description, the
   install line, and links to the matching [nestrs.dev](https://nestrs.dev)
   page and the repo) — anything longer belongs on the docs site.
5. **Write a clear description.** What changed, why, and how you verified it. Link
   the issue it closes.

The *Definition of done* is `just verify` green — the checks `ci.yml` runs on every
pull request. A PR that has not passed it is not ready for review.

### Commit messages

This project uses [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <what is now true, as one sentence>
```

Common types: `feat`, `fix`, `docs`, `refactor`, `test`, `build`, `chore`,
`perf`, with `!` for a breaking change. Example:
`fix(pipes): a refused ParseArray item is named by its position and type, never quoted`.

## Adding a dependency

A new third-party crate is a maintainer's decision: propose it in the issue
first. It needs a published release within about the last twelve months.

## Code of Conduct

Participation is governed by our [Code of Conduct](CODE_OF_CONDUCT.md). By
contributing, you agree to uphold it.

## License

By contributing, you agree that your contributions are licensed under the
project's [MIT License](LICENSE).
