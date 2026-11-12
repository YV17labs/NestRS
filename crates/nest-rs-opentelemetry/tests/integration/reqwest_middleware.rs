//! Covers `src/reqwest_middleware.rs`, against the TLS collector double: a
//! request an app's own client sends inside a unit carries a span of its own in
//! the unit's trace, one sent outside every unit carries none, and each is
//! recorded as a client operation by its method, its server and its status —
//! never its path or its query.

use std::sync::Mutex;

use nest_rs_core::Correlation;
use nest_rs_core::client_operation::{ClientOperation, ClientValue};
use nest_rs_opentelemetry::ReqwestMiddleware;
use reqwest_middleware::ClientWithMiddleware;

use crate::otlp::{AUTHORITY, Behaviour, Collector};

/// What the seeded recorder was handed for each request: its span's name, its
/// attributes as written, its class if it failed, and the span it announced.
#[derive(Clone, Debug)]
struct Recorded {
    name: String,
    attributes: Vec<(String, String)>,
    error_type: Option<String>,
    announced: Option<String>,
}

static RECORDED: Mutex<Vec<Recorded>> = Mutex::new(Vec::new());

fn capture(operation: &ClientOperation<'_>) {
    let attributes = operation
        .attributes()
        .iter()
        .map(|(key, value)| {
            let value = match value {
                ClientValue::Text(text) => (*text).to_owned(),
                ClientValue::Integer(integer) => integer.to_string(),
                _ => unreachable!("the middleware writes text and integers"),
            };
            ((*key).to_owned(), value)
        })
        .collect();
    RECORDED.lock().expect("the recorded").push(Recorded {
        name: operation.name().to_owned(),
        attributes,
        error_type: operation.error_type().map(str::to_owned),
        announced: operation
            .announced()
            .map(|announced| announced.span_id().to_hex()),
    });
}

/// A client trusting the double's authority, through the middleware.
fn client() -> ClientWithMiddleware {
    let authority =
        reqwest::Certificate::from_pem(AUTHORITY.pem().as_bytes()).expect("the authority");
    let client = reqwest::Client::builder()
        .tls_certs_only([authority])
        .build()
        .expect("a client");
    reqwest_middleware::ClientBuilder::new(client)
        .with(ReqwestMiddleware::new())
        .build()
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime")
}

#[test]
fn a_request_inside_a_unit_carries_a_span_of_its_own_in_the_units_trace() {
    let collector = Collector::serving();
    let unit = Correlation::minted(None);
    let url = format!(
        "{}/hooks/secret-in-the-path?token=secret-in-the-query",
        collector.endpoint()
    );

    runtime().block_on(nest_rs_core::with_request_scope(
        None,
        unit.clone(),
        async {
            client().get(&url).send().await.expect("answered");
        },
    ));

    let received = collector.received();
    let traceparent = received[0].header("traceparent").expect("a traceparent");
    let [version, trace_id, parent_id, _flags] = traceparent
        .split('-')
        .collect::<Vec<_>>()
        .try_into()
        .expect("four fields");
    assert_eq!(version, "00");
    assert_eq!(trace_id, unit.trace_id().to_hex(), "the unit's trace");
    assert_ne!(
        parent_id,
        unit.span_id().to_hex(),
        "a span of the request's own, under the unit's"
    );
    assert_eq!(received[0].header("baggage"), None);
}

#[test]
fn a_request_outside_every_unit_carries_no_trace_context() {
    let collector = Collector::serving();

    runtime().block_on(async {
        client()
            .get(collector.endpoint())
            .send()
            .await
            .expect("answered");
    });

    assert_eq!(collector.received()[0].header("traceparent"), None);
}

#[test]
fn a_request_is_recorded_by_its_method_server_and_status_never_its_path_or_query() {
    nest_rs_core::__private::set_client_recorder(capture);
    let collector = Collector::serving();
    let refusing = Collector::serving_to(
        Behaviour::Answer(
            "HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                .to_owned(),
        ),
        None,
    );
    let unit = Correlation::minted(None);

    runtime().block_on(nest_rs_core::with_request_scope(None, unit, async {
        client()
            .post(format!(
                "{}/hooks/secret-in-the-path?token=secret",
                collector.endpoint()
            ))
            .send()
            .await
            .expect("answered");
        client()
            .get(refusing.endpoint())
            .send()
            .await
            .expect("answered");
    }));

    let recorded = RECORDED.lock().expect("the recorded").clone();
    let [accepted, refused] = recorded.as_slice() else {
        panic!("two requests recorded: {recorded:#?}");
    };
    assert_eq!(accepted.name, "POST");
    let port = |collector: &Collector| {
        collector
            .endpoint()
            .rsplit(':')
            .next()
            .expect("a port")
            .to_owned()
    };
    let attributes = |status: &str, collector: &Collector| {
        vec![
            ("http.request.method".to_owned(), "POST".to_owned()),
            ("url.scheme".to_owned(), "https".to_owned()),
            ("server.address".to_owned(), "127.0.0.1".to_owned()),
            ("server.port".to_owned(), port(collector)),
            ("http.response.status_code".to_owned(), status.to_owned()),
        ]
    };
    assert_eq!(accepted.attributes, attributes("200", &collector));
    assert_eq!(accepted.error_type, None);
    assert_eq!(
        accepted.announced.as_deref(),
        collector.received()[0]
            .header("traceparent")
            .and_then(|parent| parent.split('-').nth(2)),
        "the span recorded is the one the peer was told is its parent",
    );

    assert_eq!(refused.name, "GET");
    assert_eq!(refused.error_type.as_deref(), Some("503"));
    assert!(!format!("{recorded:?}").contains("secret"), "{recorded:#?}");
}
