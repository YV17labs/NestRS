# A module's phases are entered through an import, held by a type

"An importer runs a module's `collect`, then its `register`" was a rustdoc
paragraph. A hand-written importer that called a module's `register` alone left
what its `collect` queues unbuilt, and the app booted. CLAUDE.md holds a rule by
the first rung that can, so it moved to the types (7.0, before its release):
`Module` and `DynamicModule`'s phases take an `Imported<Self>` only nest-rs-core
makes, and `ContainerBuilder::import::<M>()` — aware of the phase it runs in —
is the one way in, running `collect` first and each phase once.

**Refused:**

- **The rustdoc protocol, kept**: a rule nothing checks fails silently for the
  one developer who never read it.
- **Detecting it at runtime**: a hand-written module's phases are plain
  function calls; nothing the container sees tells a skipped `collect` from a
  module that queues nothing.
- **A token shared by every module**: a module receiving one could hand it to
  another's `register`. `Imported<M>` is typed by its module, so it serves that
  module alone.
- **A `Wiring<Self>` wrapper in place of `ContainerBuilder`**: the same
  guarantee, at the price of every builder call in every `register` going
  through a wrapper.
- **One `wire` method called in both phases**: a register-phase read of a
  factory's output would run in the collect phase too, and find nothing.
