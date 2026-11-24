//! What the policy tells of a failure, wherever it puts one in a line: the
//! error's chain, no URL in it past its origin. An HTTP client's error quotes
//! the URL its request was sent to, whose path and query hold what the caller
//! sent — an identifier, a token — and the dependency's error rides the
//! retry's line and the breaker's opening.

use std::borrow::Cow;
use std::error::Error;

use nest_rs_core::error_message;

/// What the policy tells of `error`: its chain, as [`error_message`] says it,
/// with every URL in it cut to its origin — RFC 3986's scheme and authority,
/// the userinfo dropped, the path, the query and the fragment gone — whatever
/// wrote the sentence: a client's error, with or without a `SafeLink` of its
/// driver's linked in, or an app's error handing that sentence on as its own
/// (`#[error(transparent)]`), which hides the client's error from the chain.
///
/// A URL is read as RFC 3986 writes one with an authority, `scheme://…`, up
/// to the next whitespace, double quote or angle bracket; a closing bracket or
/// quote it does not open is the sentence's, with the punctuation after it,
/// and stays. Not caught: an address worded otherwise — a URI with no
/// authority (`mailto:`), a path or a query quoted alone — and a value an
/// error quotes that is no URL. This covers the lines this crate files; the
/// line an edge files for an error an app returns is `nest-rs-core`'s to say.
pub(crate) fn told(error: &(dyn Error + 'static)) -> String {
    let chain = error_message(error);
    match origins(&chain) {
        Cow::Borrowed(_) => chain,
        Cow::Owned(cut) => cut,
    }
}

/// `text`, each URL in it cut to its origin.
fn origins(text: &str) -> Cow<'_, str> {
    const AUTHORITY_FOLLOWS: &str = "://";

    let mut cut = String::new();
    // Up to `kept`, `text` is in `cut` as it is told.
    let mut kept = 0;
    let mut from = 0;
    while let Some(found) = text[from..].find(AUTHORITY_FOLLOWS) {
        let colon = from + found;
        let after = colon + AUTHORITY_FOLLOWS.len();
        from = after;
        if scheme(&text[..colon]).is_empty() {
            continue;
        }
        let rest = &text[after..];
        let url = unclosed(&rest[..rest.find(ends_a_url).unwrap_or(rest.len())]);
        let origin = host_and_port(url);
        from = after + url.len();
        if origin.len() < url.len() {
            cut.push_str(&text[kept..after]);
            cut.push_str(origin);
            kept = from;
        }
    }
    if kept == 0 {
        return Cow::Borrowed(text);
    }
    cut.push_str(&text[kept..]);
    Cow::Owned(cut)
}

