//! Covers `src/safe_link.rs` against real answers: what `error_message` says
//! of each kind of error reqwest raises on an exchange with a TLS peer — its
//! kind, a status by its code, a cause by what its type tells — and never a
//! word the peer wrote, nor the URL the request was sent to.

use std::error::Error;
use std::time::Duration;

use nest_rs_core::error_message;
use nest_rs_testing::{HttpsPeer, PeerAnswer};

use crate::{AUTHORITY, CERTIFICATE};

/// What the request's URL carries, in its path and in its query, and what a
/// peer writes where a row says it does.
const SENT: &str = "s3cr3t";

/// The reason phrase `HttpsPeer` writes on every status line, its own words.
const PHRASE: &str = "Answer";

/// Where each request below is sent on its peer.
const PARCEL: &str = "/parcels/s3cr3t?token=s3cr3t";

/// Long enough for any answer the peer gives on loopback.
const ANSWERED: Duration = Duration::from_secs(10);

/// A builder of clients trusting the peer's authority alone.
fn trusting() -> reqwest::ClientBuilder {
    let authority =
        reqwest::Certificate::from_pem(AUTHORITY.pem().as_bytes()).expect("the authority");
    reqwest::Client::builder()
        .tls_certs_only([authority])
        .timeout(ANSWERED)
}

async fn peer(script: impl IntoIterator<Item = PeerAnswer>) -> HttpsPeer {
    HttpsPeer::answering(&CERTIFICATE, script).await
}

/// `error` as a line says it, which holds nothing the request carried.
fn said(error: &(dyn Error + 'static)) -> String {
    let said = error_message(error);
    for quoted in [SENT, "/parcels", "for url"] {
        assert!(!said.contains(quoted), "`{quoted}` in: {said}");
    }
    said
}

#[tokio::test]
async fn a_status_error_is_said_by_its_class_and_its_code_never_the_peers_own_phrase() {
    for (status, sentence) in [
        (503, "HTTP status server error (503)"),
        (404, "HTTP status client error (404)"),
    ] {
        let peer = peer([PeerAnswer::status(status)]).await;
        let error = trusting()
            .build()
            .expect("a client")
            .get(peer.url(PARCEL))
            .send()
            .await
            .expect("the peer answers")
            .error_for_status()
            .expect_err("no success");
        assert!(
            error.to_string().contains(&format!("({status} {PHRASE})")),
            "reqwest's own sentence quotes the phrase the peer wrote: {error}"
        );

        assert_eq!(said(&error), sentence);
        assert_eq!(
            said(&reqwest_middleware::Error::Reqwest(error)),
            sentence,
            "and beneath reqwest-middleware's error"
        );
    }
}

#[tokio::test]
async fn a_body_that_does_not_decode_is_said_without_the_value_it_holds() {
    let peer = peer([PeerAnswer::body(200, format!("\"{SENT}\""))]).await;
    let error = trusting()
        .build()
        .expect("a client")
        .get(peer.url(PARCEL))
        .send()
        .await
        .expect("the peer answers")
        .json::<u64>()
        .await
        .expect_err("a string is no number");
    assert!(error.is_decode(), "{error:?}");
    assert!(
        format!("{:?}", error.source()).contains(SENT),
        "the decoder quotes the value: {error:?}"
    );

    assert_eq!(
        said(&error),
        "error decoding response body: invalid type: a string, expected u64 at line 1 column 8"
    );
}

#[tokio::test]
async fn a_redirect_to_another_scheme_is_said_without_the_location_the_peer_wrote() {
    let peer = peer([PeerAnswer::status(302).header(
        "location",
        format!("ftp://files.example/{SENT}?token={SENT}"),
    )])
    .await;
    let error = trusting()
        .build()
        .expect("a client")
        .get(peer.url(PARCEL))
        .send()
        .await
        .expect_err("no HTTP scheme to follow");
    assert!(
        error.to_string().contains("ftp://files.example/s3cr3t"),
        "reqwest's own sentence quotes the location: {error}"
    );

    assert_eq!(said(&error), "builder error: URL scheme is not allowed");
}

#[tokio::test]
async fn a_redirect_followed_too_often_is_said_without_any_url_it_went_through() {
    let peer = peer([PeerAnswer::status(302).header("location", PARCEL)]).await;
    let error = trusting()
        .build()
        .expect("a client")
        .get(peer.url(PARCEL))
        .send()
        .await
        .expect_err("the peer redirects to itself for good");
    assert!(error.is_redirect(), "{error:?}");
    assert!(error.to_string().contains(SENT), "{error}");

    assert_eq!(said(&error), "error following redirect: too many redirects");
}

/// An error of a type this crate cannot read, saying what it is made with.
#[derive(Debug)]
struct Quoting(String);

impl std::fmt::Display for Quoting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for Quoting {}

type Cause = Box<dyn Error + Send + Sync>;

/// reqwest's error for a redirect an app's own policy refuses with the cause
/// `refusal` makes of the location the peer wrote: the one seam through which
/// a cause of any type sits beneath a real reqwest error.
async fn refused_with(refusal: fn(String) -> Cause) -> reqwest::Error {
    let peer = peer([PeerAnswer::status(302).header("location", "/moved/s3cr3t")]).await;
    let refusing = reqwest::redirect::Policy::custom(move |attempt| {
        let location = attempt.url().to_string();
        attempt.error(refusal(location))
    });
    let error = trusting()
        .redirect(refusing)
        .build()
        .expect("a client")
        .get(peer.url(PARCEL))
        .send()
        .await
        .expect_err("the policy refuses");
    let cause = error.source().expect("the policy's error").to_string();
    assert!(
        cause.contains("/moved/s3cr3t"),
        "the cause quotes what the peer wrote: {cause}"
    );
    error
}

/// A cause of a type this crate cannot read says nothing: its sentence may
/// hold what the peer wrote, as this one holds the location.
#[tokio::test]
async fn a_cause_of_a_type_this_crate_cannot_read_is_not_said() {
    let error =
        refused_with(|location| Box::new(Quoting(format!("no redirect to {location}")))).await;

    assert_eq!(said(&error), "error following redirect");
}

/// hyper's shape for a trailer it refuses: an I/O error whose sentence is the
/// held error's, quoting what the peer sent.
#[tokio::test]
async fn an_io_error_holding_another_is_said_by_its_kind_never_the_sentence_it_holds() {
    let error = refused_with(|location| {
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Invalid header value: {location}"),
        ))
    })
    .await;

    assert_eq!(
        said(&error),
        format!(
            "error following redirect: {}",
            std::io::ErrorKind::InvalidInput
        )
    );
}

