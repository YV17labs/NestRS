//! Covers `src/safe_link.rs` against real answers: what `error_message` says
//! of each kind of error reqwest raises on an exchange with a TLS peer, with
//! one refusing the handshake and with a proxy refusing the tunnel — its
//! kind, a status by its code, a cause by what its type tells or its own
//! `SafeLink` says — and never a word the peer wrote, nor the URL the request
//! was sent to, wherever an error holds reqwest's: named bare, behind a
//! pointer or inside an I/O error.

use std::error::Error;
use std::time::Duration;

use nest_rs_core::error_message;
use nest_rs_testing::{HttpsPeer, PeerAnswer};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

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

/// An error another crate's `SafeLink` claims, saying what it is made with and
/// naming a cause that says the same.
#[derive(Debug)]
struct Coded(String, Option<Quoting>);

impl std::fmt::Display for Coded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for Coded {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.1.as_ref().map(|cause| cause as _)
    }
}

fn say_coded(_: &Coded) -> Option<String> {
    Some("refused R42".to_owned())
}

nest_rs_core::inventory::submit! { nest_rs_core::SafeLink::of(&say_coded) }

/// A cause another crate's `SafeLink` claims is said as that crate says it,
/// with nothing beneath it: the driver's sentence is the whole of its meaning.
#[tokio::test]
async fn a_cause_another_crates_safe_link_claims_is_said_by_it_and_nothing_below_it() {
    let error = refused_with(|location| {
        Box::new(Coded(
            format!("no redirect to {location}"),
            Some(Quoting(format!("operation timed out at {location}"))),
        ))
    })
    .await;
    assert_eq!(said(&error), "error following redirect: refused R42");

    let held = refused_with(|location| {
        Box::new(std::io::Error::other(Coded(
            format!("no redirect to {location}"),
            None,
        )))
    })
    .await;
    assert_eq!(said(&held), "error following redirect: refused R42");
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
            "tunnel error: unsuccessful, the proxy answered for \"{location}\""
        ))))
    })
    .await;

    assert_eq!(
        said(&error),
        "error following redirect: tunnel error: unsuccessful"
    );
}

/// rustls's sentence for a certificate is read off its type, never off a text
/// opening as it does: an error of another type spelling one says nothing.
#[tokio::test]
async fn a_text_opening_as_rustls_does_is_no_certificate_this_crate_reads() {
    let error = refused_with(|location| {
        Box::new(Quoting(format!(
            "invalid peer certificate: certificate not valid for name \"{location}\""
        )))
    })
    .await;

    assert_eq!(said(&error), "error following redirect");
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

/// A certificate issued by an authority the client does not trust, under the
/// name of one it does — an authority re-keyed, its old certificate still in
/// the client's store: the handshake's failure is an I/O error holding
/// rustls's, said by its constant reason, which tells it from an issuer the
/// client never heard of.
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
        .expect_err("a signature the trusted key did not make");
    assert!(error.is_connect(), "{error:?}");

    assert_eq!(
        said(&error),
        "error sending request: invalid peer certificate: BadSignature"
    );
}

/// An endpoint serving its authority's own certificate, which the client
/// trusts: webpki refuses it in a way rustls has no word for, read off
/// webpki's error inside rustls's.
#[tokio::test]
async fn an_authoritys_certificate_presented_as_a_servers_is_said_by_webpkis_name_for_it() {
    let peer = HttpsPeer::answering(&AUTHORITY.certificate(), [PeerAnswer::status(200)]).await;
    let error = trusting()
        .build()
        .expect("a client")
        .get(peer.url(PARCEL))
        .send()
        .await
        .expect_err("an authority's certificate is no server's");
    assert!(error.is_connect(), "{error:?}");

    assert_eq!(
        said(&error),
        "error sending request: invalid peer certificate: CaUsedAsEndEntity"
    );
}

/// A certificate issued for client authentication alone, served by an
/// endpoint: said by the usage this client required, never by those the
/// certificate allows.
#[tokio::test]
async fn a_certificate_issued_for_clients_alone_is_said_by_the_usage_it_lacks() {
    let for_clients = AUTHORITY.client("127.0.0.1");
    let peer = HttpsPeer::answering(&for_clients, [PeerAnswer::status(200)]).await;
    let error = trusting()
        .build()
        .expect("a client")
        .get(peer.url(PARCEL))
        .send()
        .await
        .expect_err("no server authentication in its usages");
    assert!(error.is_connect(), "{error:?}");
    assert!(
        error_message(error.source().expect("the connection's failure"))
            .contains("allows client authentication"),
        "rustls's own sentence goes on with what the certificate allows: {error:?}"
    );

    assert_eq!(
        said(&error),
        "error sending request: invalid peer certificate: certificate does not allow extended key \
         usage for server authentication"
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

/// What a peer speaking no protocol does with a connection once it has read
/// its first bytes.
#[derive(Clone)]
enum Raw {
    /// Writes these bytes, as written.
    Answers(Vec<u8>),
    /// Ends its side, having written nothing.
    Closes,
    /// Resets the connection.
    Resets,
}

/// The address of a peer speaking no protocol: it does with each connection
/// what `raw` says, then reads until its client closes.
async fn raw_peer(raw: Raw) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback port");
    let address = listener.local_addr().expect("its address");
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let raw = raw.clone();
            tokio::spawn(async move {
                let mut sent = [0; 4096];
                let _ = socket.read(&mut sent).await;
                match raw {
                    Raw::Answers(answer) => {
                        let _ = socket.write_all(&answer).await;
                    }
                    Raw::Closes => {
                        let _ = socket.shutdown().await;
                    }
                    // A zero linger makes of the close a reset.
                    Raw::Resets => return socket.set_zero_linger().expect("a zero linger"),
                }
                // Closed over bytes unread, the socket resets under its answer.
                while socket.read(&mut sent).await.is_ok_and(|read| read > 0) {}
            });
        }
    });
    address
}

