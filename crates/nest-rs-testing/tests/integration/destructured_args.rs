//! A **destructured** handler argument (`Path(name): Path<String>`) works on
//! all four transports, each asserting a value that travelled through the
//! binding.

use std::sync::{Arc, Mutex};

use nest_rs_core::{Container, injectable, module};
use nest_rs_graphql::async_graphql::{InputObject, Result as GqlResult};
use nest_rs_graphql::{GraphqlModule, operations, resolver};
// Two `Valid` carriers (the orphan rule): the HTTP one wraps a poem extractor.
use nest_rs_http::{HttpModule, Valid as HttpValid, controller, routes};
use nest_rs_pipes::{Pipe, PipeError, Piped, Valid};
use nest_rs_queue::__private::consume::{self, AttemptOutcome, Delivery};
use nest_rs_queue::{Capabilities, ProcessMethod, QueueBackend, QueueName, processor, queue};
use nest_rs_testing::TestApp;
use nest_rs_ws::{Gateway, WsClient, WsModule, WsReply, gateway, messages};
use poem::http::StatusCode;
use poem::web::{Json, Path};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use validator::Validate;

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

#[controller(path = "/notes")]
struct NotesController;

#[routes]
impl NotesController {
    #[get("/greet/:name")]
    #[public]
    async fn greet(&self, Path(name): Path<String>) -> String {
        format!("Hello, {name}!")
    }

    #[post("/")]
    #[public]
    async fn create(&self, HttpValid(note): HttpValid<Json<Note>>) -> String {
        note.text.clone()
    }
}

#[resolver]
struct NotesResolver;

#[operations]
impl NotesResolver {
    // `Piped<P, T>` carries a phantom marker, so it binds under a plain name.
    #[query]
    #[public]
    async fn shout(&self, Valid(note): Valid<Note>, pad: Piped<Trim, String>) -> GqlResult<String> {
        Ok(format!("{}{}", note.text.to_uppercase(), pad.len()))
    }
}

#[gateway(path = "/notes-ws")]
struct NotesGateway;

#[messages]
impl NotesGateway {
    #[subscribe_message("note.echo")]
    #[public]
    async fn echo(&self, Valid(note): Valid<Note>) -> String {
        note.text.clone()
    }
}

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

    let rejected = app
        .http()
        .post("/notes")
        .body_json(&serde_json::json!({ "text": "" }))
        .send()
        .await;
    rejected.assert_status(StatusCode::BAD_REQUEST);
}

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

/// Introspection is off by default, so the schema is asked rather than read.
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

/// The attempt runs in process, so the backend it names only labels the span.
static IN_PROCESS: QueueBackend = QueueBackend::new("in-process", Capabilities::NONE);

#[tokio::test]
async fn a_process_method_dispatches_to_a_destructured_job_argument() {
    let method = nest_rs_core::inventory::iter::<ProcessMethod>()
        .find(|m| m.name() == "NotesProcessor::record")
        .expect("the #[process] method is discovered");

    let container = Container::builder().provide(NotesProcessor).build();
    let mut delivery = Delivery::new(
        &IN_PROCESS,
        QueueName::new(method.queue()).expect("the decorator checked the name"),
        serde_json::json!({ "v": nest_rs_queue::WIRE_FORMAT_VERSION, "payload": { "text": "queued" } }),
    );
    let outcome = consume::attempt(method, &mut delivery, container).await;
    assert!(
        matches!(outcome, AttemptOutcome::Ok),
        "the job runs: {outcome:?}"
    );

    assert_eq!(
        SEEN.lock().expect("lock").as_deref(),
        Some("queued"),
        "the destructured `Valid(note)` job argument reached the body",
    );
}

#[tokio::test]
async fn the_developers_method_keeps_its_pattern() {
    let ctrl = Arc::new(NotesController);
    assert_eq!(
        ctrl.greet(Path("direct".to_owned())).await,
        "Hello, direct!"
    );
}
