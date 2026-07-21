# Contributing to NestRS

First off — thank you. NestRS ships at `1.0` under a semver contract, so you
are contributing to a codebase whose public API is frozen for the `1.x` line.
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
[Contributing → Get the dev container running](README.md#contributing) in the
README. It provisions the Rust toolchain, the dev tooling, and Postgres + Redis
with `NESTRS_DATABASE__URL` / `NESTRS_QUEUE__URL` already wired.

Prefer a local toolchain? Install Rust (stable, see
[`rust-toolchain.toml`](rust-toolchain.toml)) and the CLI:

```bash
cargo install --locked nest-rs-cli      # `nestrs run` bootstraps just, bacon, and cargo-nextest on first use
cargo install --locked cargo-llvm-cov   # only for `nestrs run test cov`
rustup component add llvm-tools-preview
```

## The workflow

```bash
nestrs run dev <app>   # run an app in watch mode (rebuild + restart on save)
nestrs run test unit        # full test suite (cargo-nextest)
nestrs run lint        # clippy (strict) + format check
nestrs run fmt         # apply rustfmt
nestrs run check       # fast type-check
```

Run `nestrs run` with no arguments to list every recipe.

Before opening a PR, make sure these pass:

```bash
nestrs run fmt && nestrs run lint && nestrs run test unit
```

Unit tests cover logic; the **e2e** suite (`nestrs run test e2e`) covers routing
and wiring against live infra. For **HTTP, GraphQL, or MCP changes**, close the
loop on a real socket too: start the app (`nestrs run dev <app>`), exercise the
affected endpoints (`curl`, an MCP client, the GraphQL playground), and note it
in the PR. A GraphQL change regenerates the committed SDL by running the dev
server (see CLAUDE.md).

## Pull requests

1. **Fork and branch.** Branch off `main`; name it for the change
   (`feat/query-param-schemas`, `fix/access-graph-diamond`).
2. **Keep it focused.** One logical change per PR. Unrelated cleanups belong in
   their own PR.
3. **Add tests.** A bug fix gets a regression test; a feature gets coverage of
   the new behaviour. A test binary is always `tests/<suite>/main.rs` with
   exactly two suite names: **`tests/integration/`** (in-process, submodules
   **mirror `src/`** — see CLAUDE.md and `nest-rs-authn` as the reference) and
   **`tests/e2e/`** (live infra, gated by `binary(e2e)`; apps boot their real
   module against Postgres, no mocks). Never a flat `tests/<x>.rs`.
   Use `#[cfg(test)]` in `src/` only when tests must see private code; otherwise
   add `Type::new(...)` so integration tests can construct providers without boot.
4. **Update the docs.** If you change behaviour, update the crate README, the
   docs site, and — if you made a design decision — CLAUDE.md. Crate READMEs
   stay tight (the `Cargo.toml` description, the install line, and links to the
   matching [nestrs.dev](https://nestrs.dev) page and the repo) — anything
   longer belongs on the docs site.
5. **Write a clear description.** What changed, why, and how you verified it. Link
   the issue it closes.

The *Definition of done* is a green local gate, and it is part of the PR. Run
`cargo clippy --workspace --all-targets -- -D warnings`,
`cargo fmt --all --check`, and `cargo nextest run --workspace` (plus the
`demo/` equivalents via `nestrs run` when you touch the product), then paste
the output in your PR description. A PR that has not passed them is not ready
for review.

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
