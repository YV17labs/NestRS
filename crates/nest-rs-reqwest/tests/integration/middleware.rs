//! Covers `src/middleware.rs`: a request an app's own client sends inside a
//! unit carries a span of its own in the unit's trace, one sent outside every
//! unit carries none, and each is recorded as a client operation by its method,
//! its server and how it ended — never its path or its query.

use std::sync::Mutex;
use std::time::Duration;

use nest_rs_core::client_operation::{
    ClientOperation, HTTP_CLIENT_REQUEST_DURATION, HTTP_REQUEST_METHOD, HTTP_RESPONSE_STATUS_CODE,
    NETWORK_PROTOCOL_VERSION, SERVER_ADDRESS, SERVER_PORT, URL_SCHEME,
};
use nest_rs_core::{Correlation, TraceParent};

use crate::{Behaviour, Peer, client};

/// Long enough for any answer the peer gives on loopback.
const ANSWERED: Duration = Duration::from_secs(10);

/// What the seeded recorder was handed for each request: its span's name, its
/// attributes as written, its class if it failed, and the span it is.
#[derive(Clone, Debug)]
struct Recorded {
    name: String,
    duration: &'static str,
    attributes: Vec<(String, String)>,
    error_type: Option<String>,
    span: Option<String>,
}

static RECORDED: Mutex<Vec<Recorded>> = Mutex::new(Vec::new());

fn capture(operation: &ClientOperation<'_>) {
    RECORDED.lock().expect("the recorded").push(Recorded {
        name: operation.name().to_owned(),
        duration: operation.duration().name(),
        attributes: operation
            .attributes()
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.to_string()))
            .collect(),
        error_type: operation.error_type().map(str::to_owned),
        span: operation
            .correlation()
            .map(|correlation| correlation.span_id().to_hex()),
    });
}

/// The span the `traceparent` a peer received named as its parent.
fn parent_named(traceparent: Option<String>) -> Option<String> {
    traceparent
        .as_deref()
        .and_then(TraceParent::parse)
        .map(|parent| parent.parent_id.to_hex())
}

#[tokio::test]
async fn a_request_inside_a_unit_carries_a_span_of_its_own_in_the_units_trace() {
    let peer = Peer::serving(Behaviour::Answer(200)).await;
    let unit = Correlation::minted(None);

    nest_rs_core::with_request_scope(None, unit.clone(), async {
        client(ANSWERED)
            .get(peer.url("/"))
            .send()
            .await
            .expect("answered");
    })
    .await;

    let traceparent = peer
        .header(0, "traceparent")
        .as_deref()
        .and_then(TraceParent::parse)
        .expect("a traceparent");
    assert_eq!(traceparent.trace_id, unit.trace_id(), "the unit's trace");
    assert_ne!(
        traceparent.parent_id,
        unit.span_id(),
        "a span of the request's own, under the unit's"
    );
    assert_eq!(peer.header(0, "baggage"), None);
}

#[tokio::test]
async fn a_request_outside_every_unit_carries_no_trace_context() {
    let peer = Peer::serving(Behaviour::Answer(200)).await;

    client(ANSWERED)
        .get(peer.url("/"))
        .send()
        .await
        .expect("answered");

    assert_eq!(peer.header(0, "traceparent"), None);
}

#[tokio::test]
async fn a_request_is_recorded_by_its_method_server_and_status_never_its_path_or_query() {
    nest_rs_core::__private::set_client_recorder(capture, true);
    let accepting = Peer::serving(Behaviour::Answer(200)).await;
    let refusing = Peer::serving(Behaviour::Answer(503)).await;
    let silent = Peer::serving(Behaviour::Silent).await;
    let unit = Correlation::minted(None);

    nest_rs_core::with_request_scope(None, unit, async {
        client(ANSWERED)
            .post(accepting.url("/hooks/secret-in-the-path?token=secret"))
            .send()
            .await
            .expect("answered");
        client(ANSWERED)
            .get(refusing.url("/"))
            .send()
            .await
            .expect("answered");
        let unanswered = client(Duration::from_millis(200))
            .get(silent.url("/"))
            .send()
            .await;
        assert!(unanswered.is_err(), "the silent peer never answers");
    })
    .await;

    let recorded = RECORDED.lock().expect("the recorded").clone();
    let [accepted, refused, timed_out] = recorded.as_slice() else {
        panic!("three requests recorded: {recorded:#?}");
    };
    assert_eq!(accepted.duration, HTTP_CLIENT_REQUEST_DURATION.name());
    assert_eq!(accepted.name, "POST");
    assert_eq!(
        accepted.attributes,
        [
            (HTTP_REQUEST_METHOD, "POST"),
            (URL_SCHEME, "https"),
            (SERVER_ADDRESS, "127.0.0.1"),
            (SERVER_PORT, &accepting.port().to_string()),
            (HTTP_RESPONSE_STATUS_CODE, "200"),
            (NETWORK_PROTOCOL_VERSION, "1.1"),
        ]
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
    );
    assert_eq!(accepted.error_type, None);
    assert_eq!(
        accepted.span,
        parent_named(accepting.header(0, "traceparent")),
        "the span recorded is the one the peer was told is its parent",
    );

    assert_eq!(refused.name, "GET");
    assert_eq!(refused.error_type.as_deref(), Some("503"));
    assert_eq!(timed_out.error_type.as_deref(), Some("timeout"));
    assert!(!format!("{recorded:?}").contains("secret"), "{recorded:#?}");
}

#[tokio::test]
async fn a_request_its_caller_gives_up_on_is_still_the_span_its_peer_was_told_of() {
    nest_rs_core::__private::set_client_recorder(capture, true);
    let silent = Peer::serving(Behaviour::Silent).await;
    let unit = Correlation::minted(None);

    nest_rs_core::with_request_scope(None, unit, async {
        let given_up = tokio::time::timeout(
            Duration::from_millis(200),
            client(ANSWERED).get(silent.url("/")).send(),
        )
        .await;
        assert!(given_up.is_err(), "the caller gave up first");
    })
    .await;

    let recorded = RECORDED.lock().expect("the recorded").clone();
    let [given_up] = recorded.as_slice() else {
        panic!("one request recorded: {recorded:#?}");
    };
    assert_eq!(
        given_up.error_type, None,
        "a request given up on failed nothing"
    );
    assert!(
        !given_up
            .attributes
            .iter()
            .any(|(key, _)| key == HTTP_RESPONSE_STATUS_CODE)
    );
    assert_eq!(
        given_up.span,
        parent_named(silent.header(0, "traceparent")),
        "the peer's parent exists",
    );
}
