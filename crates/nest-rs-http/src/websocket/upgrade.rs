//! [`WebSocketUpgrade`], the server's half of the WebSocket opening handshake
//! (RFC 6455 §4.2), on the socket hyper hands over once the `101` is written.

use std::future::Future;

use http::header::{
    CONNECTION, HeaderName, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_KEY, SEC_WEBSOCKET_PROTOCOL,
    SEC_WEBSOCKET_VERSION, UPGRADE,
};
use http::{HeaderMap, HeaderValue, Method, StatusCode};
use hyper::upgrade::{OnUpgrade, Upgraded};
use hyper_util::rt::TokioIo;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};

use super::token::{elements, has_token, requested};
use crate::allow::HeadAsGet;
use crate::error::HandshakeRefused;
use crate::request::Request;
use crate::response::Response;

/// The socket an accepted upgrade hands its callback: frames over the
/// connection the accept loop served, its place under the connection cap and
/// its send deadline kept.
pub type WebSocketStream = tokio_tungstenite::WebSocketStream<TokioIo<Upgraded>>;

/// What a socket's protocol layer holds its peer to.
#[derive(Clone, Copy, Debug)]
pub struct WebSocketLimits {
    max_message_bytes: usize,
}

impl WebSocketLimits {
    /// A message, and each frame of one, held to `max_message_bytes`: a peer
    /// past it is refused before the protocol layer buffers it.
    pub const fn new(max_message_bytes: usize) -> Self {
        Self { max_message_bytes }
    }

    fn config(self) -> WebSocketConfig {
        WebSocketConfig::default()
            .max_message_size(Some(self.max_message_bytes))
            .max_frame_size(Some(self.max_message_bytes))
    }
}

/// An opening handshake the server admitted, waiting to be accepted.
pub struct WebSocketUpgrade {
    /// `Sec-WebSocket-Accept`, derived from the client's key.
    accept: HeaderValue,
    /// Every `Sec-WebSocket-Protocol` line the client sent.
    offered: Vec<HeaderValue>,
    on_upgrade: OnUpgrade,
}

impl WebSocketUpgrade {
    /// The handshake `req` opens, checked as RFC 6455 §4.2.1 reads it: a
    /// `GET` (a `HEAD` the route table hands its `GET` entry is none),
    /// `Upgrade: websocket` and `Connection: upgrade` as tokens in any case,
    /// one `Sec-WebSocket-Key` of 16 base64-encoded bytes, then one
    /// `Sec-WebSocket-Version: 13`; last, the connection under `req` must be
    /// able to switch, whose upgrade this takes.
    pub fn from_request(req: &mut Request) -> Result<Self, HandshakeRefused> {
        if req.method() != Method::GET || req.extensions().get::<HeadAsGet>().is_some() {
            return Err(HandshakeRefused::MethodNotGet);
        }
        let headers = req.headers();
        if !requested(headers) {
            return Err(HandshakeRefused::UpgradeNotWebSocket);
        }
        if !has_token(headers, &CONNECTION, "upgrade") {
            return Err(HandshakeRefused::ConnectionNotUpgrade);
        }
        let accept = only(headers, &SEC_WEBSOCKET_KEY)
            .filter(|key| is_nonce(key.as_bytes()))
            .and_then(|key| HeaderValue::try_from(derive_accept_key(key.as_bytes())).ok())
            .ok_or(HandshakeRefused::KeyMalformed)?;
        if only(headers, &SEC_WEBSOCKET_VERSION).is_none_or(|version| version != "13") {
            return Err(HandshakeRefused::VersionUnsupported);
        }
        let offered = headers
            .get_all(SEC_WEBSOCKET_PROTOCOL)
            .iter()
            .cloned()
            .collect();
        let on_upgrade = req
            .extensions_mut()
            .remove::<OnUpgrade>()
            .ok_or(HandshakeRefused::NotUpgradable)?;
        Ok(Self {
            accept,
            offered,
            on_upgrade,
        })
    }

    /// The subprotocols the client offered, in its order of preference.
    pub fn protocols(&self) -> impl Iterator<Item = &str> {
        elements(&self.offered)
    }

