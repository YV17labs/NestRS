# TLS fixtures

Throwaway material for `tests/integration/tls.rs`, generated once with
`openssl` and committed so the suite needs no toolchain beyond cargo:

- `tls_ca.pem` — a self-signed CA the test client trusts.
- `tls_a.pem` / `tls_a.key.pem` — leaf for `a.nestrs.test`.
- `tls_b.pem` / `tls_b.key.pem` — leaf for `b.nestrs.test`.

Two leaves under one CA, differing only in their subject name, is what makes a
certificate swap *observable*: the client trusts both, so which hostname the
handshake accepts is the only thing that changes.

These keys protect nothing. They are not valid for any real name, and nothing
outside this suite reads them.
