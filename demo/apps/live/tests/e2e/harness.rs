use features::Role;
use futures_util::{SinkExt, StreamExt};
use live::LiveModule;
use nest_rs_authn::JwtConfig;
use nest_rs_http::poem::http::header;
use nest_rs_testing::TestApp;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use uuid::Uuid;

pub(crate) use features::testing::{DEV_PUBLIC_KEY, ORG_ID};

pub(crate) async fn test_token() -> String {
    token_for_org(Uuid::parse_str(ORG_ID).expect("valid org uuid"), Role::User).await
}

pub(crate) async fn token_for_org(org_id: Uuid, role: Role) -> String {
    features::testing::token(org_id, vec![role], None)
}

pub(crate) fn boot_builder() -> nest_rs_testing::TestAppBuilder {
    TestApp::builder()
        .module::<LiveModule>()
        .with_test_telemetry()
        .provide(JwtConfig {
            public_key: Some(DEV_PUBLIC_KEY.into()),
            ..Default::default()
        })
}

pub(crate) type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

pub(crate) async fn assert_no_frame(socket: &mut Socket) {
    match tokio::time::timeout(std::time::Duration::from_millis(150), socket.next()).await {
        Err(_) => {}
        Ok(frame) => panic!("expected no cross-namespace frame, got {frame:?}"),
    }
}

pub(crate) async fn wait_for_presence(socket: &mut Socket, want: u64) {
    for _ in 0..50 {
        socket
            .send(Message::Text(
                json!({ "event": "presence" }).to_string().into(),
            ))
            .await
            .expect("send presence");
        let frame = next_json(socket).await;
        assert_eq!(frame["event"], "presence");
        if frame["data"].as_u64().expect("presence count") == want {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("presence never reached {want}");
}

pub(crate) async fn connect_with_retry(url: &str, token: &str) -> Socket {
    for _ in 0..50 {
        let mut request = url.into_client_request().expect("valid websocket url");
        request.headers_mut().insert(
            header::AUTHORIZATION,
            format!("Bearer {token}")
                .parse()
                .expect("valid bearer header"),
        );
        match tokio_tungstenite::connect_async(request).await {
            Ok((socket, _)) => return socket,
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
    panic!("could not connect to {url}");
}

pub(crate) async fn next_json(socket: &mut Socket) -> Value {
    loop {
        match socket.next().await.expect("a frame").expect("a message") {
            Message::Text(text) => return serde_json::from_str(&text).expect("json envelope"),
            Message::Close(_) => panic!("socket closed before a reply"),
            _ => continue,
        }
    }
}