    /// The `101` switching to the WebSocket protocol, naming `protocol` as the
    /// client spelled it when it offered it, in any case; `on_upgrade` then
    /// runs on a task of its own with the socket, held to `limits`, once the
    /// `101` is written.
    pub fn accept<F, Fut>(
        self,
        protocol: Option<&str>,
        limits: WebSocketLimits,
        on_upgrade: F,
    ) -> Response
    where
        F: FnOnce(WebSocketStream) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let chosen = protocol
            .and_then(|chosen| {
                self.protocols()
                    .find(|offered| offered.eq_ignore_ascii_case(chosen))
            })
            .and_then(|offered| HeaderValue::from_str(offered).ok());
        let Self {
            accept,
            on_upgrade: upgrade,
            ..
        } = self;
        tokio::spawn(serve(upgrade, limits, on_upgrade));
        let response = Response::builder()
            .status(StatusCode::SWITCHING_PROTOCOLS)
            .header(CONNECTION, HeaderValue::from_static("upgrade"))
            .header(UPGRADE, HeaderValue::from_static("websocket"))
            .header(SEC_WEBSOCKET_ACCEPT, accept);
        match chosen {
            Some(chosen) => response.header(SEC_WEBSOCKET_PROTOCOL, chosen),
            None => response,
        }
        .finish()
    }
}

/// Wait for the connection to switch, then run `on_upgrade` over the socket;
/// a panic in it drops the socket and is filed, never re-raised.
async fn serve<F, Fut>(upgrade: OnUpgrade, limits: WebSocketLimits, on_upgrade: F)
where
    F: FnOnce(WebSocketStream) -> Fut,
    Fut: Future<Output = ()>,
{
    let upgraded = match upgrade.await {
        Ok(upgraded) => upgraded,
        Err(error) => {
            tracing::debug!(
                target: crate::target::HTTP,
                error = %nest_rs_core::error_message(&error),
                "an accepted WebSocket upgrade never switched; the connection ended first",
            );
            return;
        }
    };
    let socket = WebSocketStream::from_raw_socket(
        TokioIo::new(upgraded),
        Role::Server,
        Some(limits.config()),
    )
    .await;
    if let Err(payload) =
        nest_rs_core::panic::contain(async move { on_upgrade(socket).await }).await
    {
        nest_rs_core::contained_panic!(
            target: crate::target::HTTP,
            payload.as_ref(),
            "an upgraded socket's task panicked; its connection is dropped",
        );
    }
}

/// The one `name` line, `None` when it is absent or repeated: RFC 6455 §11.3
/// lets neither the key nor the version appear twice in a request.
fn only<'a>(headers: &'a HeaderMap, name: &HeaderName) -> Option<&'a HeaderValue> {
    let mut lines = headers.get_all(name).iter();
    let line = lines.next()?;
    lines.next().is_none().then_some(line)
}

/// Whether `key` is 16 bytes in base64 (RFC 4648 §4): 22 characters of the
/// alphabet and the two pads 128 bits leave.
fn is_nonce(key: &[u8]) -> bool {
    let Some((encoded, padding)) = key.split_at_checked(22) else {
        return false;
    };
    padding == b"=="
        && encoded
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/'))
}

#[cfg(test)]
mod tests {
    use http::Uri;

    use super::*;
    use crate::testing::{Peer, Upgrading};

    /// A handshake as a browser sends it, over a connection hyper can switch,
    /// and the client across it.
    async fn handshake() -> (crate::RequestBuilder, Peer) {
        let Upgrading { request, peer } = Upgrading::open().await;
        let upgrade = request
            .extensions()
            .get::<OnUpgrade>()
            .cloned()
            .expect("hyper put the upgrade on the request");
        let req = Request::builder()
            .uri(Uri::from_static("/socket"))
            .header(UPGRADE, HeaderValue::from_static("websocket"))
            .header(CONNECTION, HeaderValue::from_static("keep-alive, Upgrade"))
            .header(
                SEC_WEBSOCKET_KEY,
                HeaderValue::from_static("dGhlIHNhbXBsZSBub25jZQ=="),
            )
            .header(SEC_WEBSOCKET_VERSION, HeaderValue::from_static("13"))
            .extension(upgrade);
        (req, peer)
    }

    fn refused(mut req: Request) -> HandshakeRefused {
        match WebSocketUpgrade::from_request(&mut req) {
            Ok(_) => panic!("the handshake was admitted"),
            Err(refused) => refused,
        }
    }

