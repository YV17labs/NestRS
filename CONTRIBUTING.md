# Contributing to NestRS

First off — thank you. NestRS is young and moving fast, and early contributors
shape what it becomes. This guide is the shortest path from *I want to help* to
*my change is merged*.

New here? Browse the
[`good first issue`](https://github.com/NestRS/NestRS/labels/good%20first%20issue)
label, or open a thread in
[Discussions](https://github.com/NestRS/NestRS/discussions) and say hi.

## Ways to contribute

You don't have to write Rust to help.

- **Report a bug** — open an [issue](https://github.com/NestRS/NestRS/issues/new/choose)
  with a minimal reproduction.
- **Request a feature** — open an issue describing the problem first, not just a
  proposed solution.
- **Improve the docs** — typos, unclear passages, missing examples. The README
  and crate docs are as important as the code.
- **Answer questions** in [Discussions](https://github.com/NestRS/NestRS/discussions).
- **Send a pull request** — see below.

## Before you start

For anything beyond a small fix, **open an issue or a discussion first**. It
saves you from building something that doesn't fit the project's direction, and
lets a maintainer flag overlap or design constraints early. Drafts and questions
are welcome — you don't need a finished idea to start the conversation.

Read **[CLAUDE.md](CLAUDE.md)** before a non-trivial change. It is the project's
design record: what was decided and why. Two rules matter most:

- **Reach for the macros first.** Application code stays declarative through
  `#[injectable]`, `#[module]`, `#[controller]`, `#[resolver]` and friends. When a
  pattern recurs and no macro covers it, the answer is usually *write a new
  decorator macro*, not hand-rolled boilerplate.
- **The DI container is ours.** Don't propose adopting an external DI crate — if
  ergonomics fall short, we extend our own.

## Development setup

The fastest path is the dev container — see
[Getting started](README.md#getting-started) in the README. It provisions the
Rust toolchain, the dev tooling, and Postgres + Redis with
`NESTRS_DATABASE__URL` / `NESTRS_QUEUE__URL` already wired.

Prefer a local toolchain? Install Rust (stable, see
[`rust-toolchain.toml`](rust-toolchain.toml)) and the dev tools:

```bash
cargo install --locked just bacon cargo-nextest cargo-llvm-cov
rustup component add llvm-tools-preview
```

## The workflow

```bash
just dev <app>   # run an app in watch mode (rebuild + restart on save)
just test        # full test suite (cargo-nextest)
just lint        # clippy (strict) + format check
just fmt         # apply rustfmt
just check       # fast type-check
```

Run `just` with no arguments to list every recipe.

Before opening a PR, make sure these pass:

```bash
just fmt && just lint && just test
```

Routing and wiring bugs don't surface in **unit** tests — the **e2e** tests
catch most of them in `just test`. For **HTTP, GraphQL, or MCP changes** that is
still not sufficient: start the app (`just dev <app>`), exercise the affected
endpoints (`curl`, an MCP client, the GraphQL playground), and confirm the
behaviour live (real socket and external services the in-process harness can't
reach). A GraphQL change should
regenerate the committed SDL by running the dev server (see CLAUDE.md).

## Pull requests

1. **Fork and branch.** Branch off `main`; name it for the change
   (`feat/query-param-schemas`, `fix/access-graph-diamond`).
2. **Keep it focused.** One logical change per PR. Unrelated cleanups belong in
   their own PR.
3. **Add tests.** A bug fix gets a regression test; a feature gets coverage of
   the new behaviour. **Apps**: one `tests/e2e.rs` (real Postgres, no mocks).
   **Crates**: integration tests in a tree that **mirrors `src/`** (one
   `tests/<short>.rs` entry, subpaths = modules — see CLAUDE.md and `nestrs-authn`
   as the reference). Prefer that over ad-hoc `tests/<behaviour>.rs` names.
   Use `#[cfg(test)]` in `src/` only when tests must see private code; otherwise
   add `Type::new(...)` so integration tests can construct providers without boot.
4. **Update the docs.** If you change behaviour, update the README, the crate
   docs, and — if you made a design decision — CLAUDE.md.
5. **Write a clear description.** What changed, why, and how you verified it. Link
   the issue it closes.

CI runs format, lint, and the test suite. PRs must be green before review.

### Commit messages

This project uses [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>: <summary in the imperative mood>
```

Common types: `feat`, `fix`, `docs`, `refactor`, `test`, `build`, `chore`,
`style`, `perf`. Example: `feat(openapi): emit query-parameter schemas`.

## Adding a dependency

Every new third-party crate must have a published release within the last ~12
months. If a candidate fails this bar, say so explicitly in the PR — don't add a
stale dependency silently. See the *Dependency bar* section of CLAUDE.md.

## Code of Conduct

Participation is governed by our [Code of Conduct](CODE_OF_CONDUCT.md). By
contributing, you agree to uphold it.

## License

By contributing, you agree that your contributions are licensed under the
project's [MIT License](LICENSE).
