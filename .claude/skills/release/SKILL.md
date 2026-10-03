---
name: release
description: Prepare a nestrs release — scope, dependency pass, version, changelog, upgrade guide, security and performance checks — up to the tag the owner pushes. Run when the owner asks for a release.
disable-model-invocation: true
---

# `/release` — everything up to the owner's tag

1. **Scope**: a patch carries fixes only, a minor adds, a major breaks — at most
   one major every six months, its breaks queued until then (`SECURITY.md`).
2. **Dependencies**: `/deps`.
3. **Version**: `[workspace.package] version` and every `nest-rs-*` pin of the
   root `[workspace.dependencies]`, in lockstep (`.claude/rules/manifests-ci.md`).
4. **CHANGELOG**: date the `[Unreleased]` heading. Each entry says what a caller
   sees, an action required first; a major's upgrade guide lists every action a
   caller takes.
5. **Security**: `/security-review` over the release's diff; no P0 or P1 open.
6. **Performance**: the bench harness against the previous release
   (`bench/RUNBOOK.md`); a regression is explained or fixed before the tag.
7. `just ci` green on the release commit.
8. **Hand over** the commit, the tag name and the notes. The owner signs,
   pushes and tags; `publish.yml` publishes.
9. **After a major**: `SECURITY.md` dates the end of the previous line's
   security-only window.
