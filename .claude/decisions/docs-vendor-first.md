# The docs site follows its vendor

`docs/` is built on third-party software — Astro, Starlight and their plugins —
and their vendors know it better than we do. The owner decided on 2026-10-08
that the vendor has priority: the site is upgraded through the vendor's own
tool (`npx @astrojs/upgrade`, which the Starlight and plugin release notes
name), what the vendor asks to change — a breaking change, a deprecation — is
changed in that upgrade, a release the vendor says to hold is held, and nothing
the vendor did not ask for is done. A plugin the tool does not reach is
upgraded the way its own release notes say.

**Refused:** anything of our own around the vendor's tree — an npm `overrides`,
`npm audit fix --force`, a downgrade a scanner proposes (it offered mermaid 10
for a katex advisory), a homemade patch or workaround. A defect in the stack is
the vendor's to fix and is taken on the next update
(`npm-lockfile-advisories.md`).
