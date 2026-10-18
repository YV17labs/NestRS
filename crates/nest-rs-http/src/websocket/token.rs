//! The handshake's list-valued fields read token by token (RFC 9110 §5.6.1),
//! and the one reading of `Upgrade` that says a request asks for a WebSocket.

use http::header::{HeaderName, UPGRADE};
use http::{HeaderMap, HeaderValue};

/// Whether `headers` ask to switch to a WebSocket: `Upgrade` lists
/// `websocket` on any line, in any case (RFC 9110 §7.8). `Connection`, a
/// hop-by-hop field a proxy may rewrite, has no say. The browser-origin check
/// reads this as the handshake does, so no upgrade the handshake admits skips
/// it.
pub fn requested(headers: &HeaderMap) -> bool {
    has_token(headers, &UPGRADE, "websocket")
}

/// Whether a `name` line lists `token`, in any case.
pub(super) fn has_token(headers: &HeaderMap, name: &HeaderName, token: &str) -> bool {
    elements(headers.get_all(name)).any(|listed| listed.eq_ignore_ascii_case(token))
}

/// The elements of the comma-separated list `lines` spell (RFC 9110 §5.6.1),
/// in order, each trimmed of its whitespace and the empty ones dropped.
pub(super) fn elements<'a>(
    lines: impl IntoIterator<Item = &'a HeaderValue>,
) -> impl Iterator<Item = &'a str> {
    lines
        .into_iter()
        .filter_map(|line| line.to_str().ok())
        .flat_map(|line| line.split(','))
        .map(|element| element.trim_matches([' ', '\t']))
        .filter(|element| !element.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upgrade(lines: &[&'static str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for line in lines {
            headers.append(UPGRADE, HeaderValue::from_static(line));
        }
        headers
    }

    #[test]
    fn websocket_anywhere_in_the_upgrade_list_asks_for_one() {
        for lines in [
            &["websocket"][..],
            &["WebSocket"],
            &["websocket, h2c"],
            &["h2c ,\twebsocket"],
            &["h2c", "WEBSOCKET"],
        ] {
            assert!(requested(&upgrade(lines)), "{lines:?}");
        }
    }

    #[test]
    fn no_websocket_token_asks_for_none() {
        for lines in [
            &[][..],
            &["h2c"],
            &["websockets"],
            &["websocket/13"],
            &[","],
        ] {
            assert!(!requested(&upgrade(lines)), "{lines:?}");
        }
    }
}
