# Layer contracts sit below the edges, and each edge runs its own chain

`nest-rs-guards` depends on poem, `nest-rs-http`, the three layer crates and,
behind features, on the GraphQL, WebSocket and MCP edges. The guard contract
sits above the transports it serves: a third-party guard links the HTTP stack to
implement a trait, and the in-band edges call back into guards through
function-pointer slots typed loosely to dodge a cycle — `WsMessageCheck::check`
returns `Result<(), String>`, "declared here to avoid a nest-rs-ws →
nest-rs-guards dependency cycle". `use_pipes_global` lives in guards for the
same reason, and `NoBearerChallenge`, an HTTP response marker, lives there too,
so the OAuth authorization server depends on guards for that one type.

**The direction is `nest-rs-http` → `nest-rs-guards` → `nest-rs-core`.** A
contract a layer implements sits below every edge; an edge depends on the
contracts and runs its own chain. Guards name no HTTP type: `check_http` takes
the unit's context alone and reads the request — head and body — through the
HTTP view, which `nest-rs-http` owns; `check` serves every in-band unit; and
`GuardFor<V>` declares which units a guard checks. Pipes register in
`nest-rs-pipes`, already below the edges, and each edge reads the pool once, at
mount. Interceptors and exception filters live in `nest-rs-core`
(`one-execution-pipeline.md`). Each denial renderer moves to the edge it
renders for, and `NoBearerChallenge` to `nest_rs_http::challenge`, beside the
`Bearer` challenge it opts a response out of.

**A check that guards the app runs whatever the app serves.** The global
guards' resolvability and phase checks run on the kernel's wiring step, so a
headless app with a missing global guard fails its boot too, where an HTTP
boot check runs only when HTTP serves.

`check-http-stays-on-guard.md` rests on guards depending on `nest-rs-http`;
that premise stops holding with this direction while its decision stands — the
branch that drops the dependency appends there. `macros.md`'s line that guards
"sit above the transports" changes in the same commit.

**Refused:**

- **A crate per layer family** — five, three of them typed over poem: the
  order the families run in belongs to none of them, and a family typed over
  one edge's request cannot serve another edge.
- **A dispatch crate above the edges** that knows each of them, as guards does
  through its edge features: every edge's change touches it, and an edge a
  package adds cannot join.
- **Function-pointer slots for a crate above to call back in**: a slot holds one
  implementation, is typed loosely to avoid the cycle, and is checked by
  nothing at compile time.
- **A vocabulary crate in 7.0** to hold `check_http`'s request type below
  guards: a guard reading the request through the context needs none, so the
  vocabulary stays in `nest-rs-http`, and a lower crate stays additive, later.

**Status (2026-10-10): decided, not landed.**
`refactor/guard-and-pipe-registration` moves `NoBearerChallenge`, the pipe
registration and the global-guard check onto the wiring step;
`feat/guard-contract` gives guards the context-only contract and drops their
HTTP and poem dependencies; `feat/endpoint-guards-and-class-gate` has each
in-band edge run its chain and removes the slots. Each appends an entry here
where it departs.