/// A sentence this crate knows the opening of is said by that opening, and
/// what it goes on to quote is not.
#[tokio::test]
async fn a_cause_is_said_by_the_opening_this_crate_knows_never_what_follows_it() {
    let error = refused_with(|location| {
        Box::new(std::io::Error::other(Quoting(format!(
            "invalid peer certificate: certificate not valid for name \"{location}\""
        ))))
    })
    .await;

    assert_eq!(
        said(&error),
        "error following redirect: invalid peer certificate: certificate not valid for name"
    );
}

#[tokio::test]
async fn an_answer_that_does_not_come_is_said_as_timed_out() {
    let peer = peer([PeerAnswer::Silent]).await;
    let error = trusting()
        .timeout(Duration::from_millis(100))
        .build()
        .expect("a client")
        .get(peer.url(PARCEL))
        .send()
        .await
        .expect_err("the peer answers nothing");
    assert!(error.is_timeout(), "{error:?}");

    assert_eq!(said(&error), "error sending request: operation timed out");
}

/// A body cut short is said by reqwest's kinds and the kind of the I/O error
/// under them, which holds hyper's.
#[tokio::test]
async fn a_body_cut_short_is_said_by_its_kinds_and_its_io_error() {
    let peer = peer([PeerAnswer::truncated(200)]).await;
    let error = trusting()
        .build()
        .expect("a client")
        .get(peer.url(PARCEL))
        .send()
        .await
        .expect("the head answers")
        .bytes()
        .await
        .expect_err("half a body");

    assert_eq!(
        said(&error),
        format!(
            "error decoding response body: request or response body error: {}",
            std::io::ErrorKind::UnexpectedEof
        )
    );
}

/// A certificate no authority the client trusts issued: the handshake's
/// failure is an I/O error holding rustls's, said by the opening this crate
/// knows of it.
#[tokio::test]
async fn a_certificate_the_client_does_not_trust_is_said_by_the_handshakes_failure() {
    let peer = peer([PeerAnswer::status(200)]).await;
    let stranger = nest_rs_testing::TestAuthority::new();
    let authority =
        reqwest::Certificate::from_pem(stranger.pem().as_bytes()).expect("another authority");
    let error = reqwest::Client::builder()
        .tls_certs_only([authority])
        .timeout(ANSWERED)
        .build()
        .expect("a client")
        .get(peer.url(PARCEL))
        .send()
        .await
        .expect_err("an unknown issuer");
    assert!(error.is_connect(), "{error:?}");

    assert_eq!(
        said(&error),
        "error sending request: invalid peer certificate"
    );
}

/// rustls names a certificate it refuses by the names that certificate holds,
/// which its issuer wrote.
#[tokio::test]
async fn a_certificate_for_another_name_is_said_without_the_names_it_holds() {
    let other = AUTHORITY.server(&["ledger.internal"]);
    let peer = HttpsPeer::answering(&other, [PeerAnswer::status(200)]).await;
    let error = trusting()
        .build()
        .expect("a client")
        .get(peer.url(PARCEL))
        .send()
        .await
        .expect_err("not this host's certificate");

    assert!(
        error_message(error.source().expect("the connection's failure"))
            .contains("ledger.internal"),
        "rustls's own sentence names them: {error:?}"
    );

    assert_eq!(
        said(&error),
        "error sending request: invalid peer certificate: certificate not valid for name"
    );
}
