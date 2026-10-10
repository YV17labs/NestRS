# The development services speak TLS alone, under one generated authority

The owner decided on 2026-10-08 that we work encrypted: production is TLS, the
framework ships for security, so the hard path is the one tested and nothing
is tested in plaintext. Every service the dev container and CI run — Postgres,
Valkey, RustFS — listens on TLS alone, presenting one certificate.

`.devcontainer/scripts/issue-certificates.sh` issues it, with a self-signed authority whose key
is thrown away, into a volume each service reads its own copy from (one row
per service: its owner and the names it reads). The authority joins the
system's trust store — the dev container's at start, the CI runner's in the
`dev-services` action — so every client verifies the services as it verifies
any authority, as a company installs its private one. The framework's clients
trust the system's store by default for that reason: object_store already
did, Valkey's connection and the Postgres pool now hand the system's
authorities to their libraries, read once (`nest_rs_config::system_authorities`).
CI starts the services from the dev container's own `docker-compose.yml`, so
the two never drift.

**Refused:**

- **Committed certificates**, in a crate or beside the services: test material
  in the tree outlives its purpose and leaks the habit. A double's
  certificate is issued in process (`nest_rs_testing::TestAuthority`, rcgen).
- **Each service's own certificate** (Postgres's snakeoil and the like): one
  authority per service is one trust setting per client.
- **A per-client authority setting in the environment**, as the trust path:
  the system's store is the standard every client honours; a client's own
  authority setting stays for a deployment that wants one client alone to
  trust it.
- **Skipping verification because the services are local**: a hard "no"
  (`CLAUDE.md`), and the path production takes would go untested.
- **Testing plaintext beside TLS**: what holds encrypted holds in plaintext;
  the framework still connects to a plaintext server a deployment names.

Reopened if a service cannot read a certificate a row can hand it, or a
client the framework opens cannot be given the system's authorities.

**2026-10-08 — the certificate serves client authentication too.** A Valkey
Cluster's nodes present their certificate to each other on the cluster bus,
which always asks for one, so the certificate carries `clientAuth` beside
`serverAuth`; one issued before is issued again
(`decisions/valkey-topologies.md`).

**2026-10-10 — the database fixture verifies too.** `EphemeralDatabase` built
connect options of its own that set no `sslmode`, so sqlx's `prefer` applied:
encrypted without verifying, and plaintext with a server declining TLS, on
every e2e suite built on it. It now opens through
`SeaOrmConfig::connect_options`, the one constructor the pool, the tools and
the fixture share, which refuses a mode that does not verify.

**2026-10-10 — one client vocabulary, and Postgres keeps libpq's.** Redis,
storage and authn each grew a TLS type of their own over one variable scheme
(`RedisTls`, `StorageTls`, `AuthnTls`); the third occurrence became
`nest_rs_config::ClientTls`, read under every namespace's `TLS_CA_CERT`,
`TLS_CERT` and `TLS_KEY`, judged once and handed to each library as PEM — the
system's authorities, read once, when none is set, so every client trusts
through the same read of the store. A client that cannot present a
certificate (object_store) refuses one at boot rather than ignoring it.
Postgres is the exception on purpose: its pool reads libpq's URL parameters
(`sslmode`, `sslrootcert`, `sslcert`, `sslkey`), the connection URI every
Postgres tool and managed console shares, and a standard beats a second
spelling of the same three settings. No `<PREFIX>_SEAORM__TLS_*` variable is
added, and none of those parameters is refused.
