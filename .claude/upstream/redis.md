# redis

## A Cluster has no setting to skip `CLIENT SETINFO` on its nodes

- **Seen** — 1.8.1: a node's `RedisConnectionInfo` is built from the cluster's
  parameters with `..Default::default()` (`src/cluster_handling/mod.rs:185-191`),
  so `skip_set_lib_name` stays `false`, and every connection to a node
  pipelines `CLIENT SETINFO LIB-NAME` and `LIB-VER`, its reply ignored
  (`src/connection.rs:1443-1467`). `ClusterClientBuilder` offers no setting for
  it. 2.0.0-rc.3 builds a node's settings the same way.
- **Effect** — an ACL user confined to the commands a binding sends, as the
  queue, rate-limiting and schedule pages write each binding's ACL, is refused
  `CLIENT SETINFO` on every connection to every node: the connection opens, and
  each refusal lands in the server's `ACL LOG`, where an operator audits
  denials.
- **Our side** — each node is opened through `ClusterNode`, a `Connect` of the
  framework's that sets `skip_set_lib_name`, reached through the
  `#[doc(hidden)]` `ClusterClient::get_async_generic_connection`
  (`crates/nest-rs-redis/src/cluster.rs`). A hidden item may change in any
  release, so `redis` is pinned `=` (`.claude/decisions/valkey-topologies.md`).
- **Canary** — none: a builder setting is read in the release notes when the
  version moves.
- **Upstream** — not filed.
- **When fixed** — the setting replaces `skip_set_lib_name` in `ClusterNode`.
  The seam and the `=` pin go only with the next entry fixed too, since
  `ClusterNode` also hears a node's TLS refusal. An hour, the Cluster suite
  included.
- **Ticket** —

  > **Cluster: a setting to skip `CLIENT SETINFO` on each node's connection**
  >
  > A standalone or Sentinel connection skips `CLIENT SETINFO` through
  > `RedisConnectionInfo::set_skip_set_lib_name`, but a cluster connection
  > builds each node's `RedisConnectionInfo` from `ClusterParams` with
  > `..Default::default()`, so every connection to every node sends it. Its
  > reply is ignored, yet a user whose ACL grants only the commands the
  > application needs is refused it, and each refusal fills the server's
  > `ACL LOG`. Could `ClusterClientBuilder` take a `skip_set_lib_name()` (or
  > the library name and version) and carry it into each node's connection
  > info?

## A TLS refusal read before a command is in flight is dropped

- **Seen** — 1.8.1: the multiplexed connection hands a result read off the
  socket to the oldest request in flight, and returns without one
  (`PipelineSink::send_result`, `None => return`,
  `src/aio/multiplexed_connection.rs:333-336`). TLS 1.3 settles the client's
  certificate after the client's side of the handshake, so a server refusing it
  sends the alert before the first command: the alert is dropped and the first
  command meets a closed connection. 2.0.0-rc.3 keeps the same `None => return`.
- **Effect** — a server refusing the client's certificate reads as a dropped
  connection, a transient failure retried for the whole boot budget, where the
  refusal repeats on every attempt and names its cause.
- **Our side** — after a dropped connection, `tls::hello` opens a blocking
  connection of its own, which writes before it reads, and hears the refusal
  (`tls::refusal`, `crates/nest-rs-redis/src/tls.rs`). Every topology's link
  sends its first `HELLO` through it, a Cluster's nodes through `ClusterNode`.
- **Canary** — `nest-rs-redis`'s unit test
  `tls::tests::the_first_hello_hears_a_client_certificate_refused_before_it_was_sent`,
  which asserts that the first command meets a dropped connection rather than
  the refusal.
- **Upstream** — not filed.
- **When fixed** — `tls::refusal` and its blocking handshake go, and `hello`
  reads the refusal off its own command's error. An hour, the canary turned
  into the test of the new path.
- **Ticket** —

  > **Multiplexed connection: an error read with no request in flight is dropped**
  >
  > `PipelineSink::send_result` returns when `in_flight` is empty, so a
  > result read before any command was sent is discarded. Over TLS 1.3, a server
  > refusing the client's certificate sends its alert right after the
  > handshake, before the first command: the alert is lost, and the first
  > command fails as a dropped connection, which a caller retries. Could an
  > error read with nothing in flight be kept and handed to the next request
  > (or close the connection with it as the cause), so the caller sees
  > `certificate required` or `unknown ca`?

## A refused `SELECT` loses the server's code

- **Seen** — 1.8.1: `check_db_select` turns the server's answer to `SELECT` into
  a `ResponseError` under its own sentence, "Redis server refused to switch
  database", keeping the server's text as the detail and dropping its code
  (`src/connection.rs:1524-1541`). 2.0.0-rc.3 does the same.
- **Effect** — an ACL user without `+select` (`NOPERM`) and an index out of
  range (`ERR`) read alike, and neither carries the code a caller classifies
  an error by: the client's own verdict retries both.
- **Our side** — `database_refused` matches the sentence and reads the detail,
  so a refused `SELECT` fails the boot at once unless the server was busy
  (`crates/nest-rs-redis/src/connection.rs`).
- **Canary** — `nest-rs-redis`'s e2e tests
  `connection::a_database_index_redis_does_not_have_fails_the_boot_at_once` and
  `connection::an_acl_denying_select_fails_the_boot_at_once_naming_the_index`,
  which hold the sentence to the client against a live server.
- **Upstream** — not filed.
- **When fixed** — `database_refused` and its sentence go, and `refused` reads
  the code as it reads any other answer's. Minutes.
- **Ticket** —

  > **A refused `SELECT` should keep the server's error code**
  >
  > When the server refuses the `SELECT` a connection sends for its database,
  > `check_db_select` reports `ResponseError` with the sentence "Redis server
  > refused to switch database" and the server's text as the detail, so
  > `RedisError::code()` no longer says `NOPERM` (an ACL without `+select`) or
  > `ERR` (an index out of range). A caller deciding whether to retry cannot
  > tell them from a transient `BUSY` without matching the detail. Could the
  > error keep the server's code, as other server errors do?
