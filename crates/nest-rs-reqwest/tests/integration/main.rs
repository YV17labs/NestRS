//! `nest-rs-reqwest` in process: real requests from a reqwest client through
//! the middleware to a TLS peer double whose certificate a `TestAuthority`
//! issued, which keeps the head of every request it reads.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod middleware;

use std::sync::LazyLock;

use nest_rs_reqwest::ReqwestMiddleware;
use nest_rs_testing::{HttpsPeer, PeerAnswer, TestAuthority, TestCertificate};
use reqwest_middleware::ClientWithMiddleware;

/// The authority the peer's certificate chains to, and the client trusts.
static AUTHORITY: LazyLock<TestAuthority> = LazyLock::new(TestAuthority::new);

static CERTIFICATE: LazyLock<TestCertificate> = LazyLock::new(|| AUTHORITY.server(&["127.0.0.1"]));

/// A peer answering every request with `status` and an empty body.
async fn answering(status: u16) -> HttpsPeer {
    HttpsPeer::answering(&CERTIFICATE, [PeerAnswer::status(status)]).await
}

/// A peer reading every request and answering none.
async fn silent() -> HttpsPeer {
    HttpsPeer::answering(&CERTIFICATE, [PeerAnswer::Silent]).await
}

/// The value of the header `name` (lowercase) on the request `peer` read `nth`.
fn header(peer: &HttpsPeer, nth: usize, name: &str) -> Option<String> {
    peer.requests()[nth].header(name).map(str::to_owned)
}

/// A client trusting the peer's authority, through the middleware, giving up
/// on an answer past `timeout`.
fn client(timeout: std::time::Duration) -> ClientWithMiddleware {
    let authority =
        reqwest::Certificate::from_pem(AUTHORITY.pem().as_bytes()).expect("the authority");
    let client = reqwest::Client::builder()
        .tls_certs_only([authority])
        .timeout(timeout)
        .build()
        .expect("a client");
    reqwest_middleware::ClientBuilder::new(client)
        .with(ReqwestMiddleware::new())
        .build()
}
