# TLS fixtures

Throwaway material for `tests/e2e/tls.rs`, generated once with `openssl` and
committed so the suite needs no toolchain beyond cargo:

- `tls_ca.pem` — a self-signed CA the tests pin as the trust anchor.
- `tls_server.pem` / `tls_server.key.pem` — the leaf the test's TLS proxy
  presents in front of the dev container Redis, for `localhost` and
  `127.0.0.1`.
- `tls_client.pem` / `tls_client.key.pem` — a client certificate under the same
  CA, for the mutual-TLS case.

These keys protect nothing. They are not valid for any real name, and nothing
outside this suite reads them.
