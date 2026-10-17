# The server is Valkey, in its latest release

`nest-rs-redis` claimed Redis 6.2.24 or later and Valkey 9.1.2 or later, and
CI ran its suites once more on each (`queue-redis-streams.md`, 2026-10-05).
The owner decided on 2026-10-08 that the framework runs on open source end to
end: the server is Valkey, and only its latest release is supported. The dev
container, `ci.yml`, `demo.yml` and the docs name one Valkey image, and move
together when Valkey ships.

What only Redis 6.2 needed went with it: the `redis.replicate_commands()` line
every script opened with (Valkey replicates a script's effects always), the
`XACK` of an entry 6.2's `XCLAIM` still answered after its deletion, and the
e2e branch reading the server's version. `nest-rs-redis`, `<PREFIX>_REDIS__*`
and `redis://` keep their names: they name the protocol Valkey speaks, and
the `redis` client that speaks it.

**Refused:**

- **Redis as a supported server**, at any version: the framework does not
  promise what the owner will not run.
- **A server matrix**, the oldest supported release beside the newest: one
  release is supported, so one is tested, in the job every test runs in.
- **Renaming the crate, the variables or the URL scheme** for Valkey: they
  name the protocol, and Valkey's own URLs are `redis://` and `rediss://`.

Reopened when Valkey's latest release lacks a command a script sends — then
the script changes in the commit that moves the image — or when the owner
supports a second server.
