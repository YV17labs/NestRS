use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use nest_rs::authn::JwtConfig;
use nest_rs::http::HttpTransport;
use nest_rs::http::poem::http::header;
use nest_rs::testing::{EphemeralDatabase, TestApp};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use uuid::Uuid;

use api::ApiModule;

use crate::{AUDIENCE, DEV_PUBLIC_KEY, ORG_ID, token_for, token_with_sub};

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

const BIND: &str = "127.0.0.1:13372";

const SUBSCRIBE: &str = "subscription { postPublished { id title orgId status } }";

async fn connect(url: &str, token: &str) -> Socket {
    for _ in 0..100 {
        let mut request = url.into_client_request().expect("valid websocket url");
        request.headers_mut().insert(
            header::AUTHORIZATION,
            format!("Bearer {token}")
                .parse()
                .expect("valid bearer header"),
        );
        request.headers_mut().insert(
            header::SEC_WEBSOCKET_PROTOCOL,
            "graphql-transport-ws".parse().expect("valid protocol"),
        );
        match tokio_tungstenite::connect_async(request).await {
            Ok((socket, _)) => return socket,
            Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
    panic!("could not connect to {url}");
}

async fn send(socket: &mut Socket, message: Value) {
    socket
        .send(Message::Text(message.to_string().into()))
        .await
        .expect("send a graphql-ws message");
}

async fn next_message(socket: &mut Socket, within: Duration) -> Option<Value> {
    loop {
        match tokio::time::timeout(within, socket.next()).await {
            Err(_) | Ok(None) => return None,
            Ok(Some(Ok(Message::Text(text)))) => {
                return Some(serde_json::from_str(&text).expect("a graphql-ws message is JSON"));
            }
            Ok(Some(Ok(Message::Close(_)))) => return None,
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(err))) => panic!("socket error: {err}"),
        }
    }
}

async fn subscribe(url: &str, token: &str, id: &str) -> Socket {
    let mut socket = connect(url, token).await;
    send(&mut socket, json!({ "type": "connection_init" })).await;
    let ack = next_message(&mut socket, Duration::from_secs(5))
        .await
        .expect("a connection_ack");
    assert_eq!(ack["type"], "connection_ack", "{ack}");
    send(
        &mut socket,
        json!({ "id": id, "type": "subscribe", "payload": { "query": SUBSCRIBE } }),
    )
    .await;
    socket
}

async fn next_item(socket: &mut Socket, within: Duration) -> Option<Value> {
    while let Some(message) = next_message(socket, within).await {
        match message["type"].as_str() {
            Some("next") => return Some(message["payload"].clone()),
            Some("complete") | Some("error") => return Some(message),
            _ => continue,
        }
    }
    None
}

async fn wait_ready() {
    for _ in 0..200 {
        if tokio::net::TcpStream::connect(BIND).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the api never bound {BIND}");
}

async fn post_json(path: &str, token: &str, body: Value) -> Value {
    let response = reqwest::Client::new()
        .post(format!("http://{BIND}{path}"))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .expect("the served api answers");
    assert!(
        response.status().is_success(),
        "POST {path} failed: {}",
        response.status(),
    );
    response.json().await.expect("a JSON body")
}

async fn graphql(token: &str, query: String) -> Value {
    let body = post_json("/graphql", token, json!({ "query": query })).await;
    assert!(
        body.get("errors").is_none(),
        "the operation succeeded: {body}",
    );
    body
}

#[tokio::test]
async fn a_subscriber_reads_only_the_published_posts_its_ability_allows() {
    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("create + migrate a throwaway database");
    let served = TestApp::builder()
        .module::<ApiModule>()
        .provide_arc(db.connection())
        .provide(JwtConfig {
            public_key: Some(DEV_PUBLIC_KEY.into()),
            audience: Some(AUDIENCE.into()),
            ..Default::default()
        })
        .build_headless()
        .await
        .expect("ApiModule boots against the throwaway database");
    served
        .init()
        .await
        .expect("the init phases register the event listeners");
    let transport = served
        .spawn_transport(HttpTransport::new().bind(BIND))
        .await
        .expect("the api serves on a real port");

    wait_ready().await;

    let bootstrap = token_for(ORG_ID, "admin").await;
    let org_a = post_json("/orgs", &bootstrap, json!({ "name": "SubAcme" })).await["id"]
        .as_str()
        .expect("an org id")
        .to_owned();
    let org_b = post_json("/orgs", &bootstrap, json!({ "name": "SubGlobex" })).await["id"]
        .as_str()
        .expect("an org id")
        .to_owned();

    let admin_a = token_for(&org_a, "admin").await;
    let author_id = post_json(
        "/users",
        &admin_a,
        json!({ "name": "Author", "email": "author@subacme.test" }),
    )
    .await["id"]
        .as_str()
        .and_then(|id| Uuid::parse_str(id).ok())
        .expect("a user id");
    let author = token_with_sub(&org_a, "admin", author_id).await;
    let stranger_token = token_for(&org_b, "admin").await;

    let post = post_json(
        "/posts",
        &author,
        json!({ "title": "Subscribed", "body": "Big news" }),
    )
    .await["id"]
        .as_str()
        .expect("a post id")
        .to_owned();

    let url = format!("ws://{BIND}/graphql");
    let mut reader = subscribe(&url, &author, "in").await;
    let mut stranger = subscribe(&url, &stranger_token, "out").await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    graphql(
        &author,
        format!("mutation {{ publishPost(id: \"{post}\") {{ id status }} }}"),
    )
    .await;

    let item = next_item(&mut reader, Duration::from_secs(5))
        .await
        .expect("the subscriber in the org receives the published post");
    assert_eq!(
        item["data"]["postPublished"]["title"], "Subscribed",
        "{item}",
    );
    assert_eq!(
        item["data"]["postPublished"]["status"], "PUBLISHED",
        "{item}",
    );

    let seen = next_item(&mut stranger, Duration::from_millis(700)).await;
    assert!(
        seen.is_none(),
        "a post from another org never reaches this subscriber: {seen:?}",
    );

    transport.shutdown().await.expect("the transport stops");
}
