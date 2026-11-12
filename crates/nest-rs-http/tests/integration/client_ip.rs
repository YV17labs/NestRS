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

/// Resolved before the request's trace exists, a refusal is filed inside it: an
/// operator joins it to its request, for a chain read two ways and for two
/// headers naming different clients alike.
#[tokio::test]
async fn a_refusal_is_filed_inside_the_request_it_refused_a_client_for() {
    let app = TestApp::builder()
        .module::<PinnedProxiesModule>()
        .build()
        .await
        .expect("the app boots");
    for (headers, message) in [
        (
            &[(
                "forwarded",
                "for=198.51.100.99;x=\", for=203.0.113.7;host=\",a\"",
            )][..],
            "forwarding header unreadable — no client is believed",
        ),
        (
            &[
                ("forwarded", "for=198.51.100.99"),
                ("x-forwarded-for", "203.0.113.7"),
            ],
            "forwarding headers disagree about the client — neither is believed",
        ),
        (
            &[
                ("x-forwarded-for", "198.51.100.99"),
                ("x-real-ip", "203.0.113.7"),
            ],
            "forwarding headers disagree about the client — neither is believed",
        ),
    ] {
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
        let refusal = logs.expect_one(nest_rs_http::target::HTTP, message);
        let request = logs.expect_one(
            nest_rs_core::operation_log::TARGET,
            nest_rs_http::unit::REQUEST.name(),
        );
        assert!(refusal.trace_id.is_some(), "{message}: no trace_id");
        assert_eq!(refusal.trace_id, request.trace_id, "{message}");
        assert_eq!(refusal.span_id, request.span_id, "{message}");
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
        &[
            ("x-forwarded-for", "198.51.100.99"),
            ("x-real-ip", "203.0.113.7"),
        ],
    ] {
        assert_eq!(refusals_filed(headers).await, 1, "{headers:?}");
    }
}

/// What the proxies in production write, each read off its own documentation,
/// driven through the app: the peer is the proxy facing the app, and the lines
/// are those the proxy sends on, a caller's forged hop to the left of its own.
/// No proxy binary runs here: the dev container holds none.
mod behind_a_proxy {
    use nest_rs_http::{HttpConfig, TrustedProxy};
    use nest_rs_testing::TestApp;

    use super::WhoModule;

    /// What `GET /who` answers from `peer` carrying `headers`, `trusted` listed.
    async fn answered(trusted: &[&str], peer: &str, headers: &[(&str, &str)]) -> String {
        let trusted_proxies = trusted
            .iter()
            .map(|entry| entry.parse::<TrustedProxy>().expect("an entry"))
            .collect();
        let app = TestApp::builder()
            .module::<WhoModule>()
            .provide(HttpConfig {
                trusted_proxies,
                ..HttpConfig::default()
            })
            .build()
            .await
            .expect("the app boots");
        let mut request = app.http().get("/who").peer_addr(peer.parse().unwrap());
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let answer = request.send().await;
        answer.assert_status_is_ok();
        answer.0.into_body().into_string().await.expect("a body")
    }

    /// nginx's `proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for`:
    /// the caller's own field with `$remote_addr` appended, separated by a comma
    /// (ngx_http_proxy_module), and `X-Real-IP $remote_addr` beside it, the
    /// module's example.
    #[tokio::test]
    async fn nginx_appending_to_x_forwarded_for_names_its_peer() {
        let nginx = ["10.0.0.2"];
        for (headers, client) in [
            (&[("x-forwarded-for", "203.0.113.7")][..], "203.0.113.7"),
            (
                &[("x-forwarded-for", "198.51.100.99, 203.0.113.7")],
                "203.0.113.7",
            ),
            (&[("x-forwarded-for", "2001:db8::7")], "2001:db8::7"),
            (
                &[
                    ("x-forwarded-for", "198.51.100.99, 203.0.113.7"),
                    ("x-real-ip", "203.0.113.7"),
                ],
                "203.0.113.7",
            ),
            (&[("x-real-ip", "203.0.113.7")], "203.0.113.7"),
        ] {
            assert_eq!(
                answered(&nginx, "10.0.0.2:40000", headers).await,
                format!("{client} forwarded=true"),
                "{headers:?}",
            );
        }
    }

