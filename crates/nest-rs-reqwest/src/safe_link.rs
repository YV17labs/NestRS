//! The [`SafeLink`]s saying reqwest's errors. reqwest's own sentence quotes the
//! URL the request was sent to, whose path and query hold what the caller sent
//! — an identifier, a token — and, of a status, the reason phrase the peer
//! wrote; beneath it lie other crates' errors, whose sentences quote what the
//! peer answered — a trailer hyper refuses, HTTP/2's debug data, the names of
//! a certificate. So the crate that knows reqwest's errors claims them
//! wherever one is a link of a chain, in a program linking this crate — read
//! alone, or beneath an app's error naming it as its source, bare or behind
//! the pointer `std` makes an error of (an `Arc`, a `Box`, an `Arc<dyn
//! Error>`), or inside an I/O error holding one, what a reader over a body's
//! stream makes of it, each of which `nest-rs-core` reads for every sayer —
//! and says of each only what a type it can read tells, or a sentence it
//! knows the constant opening of:
//!
//! - its kind, in reqwest's words, and of a status its class and its code:
//!   never the URL, never the peer's phrase;
//! - beneath it, a link another crate's [`SafeLink`] claims as that crate
//!   says it — the policy's own cut of a body, `nest-rs-resilience`'s — and
//!   a reqwest error the same way; a decode failure as `DecodeError` reports
//!   it, without the value; an I/O error by the sentence its OS or `std`
//!   gives it, and one holding another error by what that one tells, or else
//!   by its kind;
//! - a failure of TLS by rustls's own sentence where its variant holds nothing
//!   the peer wrote but a code of the protocol ([`said_tls`]) — an alert by
//!   its name, an answer that is not TLS by the type it was read as, a peer
//!   sharing no version or suite with this client — and a certificate rustls
//!   refuses by the constant reason it gives ([`refused_certificate`]): a bad
//!   signature, an unknown issuer, a certificate authority's own certificate
//!   presented as a server's, a purpose it does not allow — never the names,
//!   the dates or the bytes the certificate holds;
//! - a cause of any other type by the opening of its sentence when that is
//!   one of [`KNOWN`], and by nothing otherwise — hyper's, HTTP/2's, a
//!   redirect policy's, a serializer's — the chain being read on beneath it.
//!
//! An error handing reqwest's sentence on as its own (`#[error(transparent)]`)
//! holds it as no link, so nothing here reads it — reqwest-middleware's
//! apart, which this crate claims — and that sentence's URL and phrase, with
//! every cause beneath it, are its reader's to cut. A pointer to a pointer of
//! a type of its own (`Arc<Box<reqwest::Error>>`) and a reference (`&'static
//! reqwest::Error`) are types no reader names: one handed out as a link is
//! said as `std` displays it, reqwest's sentence whole.
//!
//! An app wording reqwest's errors its own way claims an error type of its
//! own holding one as its source: a second [`SafeLink`] of `reqwest::Error`
//! refuses the boot (`nest_rs_core::ContestedSafeLinkError`).

use std::error::Error;

use nest_rs_core::__private::{claimed_link, decode_failure, link_as};
use nest_rs_core::SafeLink;

nest_rs_core::inventory::submit! { SafeLink::of(&say) }

#[cfg(feature = "middleware")]
nest_rs_core::inventory::submit! { SafeLink::of(&say_middleware) }

/// What a reqwest error of a kind this crate does not know is said as.
const UNREAD: &str = "the HTTP client failed";

/// The constant openings of the sentences a cause is said by where this crate
/// cannot read its type, each the last word on a failure an operator acts on,
/// a longer one before the one it starts with: reqwest's own causes, private
/// to it (a bound of the client's that passed, a scheme it does not speak, a
/// name that did not resolve, a redirect followed past its limit); `std`'s
/// for a lookup that failed; hyper's for a connection closed under a request;
/// and hyper-util's for a proxy that opened no tunnel, a type it keeps
/// private, whose sentences are constants — all but `missing destination
/// host`, which no request meets, reqwest refusing a URL with no host first.
const KNOWN: [&str; 12] = [
    "operation timed out",
    "URL scheme is not allowed",
    "error resolving DNS",
    "too many redirects",
    "failed to lookup address information",
    "connection closed before message completed",
    "tunnel error: proxy authorization required",
    "tunnel error: proxy response headers too long",
    "tunnel error: unexpected end of file",
    "tunnel error: unsuccessful",
    "tunnel error: failed to create underlying connection",
    "tunnel error: io error establishing tunnel",
];

