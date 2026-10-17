//! The kit's client drives the stack the server runs, edge included: in process
//! a request has no peer, so `ClientIp` resolves nothing until the request
//! names one, and the hop a trusted proxy forwards is believed as on a socket.

use std::net::SocketAddr;

use nest_rs_core::module;
use nest_rs_http::{ClientIp, HttpConfig, controller, routes};
use nest_rs_testing::TestApp;

/// The proxy the deployment trusts.
const PROXY: &str = "10.0.0.9:5000";
/// The client the proxy forwards for.
const CLIENT: &str = "203.0.113.7";

#[controller(path = "/")]
#[derive(Default)]
struct WhoController;

#[routes]
impl WhoController {
    #[get("/who")]
    async fn who(&self, ip: ClientIp) -> String {
        format!("{} forwarded={}", ip.ip, ip.forwarded)
    }
}

#[module(providers = [WhoController])]
struct WhoModule;

async fn behind_the_proxy() -> TestApp {
    let proxy: SocketAddr = PROXY.parse().unwrap();
    TestApp::builder()
        .module::<WhoModule>()
        .provide(HttpConfig {
            trusted_proxies: vec![proxy.ip()],
            ..HttpConfig::default()
        })
        .build()
        .await
        .expect("the module boots")
}

#[tokio::test]
async fn without_a_peer_the_client_ip_is_unknown_and_no_header_is_believed() {
    let app = behind_the_proxy().await;
    app.http()
        .get("/who")
        .header("x-forwarded-for", CLIENT)
        .send()
        .await
        .assert_text("0.0.0.0 forwarded=false")
        .await;
}

#[tokio::test]
async fn a_trusted_peer_has_the_hop_it_forwards_believed() {
    let app = behind_the_proxy().await;
    app.http()
        .get("/who")
        .peer_addr(PROXY.parse().unwrap())
        .header("x-forwarded-for", CLIENT)
        .send()
        .await
        .assert_text(format!("{CLIENT} forwarded=true"))
        .await;
}

#[tokio::test]
async fn an_untrusted_peer_is_the_client_whatever_it_forwards() {
    let app = behind_the_proxy().await;
    app.http()
        .get("/who")
        .peer_addr("198.51.100.4:6000".parse().unwrap())
        .header("x-forwarded-for", CLIENT)
        .send()
        .await
        .assert_text("198.51.100.4 forwarded=false")
        .await;
}
