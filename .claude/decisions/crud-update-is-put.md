# `#[crud]`'s update is `PUT /{id}`, a replacement of the writable fields

Settled on 2026-10-17, by the standard, from a finding of the 7.0 foundation
run's seventh checkpoint: the demo's `PATCH /posts/{id}` refused a body that
named only some fields (`missing field body`, `400`).

`#[crud]` generated the update as `PATCH`, and its body, the `Update<E>` that
`#[expose(input(update))]` emits, carries every writable field, each required;
`Updatable::update` sets each of them on the row. RFC 5789 defines `PATCH` as a
set of changes to apply, so a client sending one field had the standard on its
side and was refused. What the route did was RFC 9110 §9.3.4's `PUT`: the
enclosed representation replaces the writable state of the target. The verb
now says so; `PATCH` at that address answers `405` with an `Allow` naming `PUT`.

**Refused:**

- **A partial `Update<E>`, every field optional, under `PATCH`** — NestJS's
  `PartialType`. A nullable column needs `Option<Option<T>>` to tell an absent
  member from a `null`, the GraphQL `#[crud]` input changes meaning with it, and
  `Updatable::update` grows a second path. A feature no caller asked for yet;
  it can be added beside `PUT` in a minor release, where a verb kept wrong could
  not be fixed without a major.

Reopened when an application needs a partial update generated rather than
written as its own route.

## 2026-11-05 — a field grant reads what a replacement changes

A `.fields` grant on `Update` now limits what a write sets, and this input sets every
writable field, so a replacement carrying back what the caller read is no write of those
columns (`field-grant-limits-a-write.md`). A partial `Update<E>` stays refused, for the
reasons above.
