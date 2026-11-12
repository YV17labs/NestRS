//! `Opaque` tells the operator the whole failure: the sentence a wrapper prints
//! and every cause beneath it, whichever error type the handler returned, filed
//! once the answer is final and naming the route it failed, as every chain the
//! edge withholds is.

use anyhow::Context;
use nest_rs_core::{Caught, ExceptionFilter, Layer, Problem, UnitContext, async_trait, code};
use nest_rs_core::{injectable, module};
use nest_rs_http::{Method, Opaque, Path, Response, Result, StatusCode, controller, routes};
use nest_rs_testing::LogCapture;

#[test]
fn the_operator_line_carries_every_cause_of_an_anyhow_chain() {
    let capture = LogCapture::install();
    let failed: anyhow::Result<()> = Err(anyhow::anyhow!("connection refused by 10.0.0.5:6379"))
        .context("the queue backend failed");

    let refused = failed.opaque().expect_err("a failure");
    capture.expect_none(nest_rs_http::target::HTTP, "request failed");
    drop(refused.into_response());

    let line = capture.expect_one(nest_rs_http::target::HTTP, "request failed");
    let error = line.field("error").unwrap_or_default();
    assert!(error.contains("the queue backend failed"), "{error}");
    assert!(
        error.contains("connection refused by 10.0.0.5:6379"),
        "{error}"
    );
}

/// What the ledger answers every route below, its words the operator's alone.
const LEDGER_REFUSED: &str = "the ledger at the eu-west warehouse refused";

fn settled() -> std::result::Result<u64, std::io::Error> {
    Err(std::io::Error::other(LEDGER_REFUSED))
}

/// Answers any failure as the caller's `409`.
#[injectable]
#[derive(Default, Layer)]
struct AsConflict;

#[async_trait]
impl ExceptionFilter for AsConflict {
    type Exception = dyn std::error::Error + Send + Sync;

    async fn catch(&self, _: &Self::Exception, _: &mut UnitContext<'_>) -> Caught {
        Caught::Problem(Problem::new(409, code::CONFLICT))
    }
}

/// Answers any failure as a `503`, the server unable to serve it in time.
#[injectable]
#[derive(Default, Layer)]
struct AsUnavailable;

#[async_trait]
impl ExceptionFilter for AsUnavailable {
    type Exception = dyn std::error::Error + Send + Sync;

    async fn catch(&self, _: &Self::Exception, _: &mut UnitContext<'_>) -> Caught {
        Caught::Problem(Problem::new(503, code::UNAVAILABLE))
    }
}

/// Claims a failure no route below raises, so the route runs layered and its
/// failure is answered by the edge's one rule.
#[injectable]
#[derive(Default, Layer)]
struct NoneOfTheLedgers;

#[async_trait]
impl ExceptionFilter for NoneOfTheLedgers {
    type Exception = std::fmt::Error;

    async fn catch(&self, _: &std::fmt::Error, _: &mut UnitContext<'_>) -> Caught {
        Caught::Problem(Problem::new(409, code::CONFLICT))
    }
}

#[controller(path = "/ledger")]
#[derive(Default)]
struct LedgerController;

#[routes]
impl LedgerController {
    #[get("/failed")]
    #[public]
    async fn failed(&self) -> anyhow::Result<String> {
        Err(anyhow::anyhow!(LEDGER_REFUSED))
    }

    #[get("/opaqued/{id}")]
    #[public]
    async fn opaqued(&self, Path(id): Path<u64>) -> Result<String> {
        Ok(format!("settled {id}: {}", settled().opaque()?))
    }

    #[post("/layered")]
    #[public]
    #[use_exception_filters(NoneOfTheLedgers)]
    async fn layered(&self) -> Result<String> {
        Ok(format!("settled {}", settled().opaque()?))
    }

    #[get("/conflicted")]
    #[public]
    #[use_exception_filters(AsConflict)]
    async fn conflicted(&self) -> Result<String> {
        Ok(format!("settled {}", settled().opaque()?))
    }

    #[get("/unavailable")]
    #[public]
    #[use_exception_filters(AsUnavailable)]
    async fn unavailable(&self) -> Result<String> {
        Ok(format!("settled {}", settled().opaque()?))
    }

    #[get("/carried")]
    #[public]
    async fn carried(&self) -> anyhow::Result<String> {
        Ok(format!("settled {}", settled().opaque()?))
    }

    #[get("/charged")]
    #[public]
    async fn charged(&self) -> anyhow::Result<String> {
        let settled = settled().opaque().context("charging the card")?;
        Ok(format!("settled {settled}"))
    }

    #[post("/charged")]
    #[public]
    #[use_exception_filters(NoneOfTheLedgers)]
    async fn charged_layered(&self) -> anyhow::Result<String> {
        let settled = settled().opaque().context("charging the card")?;
        Ok(format!("settled {settled}"))
    }
}

#[module(providers = [AsConflict, AsUnavailable, NoneOfTheLedgers, LedgerController])]
struct LedgerModule;

