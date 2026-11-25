# hyper-util

## The error of a tunnel a proxy did not open cannot be named

- **Seen** — 0.1.21: `TunnelError`
  (`src/client/legacy/connect/proxy/tunnel.rs`) is a `pub enum` of a private
  module, and `proxy/mod.rs` re-exports `Tunnel` alone, so no caller can
  downcast to it. Its `Display` writes `tunnel error: ` then one of seven
  constants, one per variant; `ConnectFailed` and `Io` hand their cause out as
  `source()`.
- **Effect** — a client cannot tell by type a proxy asking for credentials
  (`407`) from one refusing, closing or resetting: it reads the sentence, which
  any release may reword.
- **Our side** — `KNOWN` in `crates/nest-rs-reqwest/src/safe_link.rs` holds six
  of the seven sentences by their text, so a line says
  `error sending request: tunnel error: proxy authorization required`, and
  beneath the last two (`failed to create underlying connection`,
  `io error establishing tunnel`) the cause as any other is said.
  `missing destination host` is left out: reqwest 0.13.5 refuses a URL with no
  host before it reaches a connector (`src/into_url.rs`, `has_host`). On a
  hyper-util bump, `Display for TunnelError` is read again and the list moved
  with it.
- **Canary** — `a_proxy_refusing_the_tunnel_is_said_by_why_it_opened_none` and
  `a_tunnel_its_connection_failed_under_is_said_with_the_reason_the_os_gave`
  (`crates/nest-rs-reqwest/tests/integration/safe_link.rs`) meet each of the
  six through reqwest over a real socket, so a sentence reworded fails its
  row. The type made public fails nothing: it is looked for on a bump.
- **Upstream** — not filed.
- **When fixed** — `below` reads the cause as `TunnelError` and says its
  variant; the six entries leave `KNOWN`. An hour.
- **Ticket** —

  > **Export `TunnelError`**
  >
  > `client::legacy::connect::proxy::Tunnel` is public and its
  > `Service::Error` is `TunnelError`, a `pub enum` the private `tunnel` module
  > keeps: it appears in a public signature and cannot be named. A caller
  > holding a `Box<dyn Error>` from a client built on it — reqwest's, for one —
  > cannot tell a proxy asking for credentials from one that refused or closed
  > except by matching the `Display` text. Could `proxy` re-export
  > `TunnelError`, `#[non_exhaustive]` if its variants are to stay free to
  > change?
