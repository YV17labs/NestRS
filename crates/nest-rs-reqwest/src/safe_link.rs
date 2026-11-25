//! The [`SafeLink`] saying reqwest's errors. reqwest's own sentence quotes the
//! URL the request was sent to, whose path and query hold what the caller sent
//! — an identifier, a token — and, of a status, the reason phrase the peer
//! wrote; beneath it lie other crates' errors, whose sentences quote what the
//! peer answered — a trailer hyper refuses, HTTP/2's debug data, the names of
//! a certificate. So the crate that knows reqwest's errors claims them
//! wherever one is a link of a chain, in a program linking this crate — read
//! alone, or beneath an app's error naming it as its source — and says of each
//! only what a type it can read tells, or a sentence it knows the constant
//! opening of:
//!
//! - its kind, in reqwest's words, and of a status its class and its code:
//!   never the URL, never the peer's phrase;
//! - beneath it, a reqwest error the same way; a decode failure as
//!   [`DecodeError`] reports it, without the value; an I/O error by the
//!   sentence its OS or `std` gives it, and one holding another error by what
//!   that one tells, or else by its kind;
//! - a cause of any other type by the opening of its sentence when that is
//!   one of [`KNOWN`], and by nothing otherwise — hyper's, HTTP/2's, a
//!   redirect policy's, a serializer's — the chain being read on beneath it.
//!
//! An error handing reqwest's sentence on as its own (`#[error(transparent)]`)
//! holds it as no link, so nothing here reads it — reqwest-middleware's apart,
//! which this crate knows — and that sentence's URL and phrase, with every
//! cause beneath it, are its reader's to cut.

use std::error::Error;

use nest_rs_core::{DecodeError, SafeLink};

nest_rs_core::inventory::submit! { SafeLink::new(say) }

/// What a reqwest error of a kind this crate does not know is said as.
const UNREAD: &str = "the HTTP client failed";

/// The constant openings of the sentences a cause is said by where this crate
/// cannot read its type, each the last word on a failure an operator acts on,
/// a longer one before the one it starts with: reqwest's own causes, private
/// to it (a bound of the client's that passed, a scheme it does not speak, a
/// name that did not resolve, a redirect followed past its limit); `std`'s
/// for a lookup that failed; hyper's for a connection closed under a request;
/// and rustls's for a certificate it refuses, without the names and the dates
/// it goes on to quote.
const KNOWN: [&str; 11] = [
    "operation timed out",
    "URL scheme is not allowed",
    "error resolving DNS",
    "too many redirects",
    "failed to lookup address information",
    "connection closed before message completed",
    "invalid peer certificate: UnknownIssuer",
    "invalid peer certificate: certificate expired",
    "invalid peer certificate: certificate not valid yet",
    "invalid peer certificate: certificate not valid for name",
    "invalid peer certificate",
];

/// A reqwest error, and reqwest-middleware's, said by what this crate reads of
/// it; any other link is its own driver's to say.
fn say(link: &(dyn Error + 'static)) -> Option<String> {
    if let Some(error) = link.downcast_ref::<reqwest::Error>() {
        return Some(said(error));
    }
    #[cfg(feature = "middleware")]
    if let Some(error) = link.downcast_ref::<reqwest_middleware::Error>() {
        // Both variants are transparent: they display the error they hold, URL
        // included, and `source()` skips it.
        return Some(match error {
            reqwest_middleware::Error::Reqwest(error) => said(error),
            reqwest_middleware::Error::Middleware(error) => {
                nest_rs_core::error_message(error.as_ref())
            }
        });
    }
    None
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
/// colon, and answers whether any told something.
fn below(mut link: Option<&(dyn Error + 'static)>, sentence: &mut String) -> bool {
    fn tell(part: &str, sentence: &mut String) {
        sentence.push_str(": ");
        sentence.push_str(part);
    }
    let mut told = false;
    while let Some(cause) = link {
        if let Some(error) = cause.downcast_ref::<reqwest::Error>() {
            tell(&said(error), sentence);
            return true;
        }
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
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
        // `in_chain` reads on beneath a link: only a last link is read alone.
        if cause.source().is_none()
            && let Some(report) = DecodeError::in_chain(cause)
        {
            tell(&report.to_string(), sentence);
            return true;
        }
        let written = cause.to_string();
        if let Some(known) = KNOWN.iter().find(|known| written.starts_with(**known)) {
            tell(known, sentence);
            told = true;
        }
        link = cause.source();
    }
    told
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