    /// Envoy's connection manager with `use_remote_address`: the downstream
    /// address appended to `x-forwarded-for`, which its examples write with a
    /// comma and a space and its code with a comma alone; an internal Envoy
    /// behind the edge one appends the edge's address in turn.
    #[tokio::test]
    async fn envoy_appending_its_downstream_names_the_client_past_its_own_hops() {
        let mesh = ["10.0.0.0/8"];
        for (headers, client) in [
            (
                &[("x-forwarded-for", "198.51.100.99,203.0.113.7")][..],
                "203.0.113.7",
            ),
            (
                &[("x-forwarded-for", "198.51.100.99, 203.0.113.7")],
                "203.0.113.7",
            ),
            (&[("x-forwarded-for", "2001:db8::7")], "2001:db8::7"),
            (
                &[("x-forwarded-for", "203.0.113.7,10.0.0.5")],
                "203.0.113.7",
            ),
            (
                &[
                    ("x-forwarded-for", "203.0.113.7"),
                    ("x-envoy-external-address", "203.0.113.7"),
                ],
                "203.0.113.7",
            ),
        ] {
            assert_eq!(
                answered(&mesh, "10.0.0.6:40000", headers).await,
                format!("{client} forwarded=true"),
                "{headers:?}",
            );
        }
    }

    /// HAProxy's `option forwardfor` adds a field line of its own after the
    /// caller's; `option forwarded` writes RFC 7239's `forwarded` with `proto`
    /// and `for` by default, `host` and `by` when asked, a port when
    /// `for_port` asks, and an IPv6 node bracketed and quoted.
    #[tokio::test]
    async fn haproxy_adding_its_own_line_names_its_peer() {
        let haproxy = ["192.168.56.10"];
        for (headers, client) in [
            (
                &[
                    ("x-forwarded-for", "198.51.100.99"),
                    ("x-forwarded-for", "203.0.113.7"),
                ][..],
                "203.0.113.7",
            ),
            (&[("x-forwarded-for", "2001:db8::7")], "2001:db8::7"),
            (
                &[("forwarded", "proto=http;for=203.0.113.7")],
                "203.0.113.7",
            ),
            (
                &[(
                    "forwarded",
                    "proto=http;host=\"example.com\";by=\"192.168.56.10:80\";for=203.0.113.7",
                )],
                "203.0.113.7",
            ),
            (
                &[("forwarded", "proto=http;for=\"203.0.113.7:51234\"")],
                "203.0.113.7",
            ),
            (
                &[("forwarded", "proto=http;for=\"[2001:db8::7]:51234\"")],
                "2001:db8::7",
            ),
            (
                &[
                    ("forwarded", "for=198.51.100.99"),
                    ("forwarded", "proto=http;for=203.0.113.7"),
                    ("x-forwarded-for", "203.0.113.7"),
                ],
                "203.0.113.7",
            ),
        ] {
            assert_eq!(
                answered(&haproxy, "192.168.56.10:40000", headers).await,
                format!("{client} forwarded=true"),
                "{headers:?}",
            );
        }
        let passed_through = [
            ("x-forwarded-for", "198.51.100.99"),
            ("forwarded", "proto=http;for=203.0.113.7"),
        ];
        assert_eq!(
            answered(&haproxy, "192.168.56.10:40000", &passed_through).await,
            "192.168.56.10 forwarded=false",
            "a caller's own header the proxy passed through names another client",
        );
    }

    /// AWS's Application Load Balancer, in its default `append` mode: the
    /// client's address appended after a comma and a space, its port after a
    /// colon when client port preservation is on, an IPv6 address then in
    /// brackets — each example the page shows.
    #[tokio::test]
    async fn an_aws_application_load_balancer_names_the_client_it_appended() {
        let vpc = ["10.0.0.0/16"];
        for (forwarded_for, client) in [
            ("203.0.113.7", "203.0.113.7"),
            ("127.0.0.4, 127.0.0.8, 203.0.113.7", "203.0.113.7"),
            (
                "2001:DB8::21f:5bff:febf:ce22:8a2e",
                "2001:db8:0:21f:5bff:febf:ce22:8a2e",
            ),
            ("12.34.56.78:8080", "12.34.56.78"),
            (
                "[2001:db8:85a3:8d3:1319:8a2e:370:7348]:8080",
                "2001:db8:85a3:8d3:1319:8a2e:370:7348",
            ),
        ] {
            assert_eq!(
                answered(
                    &vpc,
                    "10.0.1.23:40000",
                    &[("x-forwarded-for", forwarded_for)]
                )
                .await,
                format!("{client} forwarded=true"),
                "{forwarded_for}",
            );
        }
    }

