//! The [`SafeLink`] saying reqwest's errors: reqwest's own sentence quotes the
//! URL the request was sent to, whose path and query hold what the caller sent
//! — an identifier, a token — so the crate that knows reqwest's errors claims
//! them wherever one is a link of a chain, in a program linking this crate:
//! read alone, or beneath an app's error naming it as its source. An error
//! handing reqwest's sentence on as its own (`#[error(transparent)]`) holds it
//! as no link, so nothing here reads it — reqwest-middleware's apart, which
//! this crate knows — and that sentence's URL is its reader's to cut.

use std::error::Error;

use nest_rs_core::{SafeLink, error_message};

nest_rs_core::inventory::submit! { SafeLink::new(say) }

/// What a reqwest error is said as should its sentence no longer end with the
/// URL it holds: nothing of that sentence is then known to be free of it.
const UNREAD: &str = "the HTTP client failed";

/// A reqwest error, and reqwest-middleware's, said without the request's URL;
/// any other link is its own driver's to say.
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
            reqwest_middleware::Error::Middleware(error) => error_message(error.as_ref()),
        });
    }
    None
}

/// reqwest's own sentence for `error` without the URL it ends with, then what
/// the causes beneath it say.
fn said(error: &reqwest::Error) -> String {
    let displayed = error.to_string();
    let own = match error.url() {
        None => displayed.as_str(),
        Some(url) => displayed
            .strip_suffix(&format!(" for url ({url})"))
            .unwrap_or(UNREAD),
    };
    match error.source() {
        Some(cause) => format!("{own}: {}", error_message(cause)),
        None => own.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_core::__private::find_in_chain;

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
