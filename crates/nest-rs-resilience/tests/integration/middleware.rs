//! Covers `src/middleware.rs`: an app's own reqwest client under a policy sends
//! again what the HTTP method or an `Idempotency-Key` declares idempotent, each
//! attempt a client operation numbered by its resend count, never sends again
//! what it does not, and is refused at once — never sent — while the breaker
//! is open.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use nest_rs_core::client_operation::{ClientOperation, HTTP_REQUEST_RESEND_COUNT, SERVER_PORT};
use nest_rs_core::{Correlation, Problem, code};
use nest_rs_reqwest::ReqwestMiddleware;
use nest_rs_resilience::{
    ResilienceConfig, ResilienceError, ResilienceMiddleware, ResiliencePolicy,
};
use nest_rs_testing::{TestAuthority, TestCertificate};
use reqwest::StatusCode;
use reqwest_middleware::ClientWithMiddleware;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::{policy, quiet};

/// The authority the peer's certificate chains to, and the client trusts.
static AUTHORITY: LazyLock<TestAuthority> = LazyLock::new(TestAuthority::new);

static CERTIFICATE: LazyLock<TestCertificate> = LazyLock::new(|| AUTHORITY.server(&["127.0.0.1"]));

/// One answer of the peer: a status and the headers beside it.
type Answer = (u16, &'static [(&'static str, &'static str)]);

/// One request's line and header lines, names lowercased.
#[derive(Debug, Default)]
struct Head {
    line: String,
    headers: Vec<(String, String)>,
}

impl Head {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }
}

/// An HTTPS peer on loopback, one request per connection, answering each from
/// its script in turn — `200` once the script ran out — and keeping each
/// request's head.
struct Peer {
    addr: SocketAddr,
    heads: Arc<Mutex<Vec<Head>>>,
}

impl Peer {
    async fn answering(script: &[Answer]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
        let addr = listener.local_addr().expect("its address");
        let heads = Arc::new(Mutex::new(Vec::new()));
        let script = Arc::new(Mutex::new(script.iter().copied().collect::<VecDeque<_>>()));
        let acceptor = CERTIFICATE.acceptor(None);
        let kept = Arc::clone(&heads);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (acceptor, kept, script) =
                    (acceptor.clone(), Arc::clone(&kept), Arc::clone(&script));
                tokio::spawn(async move {
                    let Ok(mut tls) = acceptor.accept(stream).await else {
                        return;
                    };
                    let mut read = Vec::new();
                    let mut buf = [0_u8; 1024];
                    while !read.windows(4).any(|window| window == b"\r\n\r\n") {
                        match tls.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(count) => read.extend_from_slice(&buf[..count]),
                        }
                    }
                    kept.lock().expect("the heads").push(head(&read));
                    let (status, headers) = script
                        .lock()
                        .expect("the script")
                        .pop_front()
                        .unwrap_or((200, &[]));
                    let mut answer = format!("HTTP/1.1 {status} Answer\r\ncontent-length: 0\r\n");
                    for (name, value) in headers {
                        answer.push_str(&format!("{name}: {value}\r\n"));
                    }
                    answer.push_str("connection: close\r\n\r\n");
                    let _ = tls.write_all(answer.as_bytes()).await;
                    let _ = tls.shutdown().await;
                });
            }
        });
        Self { addr, heads }
    }

    fn url(&self) -> String {
        format!("https://127.0.0.1:{}/parcels/42", self.addr.port())
    }

    /// How many requests the peer read.
    fn read(&self) -> usize {
        self.heads.lock().expect("the heads").len()
    }

    /// The value of the header `name` (lowercase) on the request read `nth`.
    fn header(&self, nth: usize, name: &str) -> Option<String> {
        self.heads.lock().expect("the heads")[nth]
            .header(name)
            .map(str::to_owned)
    }
}

