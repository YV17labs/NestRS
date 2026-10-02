# `check_http` stays on `Guard`, and the capability markers stop where they do

Moving `check_http` to an extension trait buys nothing: `nest-rs-guards` depends
on `nest-rs-http` unconditionally, so every build linking the guard core links
the HTTP stack whatever the consumer asked for, and a `cfg` on the method saves
no bytes. (`cargo tree -i poem` on a headless feature set names the crates a
worker actually pays for; each is its own report.)

Two residual gaps are accepted:

- A guard may declare an edge's marker trait without overriding the matching
  `check_*`. That is a deliberate line a reader sees next to the method it
  should have been. Closing it means four `check_*` traits with no defaults,
  and since every execution site holds `Arc<dyn Guard>`, a guard serving three
  edges would need three container registrations.
- `use_guards_global` takes no capability bound. A global guard legitimately
  serves whichever edges it implements; requiring `HttpGuard` would refuse a
  GraphQL-only one, and the alternative is a per-edge global list — four
  declarations where the developer wrote one.
