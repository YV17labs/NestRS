//! A **destructured** handler argument works on all four transports.
//!
//! `async fn greet(&self, Path(name): Path<String>)` is poem's own idiom, and
//! the shape a reader writes first — twelve snippets across five docs pages show
//! it. It used to be a compile error: `#[routes]` and `#[resolver]` forward each
//! argument to the generated wrapper *by name*, and a pattern has none, so the
//! macro rejected it up front. It now forwards under the identifier the pattern
//! binds and leaves the developer's method alone.
//!
//! Pinned here, against one app, rather than per transport: HTTP and GraphQL are
//! the two that had to change, and WS and queue forward positionally so they
//! already accepted patterns — which is precisely the kind of asymmetry that
//! rots unnoticed. All four assert on a **value that travelled through the
//! destructured binding**, so a wrapper forwarding the wrong thing fails here
//! rather than merely compiling.

use std::sync::{Arc, Mutex};

use nest_rs_core::{Container, injectable, module};
use nest_rs_graphql::async_graphql::{InputObject, Result as GqlResult};
use nest_rs_graphql::{GraphqlModule, resolver};
// Two `Valid` carriers by design (the orphan rule): the HTTP one wraps a poem
// extractor, the value-form one wraps the wire value. Both are exercised here.
use nest_rs_http::{HttpModule, Valid as HttpValid, controller, routes};
use nest_rs_pipes::{Pipe, PipeError, Piped, Valid};
use nest_rs_queue::{ProcessMethod, processor, queue};
use nest_rs_testing::TestApp;
use nest_rs_ws::{Gateway, WsClient, WsModule, WsReply, gateway, messages};
use poem::http::StatusCode;
use poem::web::{Json, Path};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use validator::Validate;

// --- the payload every transport carries -------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, Validate, JsonSchema, InputObject)]
struct Note {
    #[validate(length(min = 1))]
    text: String,
}

/// A pipe with an observable effect, so a `Piped<Trim, T>` argument proves the
/// pipe still ran alongside a destructured one.
struct Trim;

impl Pipe for Trim {
    type In = String;
    type Out = String;
    fn transform(input: String) -> Result<String, PipeError> {
        Ok(input.trim().to_owned())
    }
}

// --- HTTP: `#[routes]` ------------------------------------------------------

#[controller(path = "/notes")]
struct NotesController;

#[routes]
impl NotesController {
    // The exact snippet from `/http/controllers/`.
    #[get("/greet/:name")]
    #[public]
    async fn greet(&self, Path(name): Path<String>) -> String {
        format!("Hello, {name}!")
    }

    // The pipe carrier: `Valid<Json<Note>>` holds the extractor's *inner*
    // value, so the pattern binds the `Note` itself.
    #[post("/")]
    #[public]
    async fn create(&self, HttpValid(note): HttpValid<Json<Note>>) -> String {
        note.text.clone()
    }
}

// --- GraphQL: `#[resolver]` --------------------------------------------------

#[resolver]
struct NotesResolver;

#[resolver]
impl NotesResolver {
    // `Valid<T>` destructures, and the SDL argument keeps the name the pattern
    // binds. `Piped<P, T>` carries a phantom marker for `P`, so it is not a
    // tuple struct and binds under a plain name — mixed in here so one operation
    // proves both shapes coexist.
    #[query]
    #[public]
    async fn shout(&self, Valid(note): Valid<Note>, pad: Piped<Trim, String>) -> GqlResult<String> {
        Ok(format!("{}{}", note.text.to_uppercase(), pad.len()))
    }
}

// --- WebSockets: `#[messages]` -----------------------------------------------

#[gateway(path = "/notes-ws")]
struct NotesGateway;

#[messages]
impl NotesGateway {
    #[subscribe_message("note.echo")]
    async fn echo(&self, Valid(note): Valid<Note>) -> String {
        note.text.clone()
    }
}

// --- Queue: `#[processor]` ---------------------------------------------------

/// Where the job handler records what it received, so the assertion can run
/// outside the container.
static SEEN: Mutex<Option<String>> = Mutex::new(None);

#[queue(name = "destructured-notes", job = Note)]
struct NotesQueue;

#[injectable]
#[derive(Default)]
struct NotesProcessor;

#[processor]
impl NotesProcessor {
    #[process(queue = NotesQueue)]
    async fn record(&self, Valid(note): Valid<Note>) -> anyhow::Result<()> {
        *SEEN.lock().expect("lock") = Some(note.text.clone());
        Ok(())
    }
}

// --- the app ----------------------------------------------------------------