/// A peer refusing the handshake says why in one alert, which rustls names:
/// what tells an expired client certificate from a missing one, or from a
/// version the peer does not speak.
#[tokio::test]
async fn a_handshake_the_peer_refuses_is_said_by_the_alert_it_sent() {
    for (code, alert) in [
        (40, "HandshakeFailure"),
        (45, "CertificateExpired"),
        (48, "UnknownCA"),
        (70, "ProtocolVersion"),
        (116, "CertificateRequired"),
    ] {
        // One TLS record: an alert (21), TLS 1.2 (3, 3), two bytes — fatal
        // (2), then the alert's code.
        let peer = raw_peer(Raw::Answers(vec![21, 3, 3, 0, 2, 2, code])).await;
        let error = trusting()
            .build()
            .expect("a client")
            .get(format!("https://{peer}{PARCEL}"))
            .send()
            .await
            .expect_err("the handshake is refused");
        assert!(error.is_connect(), "{error:?}");

        assert_eq!(
            said(&error),
            format!("error sending request: received fatal alert: {alert}")
        );
    }
}

#[tokio::test]
async fn a_peer_answering_in_plaintext_is_said_as_speaking_no_tls() {
    let plaintext = format!("HTTP/1.1 400 {SENT}\r\n\r\n").into_bytes();
    let peer = raw_peer(Raw::Answers(plaintext)).await;
    let error = trusting()
        .build()
        .expect("a client")
        .get(format!("https://{peer}{PARCEL}"))
        .send()
        .await
        .expect_err("no TLS on that port");

    assert_eq!(
        said(&error),
        "error sending request: received corrupt message of type InvalidContentType"
    );
}

/// What a request for a host beyond `proxy` fails with, no tunnel opened to
/// it.
async fn untunnelled(proxy: std::net::SocketAddr) -> reqwest::Error {
    let through =
        reqwest::Proxy::all(format!("http://ledger:{SENT}@{proxy}")).expect("a proxy's URL");
    let error = trusting()
        .proxy(through)
        .build()
        .expect("a client")
        .get(format!("https://carrier.example{PARCEL}"))
        .send()
        .await
        .expect_err("no tunnel");
    assert!(error.is_connect(), "{error:?}");
    error
}

/// hyper-util's error for a tunnel a proxy did not open is a type it keeps
/// private, whose sentences are constants: the proxy's own answer is not said.
#[tokio::test]
async fn a_proxy_refusing_the_tunnel_is_said_by_why_it_opened_none() {
    let refusing = |status: u16| {
        Raw::Answers(format!("HTTP/1.1 {status} {SENT}\r\ncontent-length: 0\r\n\r\n").into_bytes())
    };
    // Past the 8192 bytes hyper-util reads of a head, and no end to it.
    let endless = format!("HTTP/1.1 200 {SENT}\r\nx-ledger: {}", SENT.repeat(2048));
    for (proxy, why) in [
        (refusing(407), "proxy authorization required"),
        (refusing(502), "unsuccessful"),
        (
            Raw::Answers(endless.into_bytes()),
            "proxy response headers too long",
        ),
        (Raw::Closes, "unexpected end of file"),
    ] {
        let error = untunnelled(raw_peer(proxy).await).await;

        assert_eq!(
            said(&error),
            format!("error sending request: tunnel error: {why}")
        );
    }
}

