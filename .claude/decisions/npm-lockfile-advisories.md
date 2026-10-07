# The npm lockfiles wait on a scanner that records an exception

`just audit` reads the Cargo lockfiles only. The repository owns three npm ones —
`docs/` (the build that publishes nestrs.dev) and the two NestJS SUTs under
`bench/sut/` — and on 2026-10-05 `npm audit` found eleven advisories in the
first (one critical: Astro's remote code execution through AVIF image
optimization), three and four in the others. Every one with a fix was fixed
that day: the docs on the versions their ranges already allowed, the SUTs on
NestJS 12, both SUTs passing the bench's contract gate.

What stays is `braces` ≤ 3.0.3 (GHSA-vfj7-8cjw-p6xm, published 2026-09-18),
reached through `starlight-llms-txt` → `micromatch`: no release patches it. It
exhausts the stack on deeply nested patterns an attacker supplies, and the docs
build expands only the globs its own configuration writes, so nothing here
reaches it.

**Refused, each for a reason:**

- **Plain `npm audit` in `just audit`.** npm records no exception, so an
  advisory with no patch keeps the check red until upstream moves — a check
  red for a reason nobody can act on teaches everyone to read past it.
- **`--audit-level` raised** to step over it: it steps over every real
  advisory of that level too.
- **A filter over `npm audit --json` with our own ignore list**: a homemade
  scheme where a standard tool exists, and a script reading another's output.
- **Dropping `starlight-llms-txt`** to make the tree clean: the docs lose
  `llms.txt` for an advisory they do not reach.

**Proposed, for the owner's validation** (a new tool): osv-scanner (OpenSSF's
OSV format and its reference scanner, v2.6.0 on 2026-09-14, installable through
the `taiki-e/install-action` pin CI already uses). It reads `package-lock.json`
as well as `Cargo.lock`, and an exception is an `[[IgnoredVulns]]` entry naming
the advisory, its reason and an `ignoreUntil`, so it expires rather than
outlives its reason. `just audit` would run it over the npm lockfiles beside
cargo-deny over the Cargo ones; `audit.yml` would watch `**/package-lock.json`
and `**/package.json`.

## 2026-10-06 — validated, and landed

The owner validated osv-scanner. `just audit` runs `osv-scanner scan source
--config osv-scanner.toml` over every `package-lock.json` git tracks, beside
cargo-deny; `audit.yml` and `publish.yml` install it through the pinned
`taiki-e/install-action` (2.6), the dev container fetches the release by
version and checks its digest, and `audit.yml` also watches
`**/package-lock.json`, `**/package.json` and `osv-scanner.toml`.

Its first run found two advisories published on 2026-10-05, after the
`npm audit` pass, both in `docs/` and neither fixed by a release their parents'
ranges allow: GHSA-238p-pmpm-9mq7 (katex < 0.18.2, through `astro-mermaid` →
`mermaid`, whose 12.1.0 still requires `^0.16.47`) and GHSA-rj75-hqrm-r3gf
(postcss-selector-parser < 7.1.6, through `@expressive-code/core` 0.44.2's
`postcss-nested ^6`). Both process the site's own content, so each is an
`[[IgnoredVulns]]` entry with its reason, until 2027-01-06 like braces. Refused:
an npm `overrides` forcing either across its parent's major — a fix the parent
never ran against, for an advisory the site does not reach.

## 2026-10-07 — reviewed: every entry stands to its expiry

No release fixes any of the three: `braces` 3.0.3 is the latest and
`micromatch` 4.0.8 (latest) requires it; `mermaid` 12.1.0 (latest) still
requires `katex ^0.16.47`; `@expressive-code/core` 0.44.2 (latest) still
requires `postcss-nested ^6`. Each entry stands until 2027-01-06, when
`just audit` fails on it and asks the question again — the expiry is the
reminder, so none is kept anywhere else.
