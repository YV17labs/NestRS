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

**2026-11-05.** One file names the image: `.devcontainer/compose/valkey.yml`, which every Valkey service of the dev container extends and CI starts through the `dev-services` action. `ci.yml` became `framework.yml`, and no workflow names an image.

**2026-11-12.** Valkey 9.1.3, a security release, moved the pin, and RustFS moved beside it to 1.0.5 (Postgres stays at 18.6): each service of the dev container and CI runs its latest release. The scaffold's `compose.yml` names the release line its vendor publishes, `valkey/valkey:9.1` and `postgres:18`, which a fresh pull takes to the latest patch, and RustFS, which publishes no line tag, at the dev container's release; never `latest`, which crosses a major unasked. Held by `nest-rs-cli`'s unit test reading the template, which also holds its bucket service to the store's image. Sentinel and Cluster, which extend the same image, run on it by hand (`one-server-per-backend.md`).
