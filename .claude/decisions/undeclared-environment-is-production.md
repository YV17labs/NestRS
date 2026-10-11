# A process that declares no environment runs as production

Settled on 2026-10-11. `openapi-docs-exposure.md` and
`server-timing-exposure.md` open a development surface "in the development and
test profiles only", yet a process that left `<PREFIX>_ENV` unset ran as
development: it read `.env.development` — where the demo turns the GraphQL
playground on — served the documentation, allowed storage's plain HTTP with the
dev container's public pair, and dialled Redis's plaintext loopback. A
forgotten variable is not a visible line.

`Environment::from_env()` now reads an unset, empty or unrecognised
`<PREFIX>_ENV` as `Production`, which is also `Environment::default()`, so an
undeclared process reads production's cascade. A development convenience lives
in a config's `Config::defaults()` behind `Environment::is_deployed` — the one
test, where each crate used to write `Production | Staging` privately — and is
armed only by a process declaring `development` (`dev`) or `test`. The managed
paths declare it: `nestrs run dev` through each workspace's `dev` recipe, the
demo's `db` recipes, and the test harness, which declares `test`.

A process that declares nothing says so once at boot, at `info` on
`nest_rs::config`: a production deployment that omits the variable is correct,
and a `warn` on correct configuration teaches operators to filter the target.
An unrecognised value is read as production and said once on stderr, by the
variable's name and never its value — a misspelt `prodution` no longer loads
the development cascade in production.

ASP.NET Core reads an unset `ASPNETCORE_ENVIRONMENT` as `Production`, and
Laravel's `config/app.php` falls back to `production`; Rails and Express fall
back to development, the convenience this refuses. Security decided: safe by
default, and the opening is one visible line on the process.

**Refused:**

- **Keeping `Development` and gating each convenience on
  `Environment::declared()`.** The cascade's file choice cannot be gated per
  key, so an undeclared process would still read `.env.development`; and every
  new convenience would have to remember the gate.
- **Refusing to boot without `<PREFIX>_ENV`.** An image that sets nothing is a
  correct production deployment; the variable would become one more line every
  deployment must carry, for no security the default does not already give.

Reopened only with a fifth profile, which says on which side of
`is_deployed` it falls.
