---
name: deps
description: Refresh every dependency the repository pins — the semver-compatible pass, then each major one at a time — and take the security fixes. Run at each release cut, or when an advisory lands.
disable-model-invocation: true
---

# `/deps` — the dependency pass

1. **Advisories first**: `just supply-chain`. An advisory reaching a shipped
   binary or a public-surface dependency is fixed in this pass and named in the
   CHANGELOG with its identifiers.
2. **The fast pass**: `cargo update` in every lockfile (the root, `demo/`,
   `bench/sut/nestrs`); raise each `major.minor` floor to the minor that
   resolved, in every manifest the repository owns and in the scaffold
   templates; `just pre-commit`.
3. **The majors, one at a time** (`cargo upgrade --incompatible --dry-run` lists
   them): an internal dependency's major is taken in any release; a
   public-surface one — the pinned majors named in the root manifest, and any
   0.x whose types appear in a public signature — only in a nestrs major. Each
   major is its own commit, with the API changes it forces and the proof it
   builds and passes.
4. **Seven days old at least**: a release younger than that waits, unless it
   fixes an advisory.
5. **Beyond Cargo**: the GitHub Action SHAs, the tool versions the workflows and
   the devcontainer pin, the service images, the docs' npm lockfile.
6. **The freshness bar** (`.claude/rules/manifests-ci.md`): a dependency past it
   is flagged at its pin with the evidence and the condition to move.
7. `just ci`, then report what moved, what was held back and why.
