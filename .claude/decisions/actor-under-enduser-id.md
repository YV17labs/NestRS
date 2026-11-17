# The actor leaves on a unit's span as `enduser.id`; the lines keep `actor_id`

Settled on 2026-11-17, from item 5 of the `@nestjs/observe` audit, the owner
choosing the key.

A unit's span declared `actor_id` (`operation_span!`), filled from an inherited
correlation or by the authn guard, and `tracing-opentelemetry` exports a field
under its own name: every span left with `actor_id`, a key no tracing backend
knows, so filtering traces by user or counting the users an error touched needed
a mapping written in each backend. `@nestjs/observe` exports `enduser.id`.

- **The span declares `enduser.id`** (`nest_rs_core::__private::field::ENDUSER_ID`),
  the semantic conventions' "unique identifier of an end user in the system"
  (`opentelemetry-semantic-conventions` 0.33.1, experimental), in place of
  `actor_id`. Both writers record it there: `operation_span!` from an inherited
  correlation, the authn guard as it resolves a principal.
- **The lines keep `actor_id`**, read off the ambient correlation, never off the
  span: one word per concept where an operator greps it, the log data model's
  deviation `observability.md` records. One identity, one key per signal.
- **It is personal data, and says so.** The conventions flag `enduser.id`, and
  nestrs refused `client.address` on that ground; the actor already left on
  every span under its old name, so the choice was the name, and
  `opentelemetry/traces.mdx` now says what leaves and where.

Seen red before the change: an authenticated request's exported span named its
actor under `actor_id`, not `enduser.id`. Nothing in the framework read the
span's field by name: the queue suite's three assertions on it moved with it.

Refused:

- **`user.id`**: the conventions' "unique identifier of the user" names no end
  user an operation is served for; `enduser.id` does, which is what the guard
  records and what `@nestjs/observe` exports.
- **Exporting both keys**: two spellings of one fact on one span.
- **Renaming the lines' `actor_id`**: an operator's filter on it would go dark,
  and the log data model has no field for it to follow.
- **Renaming at export**, in a span processor, with `actor_id` kept on the
  `tracing` span: one name inside the process and another outside it, and a
  processor reading every span's attributes.
