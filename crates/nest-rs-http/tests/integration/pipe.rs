//! A `Piped<P, E>` refusal is the edge's `400` problem, and its `detail` says
//! what the value had to be, never the value. Every extracted argument whose
//! type declares rules is validated there, whatever extractor carries it, a
//! pipe's source before the pipe runs, once the guards admitted the caller.

use std::sync::atomic::{AtomicU32, Ordering};

use nest_rs_core::{input, module};
use nest_rs_http::StatusCode;
use nest_rs_http::{Form, Json, Path, Query, controller, routes};
use nest_rs_pipes::{ParseArray, Pipe, PipeError, Piped};

use crate::Refuse;

#[input]
struct Charge {
    #[validate(range(min = 1))]
    amount: u64,
}

/// Counts the bodies a handler reached, so a refusal is seen to run before it.
static CHARGED: AtomicU32 = AtomicU32::new(0);

/// Counts the charges the pipe read, so a refusal is seen to run before it.
static DOUBLED: AtomicU32 = AtomicU32::new(0);

/// A pipe over a body whose type declares rules.
struct Doubled;

impl Pipe for Doubled {
    type In = Charge;
    type Out = u64;
    fn transform(charge: Charge) -> Result<u64, PipeError> {
        DOUBLED.fetch_add(1, Ordering::SeqCst);
        Ok(charge.amount * 2)
    }
}

#[controller(path = "/pipe")]
struct PipeController;

#[routes]
impl PipeController {
    #[get("/ids/{ids}")]
    #[public]
    async fn ids(&self, ids: Piped<ParseArray<u64>, Path<String>>) -> String {
        format!("{:?}", ids.into_inner())
    }

    #[post("/charge")]
    #[public]
    async fn charge(&self, Json(charge): Json<Charge>) -> String {
        CHARGED.fetch_add(1, Ordering::SeqCst);
        charge.amount.to_string()
    }

    #[get("/charge")]
    #[public]
    async fn charge_by_query(&self, charge: Query<Charge>) -> String {
        charge.amount.to_string()
    }

    #[post("/charge/form")]
    #[public]
    async fn charge_by_form(&self, charge: Form<Charge>) -> String {
        charge.amount.to_string()
    }

    #[post("/charge/doubled")]
    #[public]
    async fn charge_doubled(&self, Piped(doubled): Piped<Doubled, Json<Charge>>) -> String {
        CHARGED.fetch_add(1, Ordering::SeqCst);
        doubled.to_string()
    }
}

#[module(providers = [PipeController])]
struct PipeModule;

#[tokio::test]
async fn a_refused_list_item_is_never_quoted_in_the_problem() {
    let client = crate::boot::<PipeModule>().await;

    let resp = client
        .get("/pipe/ids/1,sk_live_51HsecretTOKEN,3")
        .send()
        .await;

    resp.assert_status(StatusCode::BAD_REQUEST);
    resp.assert_content_type("application/problem+json");
    let body = resp
        .0
        .into_body()
        .into_string()
        .await
        .expect("a readable body");
    assert!(
        !body.contains("sk_live"),
        "the problem quotes the item: {body}"
    );
    let problem: serde_json::Value = serde_json::from_str(&body).expect("a problem document");
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("u64")),
        "the refusal names what an item must be: {problem}",
    );
}

/// The `400` an argument breaking its type's rules answers: the field named,
/// the submitted value never.
async fn refused_for_its_rules(resp: nest_rs_testing::TestResponse) {
    resp.assert_status(StatusCode::BAD_REQUEST);
    resp.assert_content_type("application/problem+json");
    let body = resp
        .0
        .into_body()
        .into_string()
        .await
        .expect("a readable body");
    let problem: serde_json::Value = serde_json::from_str(&body).expect("a problem document");
    assert_eq!(problem["detail"], "validation failed", "{problem}");
    assert_eq!(problem["errors"]["amount"][0]["code"], "range", "{problem}");
}

