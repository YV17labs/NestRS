# A boot refuses one fact at a time, at the earliest phase that sees it

`App::new` cannot drain a factory queue, so a module that queued one is refused
with `UnresolvedFactoryError` before `register` runs, its remedy being
`App::builder()`. A contested declaration is checked ahead of it, since the
collect phase already holds that fact and it survives the remedy. A duplicate
provider is not: it is only seen while `register` runs, so an app that queues a
factory *and* registers a type twice is refused twice, one boot after the other.

**Refused:**

- **Running `register` on the synchronous path before refusing the queue**, to
  report the duplicate first: the register phase then runs without the outputs
  the queued factories would have built, and the first refusal it files is the
  one that hole causes — `SeaOrmDatabaseModule` naming a missing pool — which
  sends the developer after a module that is fine.
- **One error carrying every refusal**: each refusal is a typed error a caller
  matches; a bundle of them is matched by nobody, and its order would still be
  a choice.

Each refusal is a fact the boot can see at the moment it refuses, with the
remedy that fact has, as every other wiring check reports its first case.

## 2026-10-12 — the register phase keeps the order

A module first imported in a `register` whose `collect` queued a factory is
now refused at that import (`LateFactoryError`), as a refusal of its own: a
later `register` providing the type would otherwise hide it, and a refusal
the missing output causes afterwards reads after it. A late `collect` can
also declare a type again. That contest is read ahead of every refusal, a late
import's included, since it survives the remedy of each and no refusal causes
one.

**Refused:** passing over the late refusal only when the same `collect`
contested. An earlier late import's refusal would still have been read
first.