    /// Google Cloud's external Application Load Balancer appends the client's
    /// address and then its forwarding rule's, with no space, and connects from
    /// its front ends (`35.191.0.0/16`, `130.211.0.0/22`): the rule's address is
    /// one of the deployment's own hops, listed like the front ends, or the
    /// balancer itself answers as every caller.
    #[tokio::test]
    async fn a_google_cloud_load_balancer_names_the_client_once_its_rule_is_listed() {
        let chain = [("x-forwarded-for", "192.0.2.50,203.0.113.10,198.51.100.5")];
        let front_ends = ["35.191.0.0/16", "130.211.0.0/22"];
        assert_eq!(
            answered(&front_ends, "35.191.0.20:40000", &chain).await,
            "198.51.100.5 forwarded=true",
            "the rule's address unlisted answers as every caller",
        );
        let deployment = ["35.191.0.0/16", "130.211.0.0/22", "198.51.100.5"];
        assert_eq!(
            answered(&deployment, "35.191.0.20:40000", &chain).await,
            "203.0.113.10 forwarded=true",
        );
        let behind_the_backend_s_proxy = [(
            "x-forwarded-for",
            "192.0.2.50,203.0.113.10,198.51.100.5,35.191.0.20,10.128.0.2",
        )];
        let with_the_backend_s_proxy = [
            "35.191.0.0/16",
            "130.211.0.0/22",
            "198.51.100.5",
            "10.128.0.0/20",
        ];
        assert_eq!(
            answered(
                &with_the_backend_s_proxy,
                "10.128.0.3:40000",
                &behind_the_backend_s_proxy,
            )
            .await,
            "203.0.113.10 forwarded=true",
        );
    }

    /// RFC 7239's own examples (§4, §7.1): a parameter name in any case, an
    /// IPv6 node quoted with its port, `proto` and `by` beside `for`, and a
    /// listed hop to the right of the client.
    #[tokio::test]
    async fn rfc_7239_s_examples_name_their_client() {
        let proxies = ["198.51.100.17", "203.0.113.43"];
        for (forwarded, client) in [
            ("For=\"[2001:db8:cafe::17]:4711\"", "2001:db8:cafe::17"),
            ("for=192.0.2.60;proto=http;by=203.0.113.43", "192.0.2.60"),
            ("for=192.0.2.43, for=198.51.100.17", "192.0.2.43"),
            (
                "for=192.0.2.43,for=\"[2001:db8:cafe::17]\"",
                "2001:db8:cafe::17",
            ),
        ] {
            assert_eq!(
                answered(&proxies, "203.0.113.43:40000", &[("forwarded", forwarded)]).await,
                format!("{client} forwarded=true"),
                "{forwarded}",
            );
        }
    }

    /// A node the proxy withheld where the client stands — RFC 7239's
    /// `unknown` (§7.4's example) or an obfuscated identifier, as HAProxy's
    /// `for-expr` writes one — names no client: the proxy answers as itself.
    #[tokio::test]
    async fn a_client_the_proxy_withheld_answers_as_the_proxy() {
        for forwarded in [
            "for=192.0.2.43,for=\"[2001:db8:cafe::17]\",for=unknown",
            "host=\"example.com\";by=_loadbalancer1;for=\"_000000008D7250D3:_000000006BABCF75\"",
            "for=\"_gazonk\"",
        ] {
            assert_eq!(
                answered(
                    &["203.0.113.43"],
                    "203.0.113.43:40000",
                    &[("forwarded", forwarded)]
                )
                .await,
                "203.0.113.43 forwarded=false",
                "{forwarded}",
            );
        }
    }
}