/// A request's line and header lines.
fn head(read: &[u8]) -> Head {
    let text = String::from_utf8_lossy(read);
    let mut lines = text.lines();
    Head {
        line: lines.next().unwrap_or_default().to_owned(),
        headers: lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_lowercase(), value.trim().to_owned()))
            .collect(),
    }
}

/// A client trusting the peer's authority, under `policy`, each attempt traced.
fn client(policy: Arc<ResiliencePolicy>) -> ClientWithMiddleware {
    let authority =
        reqwest::Certificate::from_pem(AUTHORITY.pem().as_bytes()).expect("the authority");
    let client = reqwest::Client::builder()
        .tls_certs_only([authority])
        .build()
        .expect("a client");
    reqwest_middleware::ClientBuilder::new(client)
        .with(ResilienceMiddleware::new(policy))
        .with(ReqwestMiddleware::new())
        .build()
}

/// Two retries, each waiting a millisecond or two: the subject is what is sent
/// again, never how long it waits.
fn retrying() -> ResilienceConfig {
    ResilienceConfig {
        retries: Some(2),
        backoff_first: Duration::from_millis(1),
        backoff_ceiling: Duration::from_millis(2),
        ..quiet()
    }
}

/// The policy's refusal a request ended on.
fn refusal(error: &reqwest_middleware::Error) -> &ResilienceError<Infallible> {
    let reqwest_middleware::Error::Middleware(error) = error else {
        panic!("the policy refused: {error:?}");
    };
    error
        .downcast_ref::<ResilienceError<Infallible>>()
        .expect("the policy's refusal")
}

/// Each request's server port and resend count, as the trace middleware
/// records them.
static RECORDED: Mutex<Vec<(Option<String>, Option<String>)>> = Mutex::new(Vec::new());

fn capture(operation: &ClientOperation<'_>) {
    let attribute = |name: &str| {
        operation
            .attributes()
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.to_string())
    };
    RECORDED
        .lock()
        .expect("the recorded")
        .push((attribute(SERVER_PORT), attribute(HTTP_REQUEST_RESEND_COUNT)));
}

#[tokio::test]
async fn an_idempotent_request_is_sent_again_each_attempt_numbered_by_its_resend_count() {
    nest_rs_core::__private::set_client_recorder(capture, true);
    let peer = Peer::answering(&[(503, &[]), (502, &[])]).await;
    let (_app, policy) = policy(retrying()).await;
    let unit = Correlation::minted(None);

    let answered = nest_rs_core::with_request_scope(None, unit, async {
        client(policy).get(peer.url()).send().await
    })
    .await
    .expect("answered");

    assert_eq!(
        answered.status(),
        StatusCode::OK,
        "the third attempt answered"
    );
    assert_eq!(peer.read(), 3);
    let parents: Vec<_> = (0..3)
        .map(|nth| peer.header(nth, "traceparent").expect("a traceparent"))
        .collect();
    assert!(
        parents[0] != parents[1] && parents[1] != parents[2],
        "each attempt is a span of its own: {parents:?}"
    );
    let port = peer.addr.port().to_string();
    let resent: Vec<Option<String>> = RECORDED
        .lock()
        .expect("the recorded")
        .iter()
        .filter(|(server, _)| server.as_deref() == Some(port.as_str()))
        .map(|(_, resent)| resent.clone())
        .collect();
    assert_eq!(resent, [None, Some("1".to_owned()), Some("2".to_owned())]);
}