    #[tokio::test]
    async fn rfc6455_section_1_3_example_key_answers_its_accept() {
        let (req, _peer) = handshake().await;
        let mut req = req.finish();
        let upgrade = WebSocketUpgrade::from_request(&mut req).expect("the RFC's own handshake");
        assert_eq!(upgrade.accept, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
        assert!(
            req.extensions().get::<OnUpgrade>().is_none(),
            "the upgrade is taken off the request",
        );
    }

    #[tokio::test]
    async fn a_wrong_version_answers_426_with_the_supported_one() {
        for version in [Some("8"), Some("13, 8"), None] {
            let (req, _peer) = handshake().await;
            let mut req = req.finish();
            req.headers_mut().remove(SEC_WEBSOCKET_VERSION);
            if let Some(version) = version {
                req.headers_mut()
                    .insert(SEC_WEBSOCKET_VERSION, HeaderValue::from_static(version));
            }
            let refusal = refused(req);
            assert_eq!(refusal, HandshakeRefused::VersionUnsupported, "{version:?}");
            let response = crate::ResponseError::as_response(&refusal);
            assert_eq!(response.status(), StatusCode::UPGRADE_REQUIRED);
            assert_eq!(response.headers()[SEC_WEBSOCKET_VERSION], "13");
            assert_eq!(response.headers()[UPGRADE], "websocket");
            assert_eq!(response.headers()[CONNECTION], "upgrade");
        }
    }

    /// The handshake cannot know what else the address serves, so it names
    /// no `Allow`: the route table's `405` does.
    #[tokio::test]
    async fn a_handshake_that_is_no_get_answers_400_naming_get() {
        let (post, _peer) = handshake().await;
        let mut post = post.finish();
        post.head_mut().method = Method::POST;
        let (head, _peer) = handshake().await;
        let head = head.extension(HeadAsGet).finish();
        for req in [post, head] {
            let refusal = refused(req);
            assert_eq!(refusal, HandshakeRefused::MethodNotGet);
            let response = crate::ResponseError::as_response(&refusal);
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            assert!(response.headers().get(http::header::ALLOW).is_none());
            let body = response.into_body().into_string().await.expect("a body");
            assert!(body.contains("GET"), "{body}");
        }
    }

    #[tokio::test]
    async fn each_header_the_handshake_lacks_answers_400_naming_it() {
        let cases: [(HeaderName, Option<&'static str>, HandshakeRefused, &str); 7] = [
            (
                UPGRADE,
                None,
                HandshakeRefused::UpgradeNotWebSocket,
                "Upgrade",
            ),
            (
                UPGRADE,
                Some("h2c"),
                HandshakeRefused::UpgradeNotWebSocket,
                "Upgrade",
            ),
            (
                CONNECTION,
                None,
                HandshakeRefused::ConnectionNotUpgrade,
                "Connection",
            ),
            (
                CONNECTION,
                Some("keep-alive"),
                HandshakeRefused::ConnectionNotUpgrade,
                "Connection",
            ),
            (
                SEC_WEBSOCKET_KEY,
                None,
                HandshakeRefused::KeyMalformed,
                "Sec-WebSocket-Key",
            ),
            (
                SEC_WEBSOCKET_KEY,
                Some("test_key"),
                HandshakeRefused::KeyMalformed,
                "Sec-WebSocket-Key",
            ),
            (
                SEC_WEBSOCKET_KEY,
                Some("dGhlIHNhbXBsZSBub25jZQ"),
                HandshakeRefused::KeyMalformed,
                "Sec-WebSocket-Key",
            ),
        ];
        for (name, value, expected, named) in cases {
            let (req, _peer) = handshake().await;
            let mut req = req.finish();
            req.headers_mut().remove(&name);
            if let Some(value) = value {
                req.headers_mut()
                    .insert(name.clone(), HeaderValue::from_static(value));
            }
            let refusal = refused(req);
            assert_eq!(refusal, expected, "{name}: {value:?}");
            let response = crate::ResponseError::as_response(&refusal);
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{name}");
            let body = response.into_body().into_string().await.expect("a body");
            assert!(body.contains(named), "{name}: {body}");
            if let Some(value) = value {
                assert!(
                    !body.contains(value),
                    "the value sent is never quoted: {body}"
                );
            }
        }
    }

    #[tokio::test]
    async fn a_key_sent_twice_is_refused() {
        let (req, _peer) = handshake().await;
        let mut req = req.finish();
        req.headers_mut().append(
            SEC_WEBSOCKET_KEY,
            HeaderValue::from_static("dGhlIHNhbXBsZSBub25jZQ=="),
        );
        assert_eq!(refused(req), HandshakeRefused::KeyMalformed);
    }

    #[tokio::test]
    async fn the_tokens_are_read_in_any_case_and_from_any_line() {
        let (req, _peer) = handshake().await;
        let mut req = req.finish();
        req.headers_mut()
            .insert(UPGRADE, HeaderValue::from_static("WebSocket"));
        req.headers_mut().remove(CONNECTION);
        req.headers_mut()
            .append(CONNECTION, HeaderValue::from_static("keep-alive"));
        req.headers_mut()
            .append(CONNECTION, HeaderValue::from_static("UPGRADE"));
        assert!(WebSocketUpgrade::from_request(&mut req).is_ok());
    }

    #[tokio::test]
    async fn a_connection_that_cannot_switch_answers_426() {
        let (req, _peer) = handshake().await;
        let mut req = req.finish();
        req.extensions_mut().remove::<OnUpgrade>();
        let refusal = refused(req);
        assert_eq!(refusal, HandshakeRefused::NotUpgradable);
        let response = crate::ResponseError::as_response(&refusal);
        assert_eq!(response.status(), StatusCode::UPGRADE_REQUIRED);
        assert_eq!(response.headers()[UPGRADE], "websocket");
        assert_eq!(response.headers()[CONNECTION], "upgrade");
    }

    #[tokio::test]
    async fn the_offered_protocols_are_read_in_order_across_lines() {
        let (req, _peer) = handshake().await;
        let mut req = req
            .header(
                SEC_WEBSOCKET_PROTOCOL,
                HeaderValue::from_static("graphql-transport-ws , chat"),
            )
            .header(
                SEC_WEBSOCKET_PROTOCOL,
                HeaderValue::from_static("graphql-ws"),
            )
            .finish();
        let upgrade = WebSocketUpgrade::from_request(&mut req).expect("admitted");
        assert_eq!(
            upgrade.protocols().collect::<Vec<_>>(),
            ["graphql-transport-ws", "chat", "graphql-ws"],
        );
    }

    #[tokio::test]
    async fn the_101_names_the_accept_key_and_only_a_protocol_the_client_offered() {
        let (req, _offering) = handshake().await;
        let mut req = req
            .header(SEC_WEBSOCKET_PROTOCOL, HeaderValue::from_static("Chat"))
            .finish();
        let upgrade = WebSocketUpgrade::from_request(&mut req).expect("admitted");
        let response = upgrade.accept(Some("chat"), WebSocketLimits::new(64), |_| async {});
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
        assert_eq!(
            response.headers()[SEC_WEBSOCKET_ACCEPT],
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
        assert_eq!(
            response.headers()[SEC_WEBSOCKET_PROTOCOL],
            "Chat",
            "the client's own spelling"
        );
        assert_eq!(response.headers()[UPGRADE], "websocket");
        assert_eq!(response.headers()[CONNECTION], "upgrade");

        let (req, _silent) = handshake().await;
        let mut req = req.finish();
        let upgrade = WebSocketUpgrade::from_request(&mut req).expect("admitted");
        let response = upgrade.accept(Some("chat"), WebSocketLimits::new(64), |_| async {});
        assert!(
            response.headers().get(SEC_WEBSOCKET_PROTOCOL).is_none(),
            "a protocol the client did not offer is never named",
        );
    }

    #[test]
    fn a_nonce_is_exactly_sixteen_bytes_of_base64() {
        assert!(is_nonce(b"dGhlIHNhbXBsZSBub25jZQ=="));
        assert!(is_nonce(b"AAAAAAAAAAAAAAAAAAAAAA=="));
        assert!(!is_nonce(b"dGhlIHNhbXBsZSBub25jZQ="));
        assert!(!is_nonce(b"dGhlIHNhbXBsZSBub25jZQ==="));
        assert!(!is_nonce(b"dGhlIHNhbXBsZSBub25jZ-=="));
        assert!(!is_nonce(b""));
    }
}
