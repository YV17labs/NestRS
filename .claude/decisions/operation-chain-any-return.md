# The guard chain runs whatever an operation returns

Through 6.x and 7.0's first cut, `#[operations]` compiled the guard chain out of
an operation with a bare return type, since there was "nowhere to put a denial".
So `-> Vec<Secret>` under a deny-all resolver guard served the data, and an
app-wide guard protected only the fallible operations: the return type decided
the posture, which only `#[authorize]` / `#[public]` may.

Every emitted GraphQL wrapper now answers a `Result`, wrapping a bare `T`, so a
denial, a gate refusal, a pipe rejection and a masking failure all reach the
client as a field error. No return type is refused for that reason any more —
the `#[entity]` refusal of a non-`Result` return was withdrawn with it. WS and
MCP never had the condition.
