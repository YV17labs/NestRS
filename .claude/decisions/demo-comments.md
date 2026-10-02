# demo/ Rust carries no comments; non-Rust files keep a one-line why

The owner's rule: `demo/` is demonstration code, and the shorter it reads the
better it teaches, so its Rust carries no comments at all. During 7.0 it was
proposed to move the *why* out of the demo's non-Rust files — chart values,
`Dockerfile`, `Justfile`, manifests, `.env` — into the rules, and refused: a
reason kept away from the value it explains is one the next editor of that value
never reads. The two are reconciled by length: one short line beside the value.

A scaffolded project is the developer's own repository, so CLI templates may
teach in comments; the `// SECURITY:` note above a generated `#[public]` route
is the case that decided it.
