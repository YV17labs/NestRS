//! `src/client_ip.rs`: who a request is attributed to, over a real socket: what
//! a listener on both families reports is the kernel's to say.

use std::net::IpAddr;

use nest_rs_core::module;
use nest_rs_http::{ClientIp, HttpConfig, HttpModule, controller, routes, trusted_proxy};
use nest_rs_testing::TestApp;
use tokio::io::AsyncWriteExt;

use crate::transport::{connect, read_answer, serve_module_on};

#[controller(path = "/who")]
struct WhoController;

#[routes]
impl WhoController {
    #[get("/")]
    #[public]
    async fn who(&self, ip: ClientIp) -> String {
        format!("{} forwarded={}", ip.ip, ip.forwarded)
    }
}

#[module(providers = [WhoController])]
struct WhoModule;

/// `GET /who`, over IPv4, from a listener on every address of both families.
async fn asked_over_ipv4(config: HttpConfig, forwarded_for: &str) -> String {
    let serving = serve_module_on::<WhoModule>("[::]", config).await;
    let mut stream = connect(serving.port).await;
    stream
        .write_all(
            format!("GET /who HTTP/1.1\r\nHost: localhost\r\n{forwarded_for}\r\n").as_bytes(),
        )
        .await
        .expect("the request is sent");
    let (head, body) = read_answer(&mut stream).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    serving.cancel.cancel();
    body
}

/// A listener on `[::]` reports an IPv4 connection as `::ffff:a.b.c.d`. The
/// proxy's IPv4 entry names it all the same, so the hop it forwards is the
/// client, not the proxy.
#[tokio::test]
async fn a_dual_stack_listener_reads_an_ipv4_proxy_by_its_entry() {
    let config = HttpConfig {
        trusted_proxies: vec!["127.0.0.1".parse().unwrap()],
        ..HttpConfig::default()
    };
    assert_eq!(
        asked_over_ipv4(config, "X-Forwarded-For: 203.0.113.7\r\n").await,
        "203.0.113.7 forwarded=true"
    );
}

/// Off any proxy the same connection is its IPv4 peer, as a client reads it,
/// never the mapped spelling only the socket uses.
#[tokio::test]
async fn a_dual_stack_listener_attributes_an_ipv4_client_to_its_ipv4_address() {
    assert_eq!(
        asked_over_ipv4(HttpConfig::default(), "").await,
        "127.0.0.1 forwarded=false"
    );
}

/// A proxy adding its own `X-Forwarded-For` line after the caller's: the lines
/// reach the app as sent, and the client is the rightmost hop of both.
#[tokio::test]
async fn a_listed_proxy_s_own_field_line_outranks_the_caller_s() {
    let config = HttpConfig {
        trusted_proxies: vec!["127.0.0.1".parse().unwrap()],
        ..HttpConfig::default()
    };
    assert_eq!(
        asked_over_ipv4(
            config,
            "X-Forwarded-For: 198.51.100.99\r\nX-Forwarded-For: 203.0.113.7\r\n",
        )
        .await,
        "203.0.113.7 forwarded=true"
    );
}

/// The list as a composition root pins it, where no `?` reaches a `Result`: a
/// literal checked as the module compiles, and an address a value holds.
#[module(
    imports = [HttpModule::for_root(HttpConfig {
        trusted_proxies: vec![
            IpAddr::from([10, 0, 0, 1]).into(),
            trusted_proxy!("10.42.0.0/16"),
        ],
        ..HttpConfig::default()
    })],
    providers = [WhoController],
)]
struct PinnedProxiesModule;

#[tokio::test]
async fn a_composition_root_pins_an_address_and_a_range() {
    let app = TestApp::builder()
        .module::<PinnedProxiesModule>()
        .build()
        .await
        .expect("the app boots");
    for peer in ["10.0.0.1:40000", "10.42.0.9:40000"] {
        app.http()
            .get("/who")
            .peer_addr(peer.parse().unwrap())
            .header("x-forwarded-for", "203.0.113.7")
            .send()
            .await
            .assert_text("203.0.113.7 forwarded=true")
            .await;
    }
}

/// How many times a listed peer's `GET /who` with `headers`, answered as its
/// proxy's, was said to name no client.
async fn refusals_filed(headers: &[(&str, &str)]) -> usize {
    let app = TestApp::builder()
        .module::<PinnedProxiesModule>()
        .build()
        .await
        .expect("the app boots");
    let logs = nest_rs_testing::LogCapture::install();
    let mut request = app
        .http()
        .get("/who")
        .peer_addr("10.0.0.1:40000".parse().unwrap());
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    request
        .send()
        .await
        .assert_text("10.0.0.1 forwarded=false")
        .await;
    [
        "forwarding header unreadable — no client is believed",
        "forwarding headers disagree about the client — neither is believed",
    ]
    .iter()
    .map(|message| logs.find(nest_rs_http::target::HTTP, message).len())
    .sum()
}

/// One request, one line: the edge resolves who sent it once, and a route
/// reading `ClientIp` reads that resolution rather than filing its own.
#[tokio::test]
async fn a_request_whose_headers_name_no_client_is_said_so_once() {
    for headers in [
        &[(
            "forwarded",
            "for=198.51.100.99;x=\", for=203.0.113.7;host=\",a\"",
        )][..],
        &[
            ("forwarded", "for=198.51.100.99"),
            ("x-forwarded-for", "203.0.113.7"),
        ],
    ] {
        assert_eq!(refusals_filed(headers).await, 1, "{headers:?}");
    }
}