#[tokio::test]
async fn a_request_not_declared_idempotent_is_sent_once_and_its_answer_returned() {
    let peer = Peer::answering(&[(503, &[])]).await;
    let (_app, policy) = policy(retrying()).await;

    let answered = client(policy)
        .post(peer.url())
        .body("a parcel")
        .send()
        .await
        .expect("answered");

    assert_eq!(answered.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(peer.read(), 1, "a POST could create the parcel twice");
}

#[tokio::test]
async fn a_request_carrying_an_idempotency_key_is_sent_again_with_its_body() {
    let peer = Peer::answering(&[(500, &[])]).await;
    let (_app, policy) = policy(retrying()).await;

    let answered = client(policy)
        .post(peer.url())
        .header(
            "idempotency-key",
            "\"8e03978e-40d5-43e8-bc93-6894a57f9324\"",
        )
        .body("a parcel")
        .send()
        .await
        .expect("answered");

    assert_eq!(answered.status(), StatusCode::OK);
    assert_eq!(peer.read(), 2);
    assert_eq!(
        peer.header(1, "content-length").as_deref(),
        Some("8"),
        "the body is sent again whole"
    );
    assert!(
        peer.heads.lock().expect("the heads")[1]
            .line
            .starts_with("POST "),
        "the method is kept"
    );
}

#[tokio::test]
async fn a_query_is_idempotent_and_sent_again() {
    let peer = Peer::answering(&[(502, &[])]).await;
    let (_app, policy) = policy(retrying()).await;

    let answered = client(policy)
        .request(reqwest::Method::QUERY, peer.url())
        .body("status:shipped")
        .send()
        .await
        .expect("answered");

    assert_eq!(answered.status(), StatusCode::OK);
    assert_eq!(peer.read(), 2);
}

#[tokio::test]
async fn a_wait_asked_past_the_backoffs_ceiling_ends_the_run_with_that_answer() {
    let peer = Peer::answering(&[(429, &[("retry-after", "3600")])]).await;
    let (_app, policy) = policy(retrying()).await;

    let answered = client(policy)
        .get(peer.url())
        .send()
        .await
        .expect("answered");

    assert_eq!(answered.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        peer.read(),
        1,
        "the peer asked an hour; the policy waits 2 ms at most"
    );
}

#[tokio::test]
async fn a_client_error_is_the_dependencys_answer_and_never_sent_again() {
    let peer = Peer::answering(&[(404, &[])]).await;
    let (_app, policy) = policy(retrying()).await;

    let answered = client(policy)
        .get(peer.url())
        .send()
        .await
        .expect("answered");

    assert_eq!(answered.status(), StatusCode::NOT_FOUND);
    assert_eq!(peer.read(), 1);
}

#[tokio::test]
async fn an_open_breaker_refuses_the_request_unsent_with_the_503_every_edge_answers() {
    let peer = Peer::answering(&[(500, &[])]).await;
    let (_app, policy) = policy(ResilienceConfig {
        breaker_window: Some(1),
        breaker_failures: 1,
        breaker_open: Duration::from_secs(30),
        ..quiet()
    })
    .await;
    let client = client(policy);

    let failed = client.get(peer.url()).send().await.expect("answered");
    assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let refused = client
        .get(peer.url())
        .send()
        .await
        .expect_err("the breaker opened on the 500");
    assert!(
        matches!(refusal(&refused), ResilienceError::Open { .. }),
        "{refused:?}"
    );
    let problem = nest_rs_core::__private::find_in_chain::<Problem>(&refused)
        .expect("the problem rides the chain");
    assert_eq!(
        (problem.status(), problem.code(), problem.retry_after()),
        (503, code::UNAVAILABLE, Some(30))
    );
    assert_eq!(peer.read(), 1, "a refused request is never sent");
}

#[tokio::test]
async fn a_peer_that_never_answers_is_given_up_at_the_policys_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let addr = listener.local_addr().expect("its address");
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            held.push(stream);
        }
    });
    let (_app, policy) = policy(ResilienceConfig {
        timeout: Duration::from_millis(200),
        ..quiet()
    })
    .await;

    let given_up = client(policy)
        .get(format!("https://{addr}/"))
        .send()
        .await
        .expect_err("no answer");

    assert!(
        matches!(
            refusal(&given_up),
            ResilienceError::TimedOut { timeout, .. } if *timeout == Duration::from_millis(200)
        ),
        "{given_up:?}"
    );
}
