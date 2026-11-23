# Upstream

What a third-party library or the toolchain gets wrong, or leaves out, that
nestrs works around or documents as a limit: kept so it is reported upstream and
retired the day upstream fixes it, never patched or forked here
(`rules/manifests-ci.md`). Never loaded into a session — read a library's file
when its version moves, the toolchain's when `rust-toolchain.toml`'s pin does.
One file per library, named by its crate, and one per toolchain component, named
by it (`rustc.md`); an entry is appended to, never rewritten, and closed by a
line saying which version fixed it and which commit retired our side.

An entry holds, in this order:

- **Seen** — the version, and the proof: the source lines in that version, or
  the test that shows it.
- **Effect** — what a nestrs user gets wrong or goes without.
- **Our side** — the workaround and the file holding it, or the documented limit
  and the page saying it.
- **Canary** — the test that fails the day upstream fixes it, where one is cheap;
  none otherwise.
- **Upstream** — `not filed`, or the link to the issue or the pull request. Filing
  is the owner's.
- **When fixed** — what changes here, and what it costs.
- **Ticket** — the text to file, ready to paste.