#[module(
    imports = [HttpModule::for_root(None), GraphqlModule::for_root(None), WsModule],
    providers = [NotesController, NotesResolver, NotesGateway, NotesProcessor],
)]
struct DestructuredModule;

async fn app() -> TestApp {
    TestApp::builder()
        .module::<DestructuredModule>()
        .build()
        .await
        .expect("an app whose handlers destructure their arguments boots")
}

// --- HTTP -------------------------------------------------------------------

#[tokio::test]
async fn http_forwards_a_destructured_path_extractor() {
    let res = app().await.http().get("/notes/greet/ada").send().await;
    res.assert_status_is_ok();
    res.assert_text("Hello, ada!").await;
}

#[tokio::test]
async fn http_forwards_a_nested_destructured_extractor_and_still_validates() {
    let app = app().await;

    let ok = app
        .http()
        .post("/notes")
        .body_json(&serde_json::json!({ "text": "kept" }))
        .send()
        .await;
    ok.assert_status_is_ok();
    ok.assert_text("kept").await;

    // The pattern must not have swallowed the `Valid` layer: an empty `text`
    // still trips `#[validate(length(min = 1))]` at the edge.
    let rejected = app
        .http()
        .post("/notes")
        .body_json(&serde_json::json!({ "text": "" }))
        .send()
        .await;
    rejected.assert_status(StatusCode::BAD_REQUEST);
}

// --- GraphQL ----------------------------------------------------------------

#[tokio::test]
async fn graphql_forwards_a_destructured_piped_argument() {
    let res = app()
        .await
        .http()
        .post("/graphql")
        .body_json(
            &serde_json::json!({ "query": r#"{ shout(note: { text: "hi" }, pad: "  x  ") }"# }),
        )
        .send()
        .await;
    res.assert_status_is_ok();
    let body = res.0.into_body().into_string().await.expect("body");
    assert!(
        body.contains("HI1"),
        "the destructured `Valid` carried the note, and the plain-bound `Piped` \
         still trimmed its own argument to one char: {body}",
    );
}

/// The SDL argument name comes from the wrapper's parameter, so a synthesized
/// one would silently rewrite the public schema. The query above had to spell
/// `note` — the identifier the pattern binds — and a different name is refused
/// by the schema, which is what pins it. (Introspection is off by default, so
/// this asks the schema rather than reading it.)
#[tokio::test]
async fn graphql_names_the_argument_after_the_pattern_binding() {
    let res = app()
        .await
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({
            "query": r#"{ shout(__nestrs_arg0: { text: "hi" }, pad: "x") }"#
        }))
        .send()
        .await;
    let body = res.0.into_body().into_string().await.expect("body");
    assert!(
        body.contains("Unknown argument") || body.contains("__nestrs_arg0"),
        "a generated parameter name must not be what the schema exposes: {body}",
    );
    assert!(
        !body.contains("\"data\":{\"shout\""),
        "and the operation must not have succeeded under that name: {body}",
    );
}

// --- WebSockets -------------------------------------------------------------

#[tokio::test]
async fn ws_dispatches_to_a_destructured_payload_argument() {
    let reply = NotesGateway
        .dispatch(
            &WsClient::for_test(),
            "note.echo",
            serde_json::json!({ "text": "framed" }),
        )
        .await;
    match reply {
        WsReply::Reply(v) => assert_eq!(v.as_str(), Some("framed")),
        WsReply::Error(msg) => panic!("expected a reply, got error: {msg}"),
        WsReply::None => panic!("expected a reply, got none"),
    }
}

// --- Queue ------------------------------------------------------------------

#[tokio::test]
async fn a_process_method_dispatches_to_a_destructured_job_argument() {
    let method = nest_rs_core::inventory::iter::<ProcessMethod>()
        .find(|m| m.name == "NotesProcessor::record")
        .expect("the #[process] method is discovered");

    let container = Container::builder().provide(NotesProcessor).build();
    (method.handler)(
        serde_json::json!({ "v": nest_rs_queue::WIRE_FORMAT_VERSION, "payload": { "text": "queued" } }),
        container,
    )
    .await
    .expect("the job runs");

    assert_eq!(
        SEEN.lock().expect("lock").as_deref(),
        Some("queued"),
        "the destructured `Valid(note)` job argument reached the body",
    );
}

/// Keeps `Arc` in use on the same import line the other tests rely on, and
/// documents that the pattern rewrite is wrapper-only: the developer's method
/// still sees the whole carrier, so it can be called directly.
#[tokio::test]
async fn the_developers_method_keeps_its_pattern() {
    let ctrl = Arc::new(NotesController);
    assert_eq!(
        ctrl.greet(Path("direct".to_owned())).await,
        "Hello, direct!"
    );
}