/// A handler's `Err` and `.opaque()`'s chain are filed once the answer is final,
/// naming the route as its `http.request` line does, by `verb` and `path`: on
/// a route running no layer, whose error the edge renders, and on one running
/// its layers, whose site renders it.
#[tokio::test]
async fn a_withheld_chain_is_filed_naming_its_route() {
    for (verb, sent, route) in [
        ("GET", "/ledger/failed", "/ledger/failed"),
        ("GET", "/ledger/opaqued/7", "/ledger/opaqued/{id}"),
        ("POST", "/ledger/layered", "/ledger/layered"),
    ] {
        let client = crate::boot::<LedgerModule>().await;
        let logs = LogCapture::install();
        let method = Method::from_bytes(verb.as_bytes()).expect("a method");
        let resp = client.request(method, sent).send().await;
        resp.assert_status(StatusCode::INTERNAL_SERVER_ERROR);

        let line = logs.expect_one(nest_rs_http::target::HTTP, "request failed");
        assert_eq!(line.level, "error", "{route}");
        assert_eq!(
            line.field("error").as_deref(),
            Some(LEDGER_REFUSED),
            "{route}"
        );
        assert_eq!(
            line.field("verb").as_deref(),
            Some(verb),
            "{route}: {:?}",
            line.fields
        );
        assert_eq!(
            line.field("path").as_deref(),
            Some(route),
            "{route}: {:?}",
            line.fields
        );
    }
}

/// An `anyhow` chain a `?` built over `.opaque()`'s error, bare or under a
/// context, files the chain `.opaque()` kept, never the opaque `500`'s own
/// words, naming the route: on a route running no layer and one running its
/// layers.
#[tokio::test]
async fn an_anyhow_chain_over_opaque_files_the_chain_it_kept() {
    for (verb, route) in [
        ("GET", "/ledger/carried"),
        ("GET", "/ledger/charged"),
        ("POST", "/ledger/charged"),
    ] {
        let client = crate::boot::<LedgerModule>().await;
        let logs = LogCapture::install();
        let method = Method::from_bytes(verb.as_bytes()).expect("a method");
        let resp = client.request(method, route).send().await;
        resp.assert_status(StatusCode::INTERNAL_SERVER_ERROR);

        let line = logs.expect_one(nest_rs_http::target::HTTP, "request failed");
        assert_eq!(
            line.field("error").as_deref(),
            Some(LEDGER_REFUSED),
            "{verb} {route}: {:?}",
            line.fields
        );
        assert_eq!(line.field("verb").as_deref(), Some(verb), "{route}");
        assert_eq!(line.field("path").as_deref(), Some(route), "{verb}");
    }
}

/// A filter answering `.opaque()`'s failure as the caller's `4xx` withholds
/// nothing, so nothing is filed.
#[tokio::test]
async fn a_filter_answering_4xx_over_opaque_files_nothing() {
    let client = crate::boot::<LedgerModule>().await;
    let logs = LogCapture::install();
    client
        .get("/ledger/conflicted")
        .send()
        .await
        .assert_status(StatusCode::CONFLICT);
    logs.expect_none(nest_rs_http::target::HTTP, "request failed");
    logs.expect_none(nest_rs_http::target::HTTP, "request unavailable");
}

/// A filter answering `.opaque()`'s failure `503` withholds the chain it kept,
/// filed at the `503`'s level, `warn`, naming the route.
#[tokio::test]
async fn a_filter_answering_503_over_opaque_files_its_chain_at_warn() {
    let client = crate::boot::<LedgerModule>().await;
    let logs = LogCapture::install();
    client
        .get("/ledger/unavailable")
        .send()
        .await
        .assert_status(StatusCode::SERVICE_UNAVAILABLE);
    logs.expect_none(nest_rs_http::target::HTTP, "request failed");
    let line = logs.expect_one(nest_rs_http::target::HTTP, "request unavailable");
    assert_eq!(line.level, "warn");
    assert_eq!(line.field("error").as_deref(), Some(LEDGER_REFUSED));
    assert_eq!(line.field("verb").as_deref(), Some("GET"));
    assert_eq!(line.field("path").as_deref(), Some("/ledger/unavailable"));
}

/// A route registered by hand, whose endpoint takes the request whole, files
/// its chain naming the route the router matched.
#[tokio::test]
async fn a_hand_written_routes_chain_is_filed_naming_its_route() {
    let client = crate::vocabulary_route(Method::DELETE, "/drafts/{id}", |_, _| async {
        let settled = settled().opaque()?;
        Ok(Response::builder().body(settled.to_string()))
    })
    .await;
    let logs = LogCapture::install();
    client
        .delete("/drafts/7")
        .send()
        .await
        .assert_status(StatusCode::INTERNAL_SERVER_ERROR);
    let line = logs.expect_one(nest_rs_http::target::HTTP, "request failed");
    assert_eq!(line.field("error").as_deref(), Some(LEDGER_REFUSED));
    assert_eq!(line.field("verb").as_deref(), Some("DELETE"));
    assert_eq!(line.field("path").as_deref(), Some("/drafts/{id}"));
}
