use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use nest_rs::http::header;
use nest_rs::testing::TestApp;
use nest_rs::testing::ws::WsApp;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use uuid::Uuid;

use api::ApiModule;
use features::{Role, Subject};

use crate::{ORG_ID, claims, client};

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

const SUBSCRIBE: &str = "subscription { postPublished { id title orgId status } }";

async fn connect(app: &WsApp, authorization: Option<&str>) -> Socket {
    let mut request = app
        .url("/graphql")
        .into_client_request()
        .expect("valid websocket url");
    if let Some(authorization) = authorization {
        request.headers_mut().insert(
            header::AUTHORIZATION,
            authorization.parse().expect("valid bearer header"),
        );
    }
    request.headers_mut().insert(
        header::SEC_WEBSOCKET_PROTOCOL,
        "graphql-transport-ws".parse().expect("valid protocol"),
    );
    tokio_tungstenite::connect_async(request)
        .await
        .expect("the served api upgrades")
        .0
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

async fn subscribe(app: &WsApp, authorization: Option<&str>, id: &str) -> Socket {
    let mut socket = connect(app, authorization).await;
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

async fn post_json(app: &WsApp, path: &str, authorization: &str, body: Value) -> Value {
    let response = reqwest::Client::new()
        .post(format!("http://{}{path}", app.addr()))
        .header(header::AUTHORIZATION, authorization)
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

async fn graphql(app: &WsApp, authorization: &str, query: String) -> Value {
    let body = post_json(app, "/graphql", authorization, json!({ "query": query })).await;
    assert!(
        body.get("errors").is_none(),
        "the operation succeeded: {body}",
    );
    body
}

fn id_of(created: Value) -> Uuid {
    created["id"]
        .as_str()
        .and_then(|id| id.parse().ok())
        .expect("the id of the row it created")
}

async fn serve() -> WsApp {
    TestApp::builder()
        .module::<ApiModule>()
        .ephemeral_database::<migrations::Migrator>()
        .ephemeral_signing_key()
        .build_ws()
        .await
        .expect("ApiModule serves on a real port against a throwaway database")
}

#[tokio::test]
async fn an_anonymous_subscriber_is_refused_at_subscribe() {
    let app = serve().await;

    let mut socket = subscribe(&app, None, "anonymous").await;

    let refused = next_item(&mut socket, Duration::from_secs(5))
        .await
        .expect("the refusal is delivered");
    assert_eq!(
        refused["errors"][0]["extensions"]["code"], "UNAUTHENTICATED",
        "{refused}",
    );

    app.close().await;
}

#[tokio::test]
async fn a_subscriber_reads_only_the_published_posts_its_ability_allows() {
    let app = serve().await;
    let sign = |org_id: Uuid, sub: Subject| app.bearer(&claims(org_id, Role::Admin, sub));

    let bootstrap = sign(ORG_ID, client());
    let org_a = id_of(post_json(&app, "/orgs", &bootstrap, json!({ "name": "SubAcme" })).await);
    let org_b = id_of(post_json(&app, "/orgs", &bootstrap, json!({ "name": "SubGlobex" })).await);

    let admin_a = sign(org_a, client());
    let author_id = id_of(
        post_json(
            &app,
            "/users",
            &admin_a,
            json!({ "name": "Author", "email": "author@subacme.test" }),
        )
        .await,
    );
    let author = sign(org_a, Subject::User(author_id));
    let stranger = sign(org_b, client());

    let post = id_of(
        post_json(
            &app,
            "/posts",
            &author,
            json!({ "title": "Subscribed", "body": "Big news" }),
        )
        .await,
    );

    let mut reader = subscribe(&app, Some(&author), "in").await;
    let mut outsider = subscribe(&app, Some(&stranger), "out").await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    graphql(
        &app,
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

    let seen = next_item(&mut outsider, Duration::from_millis(700)).await;
    assert!(
        seen.is_none(),
        "a post from another org never reaches this subscriber: {seen:?}",
    );

    app.close().await;
}
