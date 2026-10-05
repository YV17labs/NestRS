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