#[tokio::test]
async fn a_bare_body_whose_type_declares_rules_is_refused_before_the_handler() {
    let client = crate::boot::<PipeModule>().await;
    let before = CHARGED.load(Ordering::SeqCst);

    refused_for_its_rules(
        client
            .post("/pipe/charge")
            .content_type("application/json")
            .body(br#"{"amount":0}"#.to_vec())
            .send()
            .await,
    )
    .await;

    assert_eq!(CHARGED.load(Ordering::SeqCst), before, "the handler ran");
    client
        .post("/pipe/charge")
        .content_type("application/json")
        .body(br#"{"amount":5}"#.to_vec())
        .send()
        .await
        .assert_text("5")
        .await;
}

#[tokio::test]
async fn every_extractor_carrying_a_type_with_rules_is_validated() {
    let client = crate::boot::<PipeModule>().await;

    refused_for_its_rules(client.get("/pipe/charge?amount=0").send().await).await;
    refused_for_its_rules(
        client
            .post("/pipe/charge/form")
            .content_type("application/x-www-form-urlencoded")
            .body(b"amount=0".to_vec())
            .send()
            .await,
    )
    .await;
    client
        .get("/pipe/charge?amount=3")
        .send()
        .await
        .assert_text("3")
        .await;
}

#[tokio::test]
async fn a_piped_source_breaking_its_rules_is_refused_before_the_pipe_and_the_handler() {
    let client = crate::boot::<PipeModule>().await;
    let (piped, charged) = (
        DOUBLED.load(Ordering::SeqCst),
        CHARGED.load(Ordering::SeqCst),
    );

    refused_for_its_rules(
        client
            .post("/pipe/charge/doubled")
            .content_type("application/json")
            .body(br#"{"amount":0}"#.to_vec())
            .send()
            .await,
    )
    .await;

    assert_eq!(DOUBLED.load(Ordering::SeqCst), piped, "the pipe ran");
    assert_eq!(CHARGED.load(Ordering::SeqCst), charged, "the handler ran");
    client
        .post("/pipe/charge/doubled")
        .content_type("application/json")
        .body(br#"{"amount":4}"#.to_vec())
        .send()
        .await
        .assert_text("8")
        .await;
}

#[controller(path = "/gated")]
#[use_guards(Refuse)]
struct GatedController;

#[routes]
impl GatedController {
    #[post("/charge")]
    async fn charge(&self, charge: Json<Charge>) -> String {
        charge.amount.to_string()
    }
}

#[module(providers = [Refuse, GatedController])]
struct GatedModule;

#[tokio::test]
async fn a_refused_caller_is_answered_before_its_input_is_validated() {
    let client = crate::boot::<GatedModule>().await;

    client
        .post("/gated/charge")
        .content_type("application/json")
        .body(br#"{"amount":0}"#.to_vec())
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

/// The same route written by hand on the vocabulary's extractors: what
/// `#[routes]` answers, case for case. `Piped` is no extractor, so the route
/// extracts the source, then runs the pipe over its value.
async fn on_the_vocabulary() -> crate::Client {
    crate::vocabulary_route(
        nest_rs_http::Method::GET,
        "/pipe/ids/{ids}",
        |req, mut body| async move {
            let raw =
                <Path<String> as nest_rs_http::FromRequest>::from_request(&req, &mut body).await?;
            let ids = Piped::<ParseArray<u64>, Path<String>>::apply(raw.into_inner())
                .map_err(|refused| nest_rs_http::pipe_error(&refused))?;
            Ok(nest_rs_http::IntoResponse::into_response(format!(
                "{:?}",
                ids.into_inner()
            )))
        },
    )
    .await
}

#[tokio::test]
async fn the_vocabulary_answers_each_case_as_6x_did() {
    let six = crate::boot::<PipeModule>().await;
    let seven = on_the_vocabulary().await;
    for path in [
        "/pipe/ids/1,2,3",
        "/pipe/ids/1,sk_live_51HsecretTOKEN,3",
        "/pipe/ids/%2C%2C",
    ] {
        assert_eq!(
            crate::answered(seven.get(path).send().await).await,
            crate::answered(six.get(path).send().await).await,
            "{path}",
        );
    }
}