/// rustls's opening for a certificate it refuses, before the reason.
const REFUSED_CERTIFICATE: &str = "invalid peer certificate";

/// A reqwest error said by what this crate reads of it.
fn say(error: &reqwest::Error) -> Option<String> {
    Some(said(error))
}

/// reqwest-middleware's error said as the error it holds. Both variants are
/// transparent: they display that error, URL included, and `source()` skips
/// it.
#[cfg(feature = "middleware")]
fn say_middleware(error: &reqwest_middleware::Error) -> Option<String> {
    Some(match error {
        reqwest_middleware::Error::Reqwest(error) => said(error),
        reqwest_middleware::Error::Middleware(error) => nest_rs_core::error_message(error.as_ref()),
    })
}

/// `error` by its kind, then what the causes beneath it tell.
fn said(error: &reqwest::Error) -> String {
    let mut sentence = kind(error);
    below(error.source(), &mut sentence);
    sentence
}

/// reqwest's sentence for the kind of `error`, which it writes before the URL:
/// a constant, but for a status, said by its class and its code alone —
/// reqwest's own goes on with the phrase the peer wrote.
fn kind(error: &reqwest::Error) -> String {
    if let Some(status) = error.status() {
        let class = if status.is_client_error() {
            "client"
        } else {
            "server"
        };
        return format!("HTTP status {class} error ({})", status.as_u16());
    }
    let kind = if error.is_builder() {
        "builder error"
    } else if error.is_request() {
        "error sending request"
    } else if error.is_body() {
        "request or response body error"
    } else if error.is_decode() {
        "error decoding response body"
    } else if error.is_redirect() {
        "error following redirect"
    } else if error.is_upgrade() {
        "error upgrading connection"
    } else {
        UNREAD
    };
    kind.to_owned()
}