/// A tunnel the connection failed under is said by where it failed, then by
/// the sentence the OS gives its reason, as beneath any other cause.
#[tokio::test]
async fn a_tunnel_its_connection_failed_under_is_said_with_the_reason_the_os_gave() {
    use std::io::ErrorKind;

    let closed = {
        let closed = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        closed.local_addr().expect("its address")
    };
    for (proxy, why, kind) in [
        (
            closed,
            "failed to create underlying connection",
            ErrorKind::ConnectionRefused,
        ),
        (
            raw_peer(Raw::Resets).await,
            "io error establishing tunnel",
            ErrorKind::ConnectionReset,
        ),
    ] {
        let error = untunnelled(proxy).await;
        let mut reason: &(dyn Error + 'static) = &error;
        while let Some(cause) = reason.source() {
            reason = cause;
        }
        let reason = reason
            .downcast_ref::<std::io::Error>()
            .expect("the OS's error, last of the chain");
        assert_eq!(reason.kind(), kind, "{error:?}");

        assert_eq!(
            said(&error),
            format!("error sending request: tunnel error: {why}: {reason}")
        );
    }
}

/// What thiserror's `#[source]` makes of a field: a sentence of the error's
/// own, and the field handed out as the link — the pointer itself where the
/// field is one, which `std` makes an error displaying what it holds.
#[derive(Debug)]
struct Naming<S>(S);

impl<S> std::fmt::Display for Naming<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the carrier failed")
    }
}

impl<S: Error + 'static> Error for Naming<S> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.0)
    }
}

type Shared = std::sync::Arc<dyn Error + Send + Sync>;

/// Each way an error names reqwest's as its source without holding it bare:
/// shared, since reqwest's error is no `Clone`; boxed, to stay small; erased;
/// beneath reqwest-middleware's; and inside the I/O error a reader over a
/// body's stream makes of it.
fn named(error: reqwest::Error, shape: usize) -> Option<Box<dyn Error>> {
    use std::io::Error as Io;
    use std::sync::Arc;

    use reqwest_middleware::Error as Middleware;

    Some(match shape {
        0 => Box::new(Naming(error)),
        1 => Box::new(Naming(Arc::new(error))),
        2 => Box::new(Naming(Box::new(error))),
        3 => Box::new(Naming(Arc::new(error) as Shared)),
        4 => Box::new(Naming(Arc::new(Box::new(error)) as Shared)),
        5 => Box::new(Naming(Arc::new(Arc::new(error) as Shared) as Shared)),
        6 => Box::new(Naming(Arc::new(Middleware::Reqwest(error)))),
        7 => Box::new(Naming(Box::new(Middleware::Reqwest(error)))),
        8 => Box::new(Naming(Arc::new(Middleware::middleware(error)))),
        9 => Box::new(Naming(Arc::new(Middleware::middleware(Arc::new(error))))),
        10 => Box::new(Naming(Io::other(error))),
        11 => Box::new(Naming(Io::other(Arc::new(error)))),
        12 => Box::new(Naming(Io::other(Io::other(Box::new(error))))),
        13 => Box::new(Naming(Arc::new(Io::other(error)))),
        14 => Box::new(Naming(Io::other(Middleware::Reqwest(error)))),
        _ => return None,
    })
}

/// A pointer to reqwest's error handed out as the link is said as the error
/// it holds: its downcast would fail, and its `Display` is reqwest's whole.
#[tokio::test]
async fn a_reqwest_error_named_as_a_source_behind_a_pointer_is_said_as_one_named_bare() {
    let peer = peer([PeerAnswer::status(503)]).await;
    let client = trusting().build().expect("a client");
    for shape in 0.. {
        let error = client
            .get(peer.url(PARCEL))
            .send()
            .await
            .expect("the peer answers")
            .error_for_status()
            .expect_err("no success");
        let Some(named) = named(error, shape) else {
            assert!(shape > 14, "every shape was read");
            break;
        };
        assert!(
            named
                .source()
                .is_some_and(|link| link.to_string().contains(SENT)),
            "the link's own sentence quotes the URL: {named:?}"
        );

        assert_eq!(
            error_message(&*named),
            "the carrier failed: HTTP status server error (503)",
            "shape {shape}"
        );
    }
}

/// The causes beneath a reqwest error held behind a pointer are said as
/// beneath one held bare: rustls names the certificate it refuses by the
/// names its issuer wrote.
#[tokio::test]
async fn the_causes_of_a_reqwest_error_behind_a_pointer_are_said_without_a_certificates_names() {
    let other = AUTHORITY.server(&["ledger.internal"]);
    let peer = HttpsPeer::answering(&other, [PeerAnswer::status(200)]).await;
    let client = trusting().build().expect("a client");
    for shape in 0.. {
        let error = client
            .get(peer.url(PARCEL))
            .send()
            .await
            .expect_err("not this host's certificate");
        let Some(named) = named(error, shape) else {
            break;
        };

        assert_eq!(
            error_message(&*named),
            "the carrier failed: error sending request: invalid peer certificate: certificate \
             not valid for name",
            "shape {shape}"
        );
    }
}

/// An I/O error holding no reqwest error is no link this crate claims: it is
/// said as `error_message` says any other.
#[tokio::test]
async fn an_io_error_holding_no_reqwest_error_stays_its_own() {
    let own = Naming(std::io::Error::other(Quoting(
        "the disk is full".to_owned(),
    )));
    assert_eq!(said(&own), "the carrier failed: the disk is full");

    let os = Naming(std::io::Error::from(std::io::ErrorKind::BrokenPipe));
    assert_eq!(
        said(&os),
        format!("the carrier failed: {}", std::io::ErrorKind::BrokenPipe)
    );
}