/// The scheme `before` ends with: `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`.
fn scheme(before: &str) -> &str {
    let run = before
        .bytes()
        .rev()
        .take_while(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
        .count();
    before[before.len() - run..].trim_start_matches(|first: char| !first.is_ascii_alphabetic())
}

/// Whether no URL holds `character` unencoded, in any of its parts.
fn ends_a_url(character: char) -> bool {
    character.is_whitespace() || character.is_control() || matches!(character, '"' | '<' | '>')
}

/// `run` without what the sentence around it closes it with: a bracket or a
/// quote the run does not open, and the punctuation after it. Punctuation
/// after anything else may be the URL's own, and stays in the run.
fn unclosed(run: &str) -> &str {
    let mut url = run;
    loop {
        let closed = url.trim_end_matches(['.', ',', ';', ':', '!', '?']);
        let count = |sought: char| closed.matches(sought).count();
        let closes = match closed.chars().next_back() {
            Some(')') => count(')') > count('('),
            Some(']') => count(']') > count('['),
            Some('}') => count('}') > count('{'),
            Some('\'') => count('\'') % 2 == 1,
            _ => false,
        };
        if !closes {
            return url;
        }
        url = &closed[..closed.len() - 1];
    }
}

/// The host of `url`, what follows its `scheme://`, and its port: the
/// authority without its userinfo, and without a port that is no number, which
/// no authority holds.
fn host_and_port(url: &str) -> &str {
    let authority = &url[..url.find(['/', '?', '#']).unwrap_or(url.len())];
    let host_and_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_userinfo, host_and_port)| host_and_port);
    let host_ends = match host_and_port.strip_prefix('[') {
        Some(literal) => literal.find(']').map(|closing| closing + 2),
        None => host_and_port.find(':'),
    }
    .unwrap_or(host_and_port.len());
    let (host, port) = host_and_port.split_at(host_ends);
    let numbered = port
        .strip_prefix(':')
        .is_some_and(|port| port.bytes().all(|digit| digit.is_ascii_digit()));
    if numbered { host_and_port } else { host }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a URL below carries past its origin.
    const SENT: &str = "s3cr3t";

    /// An error saying `0`, above the cause it holds.
    #[derive(Debug, thiserror::Error)]
    #[error("{0}")]
    struct Said(&'static str, #[source] Option<Box<Said>>);

    #[test]
    fn a_url_is_cut_to_its_scheme_and_authority_wherever_the_sentence_holds_it() {
        for (text, told) in [
            (
                "error sending request for url \
                 (https://user:pw@host:8443/parcels/s3cr3t?token=s3cr3t): client error (Connect)",
                "error sending request for url (https://host:8443): client error (Connect)",
            ),
            (
                "could not fetch https://host/parcels/s3cr3t",
                "could not fetch https://host",
            ),
            (
                "invalid URL \"https://host/p?token=s3cr3t\" given",
                "invalid URL \"https://host\" given",
            ),
            (
                "invalid URL 'https://host/p?token=s3cr3t', twice",
                "invalid URL 'https://host', twice",
            ),
            (
                "see <https://host/p?token=s3cr3t#s3cr3t> for more",
                "see <https://host> for more",
            ),
            (
                "from https://a.example/s3cr3t?k=1 to http://b.example:8080/y#s3cr3t",
                "from https://a.example to http://b.example:8080",
            ),
            (
                "proxied https://proxy.example/?to=https://inner.example/s3cr3t",
                "proxied https://proxy.example",
            ),
            (
                "query {https://host/p?filter={s3cr3t}|s3cr3t}",
                "query {https://host}",
            ),
        ] {
            assert_eq!(origins(text), told);
            assert!(!origins(text).contains(SENT), "{text}");
        }
    }

    #[test]
    fn a_scheme_other_than_https_is_cut_the_same_and_its_userinfo_dropped() {
        for (text, told) in [
            (
                "postgres://ledger:s3cr3t@db.internal:5432/ledger?sslmode=verify-full refused",
                "postgres://db.internal:5432 refused",
            ),
            ("redis://:s3cr3t@cache:6379/0", "redis://cache:6379"),
            ("git+ssh://git@host/s3cr3t.git", "git+ssh://host"),
            ("file:///var/run/s3cr3t", "file://"),
            ("ws://host/socket?ticket=s3cr3t.", "ws://host"),
            ("(HTTPS://HOST/S3CR3T)", "(HTTPS://HOST)"),
        ] {
            assert_eq!(origins(text), told);
        }
    }

    #[test]
    fn an_ipv6_authority_keeps_its_literal_and_its_port() {
        for (text, told) in [
            (
                "https://[2001:db8::1]:8443/parcels/s3cr3t?token=s3cr3t",
                "https://[2001:db8::1]:8443",
            ),
            ("https://user:s3cr3t@[::1]/x", "https://[::1]"),
            (
                "for url (https://[::1]): refused",
                "for url (https://[::1]): refused",
            ),
            ("https://[::1]:s3cr3t/x", "https://[::1]"),
        ] {
            assert_eq!(origins(text), told);
        }
    }

    /// A password holding a `/` its URL did not encode ends the authority
    /// early: what then reads as a port is no number, and is not told.
    #[test]
    fn a_port_that_is_no_number_is_not_told() {
        assert_eq!(
            origins("https://ledger:s3cr/3t@host/x failed"),
            "https://ledger failed"
        );
        assert_eq!(origins("https://host:/x"), "https://host:");
    }

    #[test]
    fn what_is_no_url_is_left_as_it_is_said() {
        for text in [
            "tcp connect error: 127.0.0.1:8443: Connection refused (os error 111)",
            "the upstream answered 503 at host:443",
            "expected `std::fmt::Display`, found a path: //comment",
            "1.2://not-a-scheme/path",
            "://nothing/before",
            "https://",
            "",
        ] {
            assert!(matches!(origins(text), Cow::Borrowed(same) if same == text));
        }
    }

    #[test]
    fn every_link_of_a_chain_is_told_with_its_urls_cut() {
        let chain = Said(
            "the tracker could not be reached",
            Some(Box::new(Said(
                "error sending request for url (https://tracker.example/parcels/s3cr3t)",
                Some(Box::new(Said(
                    "redirected to https://user:s3cr3t@login.example/?next=s3cr3t",
                    None,
                ))),
            ))),
        );
        assert_eq!(
            told(&chain),
            "the tracker could not be reached: error sending request for url \
             (https://tracker.example): redirected to https://login.example"
        );
        assert_eq!(
            told(&Said("the ledger refused the entry", None)),
            "the ledger refused the entry"
        );
    }

    /// An error handing on the sentence of the one it holds, and that one's
    /// cause: the error it holds is no link of its chain, so no `SafeLink`
    /// reads it.
    #[derive(Debug, thiserror::Error)]
    #[error(transparent)]
    struct HandedOn(#[from] reqwest::Error);

    /// reqwest's own error, refused before it is sent: nothing listens, and
    /// nothing is asked of the network.
    async fn unsent() -> reqwest::Error {
        reqwest::Client::new()
            .get(format!("ftp://files.example/{SENT}?token={SENT}"))
            .send()
            .await
            .expect_err("no HTTP scheme")
    }

    #[tokio::test]
    async fn an_error_handing_reqwests_sentence_on_is_told_without_its_path_and_query() {
        let handed_on = HandedOn(unsent().await);
        assert!(
            error_message(&handed_on).contains(SENT),
            "the chain quotes the URL: {}",
            error_message(&handed_on)
        );

        assert_eq!(
            told(&handed_on),
            "builder error for url (ftp://files.example): URL scheme is not allowed"
        );
    }
}