/// Adds to `sentence` what the links from `link` down tell, each after a
/// colon, and answers whether any told something. A link a [`SafeLink`] claims
/// — this crate's own, for a reqwest error, or another driver's — is said by
/// it, causes included: the reading ends there.
fn below(mut link: Option<&(dyn Error + 'static)>, sentence: &mut String) -> bool {
    fn tell(part: &str, sentence: &mut String) {
        sentence.push_str(": ");
        sentence.push_str(part);
    }
    let mut told = false;
    while let Some(cause) = link {
        if let Some(claimed) = claimed_link(cause) {
            tell(&claimed, sentence);
            return true;
        }
        if let Some(io) = link_as::<std::io::Error>(cause) {
            match io.get_ref() {
                // The OS's sentence, or `std`'s own: no library wrote it.
                None => tell(&io.to_string(), sentence),
                // Its sentence is the held error's, a library's: hyper quotes
                // a trailer it refuses there, rustls a certificate's names.
                Some(held) => {
                    if !below(Some(held), sentence) {
                        tell(&io.kind().to_string(), sentence);
                    }
                }
            }
            return true;
        }
        if let Some(report) = decode_failure(cause) {
            tell(&report.to_string(), sentence);
            return true;
        }
        if let Some(refused) = link_as::<rustls::Error>(cause) {
            if let Some(said) = said_tls(refused) {
                tell(&said, sentence);
                told = true;
            }
        } else {
            let written = cause.to_string();
            if let Some(known) = KNOWN.iter().find(|known| written.starts_with(**known)) {
                tell(known, sentence);
                told = true;
            }
        }
        link = cause.source();
    }
    told
}

/// What is said of a failure of TLS, and `None` for one nothing is said of.
///
/// rustls's own sentence where its variant holds nothing the peer wrote but a
/// code of the protocol — an alert's, a message type's, an extension's —
/// which rustls names where it knows it. An allowlist, the enum being open: a
/// rejected Encrypted Client Hello quotes the configuration the peer
/// answered, and a catch-all's text (`General`, `Other`) is written by
/// whoever raised it. A certificate's refusal quotes its names and its dates,
/// so it is said by rustls's opening and its constant reason
/// ([`refused_certificate`]), or the opening alone.
fn said_tls(error: &rustls::Error) -> Option<String> {
    use rustls::Error as Tls;
    use rustls::PeerIncompatible::ServerRejectedEncryptedClientHello as RejectedHello;

    match error {
        Tls::InvalidCertificate(refused) => Some(match refused_certificate(refused) {
            Some(reason) => format!("{REFUSED_CERTIFICATE}: {reason}"),
            None => REFUSED_CERTIFICATE.to_owned(),
        }),
        Tls::PeerIncompatible(RejectedHello(_)) => None,
        Tls::AlertReceived(_)
        | Tls::InvalidMessage(_)
        | Tls::InappropriateMessage { .. }
        | Tls::InappropriateHandshakeMessage { .. }
        | Tls::PeerMisbehaved(_)
        | Tls::PeerIncompatible(_)
        | Tls::NoCertificatesPresented
        | Tls::DecryptError
        | Tls::PeerSentOversizedRecord
        | Tls::HandshakeNotComplete
        | Tls::NoApplicationProtocol
        | Tls::UnsupportedNameType
        | Tls::EncryptError
        | Tls::FailedToGetCurrentTime
        | Tls::FailedToGetRandomBytes
        | Tls::BadMaxFragmentSize => Some(error.to_string()),
        _ => None,
    }
}

/// Why rustls refused a certificate, as a constant: a variant holding nothing
/// by rustls's own word for it, its name; one holding what the certificate
/// does — the dates it is valid between, the names it is for, the usages it
/// allows, an algorithm's identifier — by the constant opening of rustls's
/// sentence, or the name of the variant it replaced; and webpki's own refusal,
/// which rustls hands on inside `Other` ([`refused_by_webpki`]). `None` for a
/// variant read off no release this crate was written against, and for an
/// `Other` a verifier of the app's raised, whose text is its own.
fn refused_certificate(error: &rustls::CertificateError) -> Option<String> {
    use rustls::CertificateError as Refused;
    use rustls::ExtendedKeyPurpose as Purpose;

    let reason = match error {
        Refused::BadEncoding
        | Refused::Expired
        | Refused::NotValidYet
        | Refused::Revoked
        | Refused::UnhandledCriticalExtension
        | Refused::UnknownIssuer
        | Refused::UnknownRevocationStatus
        | Refused::ExpiredRevocationList
        | Refused::BadSignature
        | Refused::NotValidForName
        | Refused::InvalidPurpose
        | Refused::InvalidOcspResponse
        | Refused::ApplicationVerificationFailure => return Some(error.to_string()),
        Refused::ExpiredContext { .. } => "certificate expired",
        Refused::NotValidYetContext { .. } => "certificate not valid yet",
        Refused::ExpiredRevocationListContext { .. } => "certificate revocation list expired",
        Refused::NotValidForNameContext { .. } => "certificate not valid for name",
        // What this client required of the certificate is its own; what the
        // certificate allows instead is the certificate's.
        Refused::InvalidPurposeContext {
            required: Purpose::ServerAuth,
            ..
        } => "certificate does not allow extended key usage for server authentication",
        Refused::InvalidPurposeContext {
            required: Purpose::ClientAuth,
            ..
        } => "certificate does not allow extended key usage for client authentication",
        Refused::InvalidPurposeContext { .. } => "certificate does not allow extended key usage",
        Refused::UnsupportedSignatureAlgorithmContext { .. } => "UnsupportedSignatureAlgorithm",
        Refused::UnsupportedSignatureAlgorithmForPublicKeyContext { .. } => {
            "UnsupportedSignatureAlgorithmForPublicKey"
        }
        Refused::Other(raised) => return refused_by_webpki(raised.0.downcast_ref()?),
        _ => return None,
    };
    Some(reason.to_owned())
}

/// webpki's refusal of a certificate, by its name, where rustls has no word of
/// its own for it — a certificate authority's certificate presented as a
/// server's, a name constraint — and the variant holds nothing: an allowlist
/// read off 0.103.17, the enum being open, a variant holding the
/// certificate's names, dates or identifiers never said.
fn refused_by_webpki(error: &webpki::Error) -> Option<String> {
    use webpki::Error as Refused;

    matches!(
        error,
        Refused::BadDer
            | Refused::BadDerTime
            | Refused::CaUsedAsEndEntity
            | Refused::CertDistrusted
            | Refused::CertRevoked
            | Refused::EmptyEkuExtension
            | Refused::EndEntityUsedAsCa
            | Refused::ExtensionValueInvalid
            | Refused::InvalidCertValidity
            | Refused::InvalidCrlNumber
            | Refused::InvalidNetworkMaskConstraint
            | Refused::InvalidSerialNumber
            | Refused::InvalidCrlSignatureForPublicKey
            | Refused::InvalidSignatureForPublicKey
            | Refused::IssuerNotCrlSigner
            | Refused::MalformedDnsIdentifier
            | Refused::MalformedExtensions
            | Refused::MalformedNameConstraint
            | Refused::MaximumNameConstraintComparisonsExceeded
            | Refused::MaximumPathBuildCallsExceeded
            | Refused::MaximumPathDepthExceeded
            | Refused::MaximumSignatureChecksExceeded
            | Refused::NameConstraintViolation
            | Refused::PathLenConstraintViolated
            | Refused::SignatureAlgorithmMismatch
            | Refused::UnknownIssuer
            | Refused::UnknownRevocationStatus
            | Refused::UnsupportedCertVersion
            | Refused::UnsupportedCriticalExtension
            | Refused::UnsupportedCrlIssuingDistributionPoint
            | Refused::UnsupportedCrlVersion
            | Refused::UnsupportedDeltaCrl
            | Refused::UnsupportedIndirectCrl
            | Refused::UnsupportedNameType
            | Refused::UnsupportedRevocationReason
            | Refused::UnsupportedRevocationReasonsPartitioning
    )
    .then(|| error.to_string())
}

#[cfg(test)]
mod tests {
    use nest_rs_core::__private::find_in_chain;
    use nest_rs_core::error_message;

    use super::*;

    /// What the request's URL carries, in its path and in its query.
    const SENT: &str = "s3cr3t";

    /// What a request to a loopback port nothing listens on fails with.
    async fn refused() -> reqwest::Error {
        let port = {
            let closed = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
            closed.local_addr().expect("its address").port()
        };
        reqwest::Client::new()
            .get(format!(
                "https://127.0.0.1:{port}/parcels/{SENT}?token={SENT}"
            ))
            .send()
            .await
            .expect_err("nothing listens there")
    }

    #[tokio::test]
    async fn a_request_that_failed_is_said_by_its_kind_and_its_causes_without_its_url() {
        let error = refused().await;
        assert!(
            error.to_string().contains(SENT),
            "reqwest's own sentence quotes the URL: {error}"
        );
        let refusal = find_in_chain::<std::io::Error>(&error)
            .expect("the connection's refusal")
            .to_string();

        let said = error_message(&error);
        assert!(
            said.starts_with("error sending request: "),
            "reqwest's sentence for the kind: {said}"
        );
        assert!(said.ends_with(&refusal), "down to the last cause: {said}");
        for quoted in [SENT, "/parcels", "for url", "127.0.0.1"] {
            assert!(!said.contains(quoted), "`{quoted}` in: {said}");
        }
        assert_eq!(
            said,
            error_message(&error.without_url()),
            "as reqwest says it once its URL is taken out"
        );
    }

    /// A request refused before it is sent holds its URL too, and a cause
    /// that says something.
    #[tokio::test]
    async fn a_request_refused_unsent_is_said_without_its_url() {
        let error = reqwest::Client::new()
            .get(format!("ftp://files.example/{SENT}?token={SENT}"))
            .send()
            .await
            .expect_err("no HTTP scheme");
        assert!(error.to_string().contains(SENT), "{error}");

        assert_eq!(
            error_message(&error),
            "builder error: URL scheme is not allowed"
        );
    }

    /// A userinfo the request's URL holds goes with the URL, cut whole: no
    /// host is read out of it, so none is mistaken for one.
    #[tokio::test]
    async fn a_request_whose_url_holds_a_userinfo_is_said_without_it() {
        for userinfo in ["ledger:hunter2", "ledger:2024%2FSpring", "ledger"] {
            let error = reqwest::Client::new()
                .get(format!("ftp://{userinfo}@files.example/{SENT}"))
                .send()
                .await
                .expect_err("no HTTP scheme");

            assert_eq!(
                error_message(&error),
                "builder error: URL scheme is not allowed",
                "{userinfo}"
            );
        }
    }

    /// The first opening a sentence starts with is the one said: a shorter
    /// one listed first would hide the longer.
    #[test]
    fn no_known_opening_comes_before_a_longer_one_it_starts() {
        for (at, known) in KNOWN.iter().enumerate() {
            for later in &KNOWN[at + 1..] {
                assert!(!later.starts_with(known), "`{known}` hides `{later}`");
            }
        }
    }

    /// What `below` adds for a failure of TLS as tokio-rustls hands it back:
    /// an I/O error holding rustls's.
    fn beneath_tls(refused: rustls::Error) -> String {
        let failed = std::io::Error::new(std::io::ErrorKind::InvalidData, refused);
        let mut sentence = String::new();
        below(Some(&failed), &mut sentence);
        sentence
    }

    #[test]
    fn a_tls_failure_holding_nothing_its_peer_wrote_is_said_in_rustls_words() {
        use rustls::Error as Tls;
        use rustls::{AlertDescription, ContentType, HandshakeType};
        use rustls::{InvalidMessage, PeerIncompatible, PeerMisbehaved};

        for refused in [
            Tls::AlertReceived(AlertDescription::CertificateExpired),
            Tls::InvalidMessage(InvalidMessage::InvalidContentType),
            Tls::InappropriateMessage {
                expect_types: vec![ContentType::Handshake],
                got_type: ContentType::ApplicationData,
            },
            Tls::InappropriateHandshakeMessage {
                expect_types: vec![HandshakeType::ServerHello],
                got_type: HandshakeType::Finished,
            },
            Tls::PeerMisbehaved(PeerMisbehaved::SelectedUnofferedCipherSuite),
            Tls::PeerIncompatible(PeerIncompatible::Tls12NotOffered),
            Tls::NoCertificatesPresented,
            Tls::DecryptError,
            Tls::PeerSentOversizedRecord,
            Tls::HandshakeNotComplete,
            Tls::NoApplicationProtocol,
            Tls::UnsupportedNameType,
            Tls::EncryptError,
            Tls::FailedToGetCurrentTime,
            Tls::FailedToGetRandomBytes,
            Tls::BadMaxFragmentSize,
        ] {
            let own = refused.to_string();
            assert_eq!(beneath_tls(refused), format!(": {own}"));
        }
    }

    /// What the allowlist leaves out is said by the kind of the I/O error
    /// holding it: a text whoever raised the failure wrote, or the peer.
    #[test]
    fn a_tls_failure_quoting_its_peer_or_its_raiser_is_said_by_its_io_kind() {
        use rustls::Error as Tls;
        use rustls::{CertRevocationListError, EncryptedClientHelloError, PeerIncompatible};

        let raised = std::sync::Arc::new(std::io::Error::other(SENT));
        for refused in [
            Tls::General(SENT.to_owned()),
            Tls::Other(rustls::OtherError(raised)),
            Tls::InvalidCertRevocationList(CertRevocationListError::BadSignature),
            Tls::InvalidEncryptedClientHello(EncryptedClientHelloError::NoCompatibleConfig),
            Tls::PeerIncompatible(PeerIncompatible::ServerRejectedEncryptedClientHello(None)),
        ] {
            assert_eq!(
                beneath_tls(refused),
                format!(": {}", std::io::ErrorKind::InvalidData)
            );
        }
    }

    /// rustls goes on from each opening with what the certificate holds: the
    /// names its issuer wrote, the dates it is valid between.
    #[test]
    fn a_certificate_rustls_refuses_is_said_without_its_names_or_its_dates() {
        use std::time::Duration;

        use rustls::CertificateError;
        use rustls::pki_types::{ServerName, UnixTime};

        let at = |secs| UnixTime::since_unix_epoch(Duration::from_secs(secs));
        let dialled = ServerName::try_from("carrier.example").expect("a DNS name");
        for (refused, said, quoted) in [
            (
                CertificateError::ExpiredContext {
                    time: at(1_800_000_000),
                    not_after: at(1_700_000_000),
                },
                "certificate expired",
                "1700000000",
            ),
            (
                CertificateError::NotValidYetContext {
                    time: at(1_700_000_000),
                    not_before: at(1_800_000_000),
                },
                "certificate not valid yet",
                "1800000000",
            ),
            (
                CertificateError::NotValidForNameContext {
                    expected: dialled,
                    presented: vec!["DnsName(\"ledger.internal\")".to_owned()],
                },
                "certificate not valid for name",
                "ledger.internal",
            ),
        ] {
            let refused = rustls::Error::InvalidCertificate(refused);
            assert!(refused.to_string().contains(quoted), "{refused}");
            assert_eq!(
                beneath_tls(refused),
                format!(": invalid peer certificate: {said}")
            );
        }
    }

    /// What tells one refused certificate from another, and what to change
    /// when reissuing it: rustls's own word where its variant holds nothing.
    #[test]
    fn a_certificate_rustls_refuses_is_said_by_its_constant_reason() {
        use rustls::CertificateError;

        for (refused, reason) in [
            (CertificateError::BadEncoding, "BadEncoding"),
            (CertificateError::Expired, "Expired"),
            (CertificateError::NotValidYet, "NotValidYet"),
            (CertificateError::Revoked, "Revoked"),
            (
                CertificateError::UnhandledCriticalExtension,
                "UnhandledCriticalExtension",
            ),
            (CertificateError::UnknownIssuer, "UnknownIssuer"),
            (
                CertificateError::UnknownRevocationStatus,
                "UnknownRevocationStatus",
            ),
            (
                CertificateError::ExpiredRevocationList,
                "ExpiredRevocationList",
            ),
            (CertificateError::BadSignature, "BadSignature"),
            (CertificateError::NotValidForName, "NotValidForName"),
            (CertificateError::InvalidPurpose, "InvalidPurpose"),
            (CertificateError::InvalidOcspResponse, "InvalidOcspResponse"),
            (
                CertificateError::ApplicationVerificationFailure,
                "ApplicationVerificationFailure",
            ),
        ] {
            assert_eq!(
                beneath_tls(refused.into()),
                format!(": invalid peer certificate: {reason}")
            );
        }
    }

    /// A variant holding what the certificate does is said by rustls's
    /// opening for it, never by what it holds: the usages the certificate
    /// allows, a revocation list's dates, an algorithm's identifier.
    #[test]
    fn a_certificate_refused_for_what_it_holds_is_said_without_it() {
        use std::time::Duration;

        use rustls::pki_types::UnixTime;
        use rustls::{CertificateError, ExtendedKeyPurpose};

        let at = |secs| UnixTime::since_unix_epoch(Duration::from_secs(secs));
        for (refused, said, quoted) in [
            (
                CertificateError::InvalidPurposeContext {
                    required: ExtendedKeyPurpose::ServerAuth,
                    presented: vec![ExtendedKeyPurpose::ClientAuth],
                },
                "certificate does not allow extended key usage for server authentication",
                "client authentication",
            ),
            (
                CertificateError::InvalidPurposeContext {
                    required: ExtendedKeyPurpose::ClientAuth,
                    presented: vec![ExtendedKeyPurpose::Other(vec![1, 3, 6, 1, 4, 1, 4242])],
                },
                "certificate does not allow extended key usage for client authentication",
                "4242",
            ),
            (
                CertificateError::InvalidPurposeContext {
                    required: ExtendedKeyPurpose::Other(vec![1, 3, 6, 1, 4, 1, 4242]),
                    presented: vec![ExtendedKeyPurpose::ServerAuth],
                },
                "certificate does not allow extended key usage",
                "4242",
            ),
            (
                CertificateError::ExpiredRevocationListContext {
                    time: at(1_800_000_000),
                    next_update: at(1_700_000_000),
                },
                "certificate revocation list expired",
                "1700000000",
            ),
            (
                CertificateError::UnsupportedSignatureAlgorithmContext {
                    signature_algorithm_id: vec![42, 134, 72],
                    supported_algorithms: Vec::new(),
                },
                "UnsupportedSignatureAlgorithm",
                "134",
            ),
            (
                CertificateError::UnsupportedSignatureAlgorithmForPublicKeyContext {
                    signature_algorithm_id: vec![42, 134, 72],
                    public_key_algorithm_id: vec![42, 134, 73],
                },
                "UnsupportedSignatureAlgorithmForPublicKey",
                "134",
            ),
        ] {
            let refused = rustls::Error::InvalidCertificate(refused);
            assert!(refused.to_string().contains(quoted), "{refused}");
            assert_eq!(
                beneath_tls(refused),
                format!(": invalid peer certificate: {said}")
            );
        }
    }

    /// webpki's refusals rustls has no word for reach a line inside `Other`:
    /// one holding nothing is said by its name, and a text a verifier of the
    /// app's raised, or a variant holding the certificate's names, is not.
    #[test]
    fn a_certificate_webpki_refuses_is_said_by_its_name_and_any_other_raiser_by_nothing() {
        use std::sync::Arc;

        use rustls::pki_types::ServerName;
        use rustls::{CertificateError, OtherError};

        let other = |raised: Arc<dyn Error + Send + Sync>| {
            rustls::Error::InvalidCertificate(CertificateError::Other(OtherError(raised)))
        };
        for (refused, name) in [
            (webpki::Error::CaUsedAsEndEntity, "CaUsedAsEndEntity"),
            (webpki::Error::EndEntityUsedAsCa, "EndEntityUsedAsCa"),
            (
                webpki::Error::NameConstraintViolation,
                "NameConstraintViolation",
            ),
            (
                webpki::Error::PathLenConstraintViolated,
                "PathLenConstraintViolated",
            ),
            (
                webpki::Error::UnsupportedCertVersion,
                "UnsupportedCertVersion",
            ),
        ] {
            let refused = other(Arc::new(refused));
            assert_eq!(
                beneath_tls(refused),
                format!(": invalid peer certificate: {name}")
            );
        }

        let raised = other(Arc::new(std::io::Error::other(SENT)));
        assert!(raised.to_string().contains(SENT), "{raised}");
        assert_eq!(beneath_tls(raised), ": invalid peer certificate");

        let named = other(Arc::new(webpki::Error::CertNotValidForName(
            webpki::InvalidNameContext {
                expected: ServerName::try_from("carrier.example").expect("a DNS name"),
                presented: vec!["DnsName(\"ledger.internal\")".to_owned()],
            },
        )));
        assert!(named.to_string().contains("ledger.internal"), "{named}");
        assert_eq!(beneath_tls(named), ": invalid peer certificate");
    }

    /// What `#[error(transparent)]` expands to: reqwest's sentence as its own,
    /// and reqwest's cause as its source.
    #[derive(Debug)]
    struct HandedOn(reqwest::Error);

    impl std::fmt::Display for HandedOn {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            std::fmt::Display::fmt(&self.0, f)
        }
    }

    impl Error for HandedOn {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            self.0.source()
        }
    }

    /// The limit this file's `//!` states: such an error's chain holds no
    /// reqwest error to claim, and says the URL.
    #[tokio::test]
    async fn an_error_handing_reqwests_sentence_on_is_no_link_this_crate_reads() {
        let said = error_message(&HandedOn(refused().await));
        assert!(said.contains(SENT), "{said}");
    }

    /// What `#[source]` makes of a field: the field handed out as the link.
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

    /// The other limit this file's `//!` states: one pointer is read through,
    /// a pointer to a pointer is a type this crate does not name.
    #[tokio::test]
    async fn a_pointer_to_a_pointer_is_no_link_this_crate_reads() {
        use std::sync::Arc;

        let once = error_message(&Naming(Arc::new(refused().await)));
        assert!(!once.contains(SENT), "{once}");
        assert_eq!(once, error_message(&Naming(Box::new(refused().await))));

        let twice = error_message(&Naming(Arc::new(Box::new(refused().await))));
        assert!(twice.contains(SENT), "{twice}");
    }

    /// reqwest-middleware's error displays the error it holds and hands out
    /// that error's source, so the reqwest error is no link of its chain.
    #[cfg(feature = "middleware")]
    #[tokio::test]
    async fn reqwest_middlewares_error_is_said_as_the_error_it_holds() {
        let said = error_message(&refused().await);

        let sent = reqwest_middleware::Error::Reqwest(refused().await);
        assert!(sent.to_string().contains(SENT), "{sent}");
        assert_eq!(error_message(&sent), said);

        let inner = reqwest_middleware::Error::middleware(refused().await);
        assert!(inner.to_string().contains(SENT), "{inner}");
        assert_eq!(error_message(&inner), said);

        let own = reqwest_middleware::Error::middleware(std::io::Error::other("no route to host"));
        assert_eq!(error_message(&own), "no route to host");
    }
}
