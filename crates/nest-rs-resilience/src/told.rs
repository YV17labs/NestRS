//! What the policy tells of a failure, wherever it puts one in a line: the
//! error's chain, no URL in it past its origin and no status past its code.
//! An HTTP client's error quotes the URL its request was sent to, whose
//! userinfo, path and query hold what the caller sent — a password, an
//! identifier, a token — and, of a status, the reason phrase the dependency
//! wrote; the dependency's error rides the retry's line and the breaker's
//! opening.

use std::borrow::Cow;
use std::error::Error;

use nest_rs_core::error_message;

/// What the policy tells of `error`: its chain, as [`error_message`] says it,
/// with every URL in it cut to its origin — its scheme, its host and a port of
/// digits — whatever wrote the sentence: a client's error, with or without a
/// `SafeLink` of its driver's linked in, or an app's error handing that
/// sentence on as its own (`#[error(transparent)]`), which hides the client's
/// error from the chain.
///
/// A URL is read wherever `://` is written, whatever precedes it, up to the
/// next whitespace, control character, double quote or angle bracket. A
/// closing bracket or quote it does not open is the sentence's, with the
/// punctuation after it, and so is the punctuation ending an authority alone;
/// both stay. Of what follows the `://`:
///
/// - the host is read after the last `@` of the authority, so no byte of a
///   userinfo is told, an `@` in a password included, nor digits of it as a
///   port;
/// - the authority ends at a `/`, `?` or `#`, or at the `\` a browser reads as
///   a `/`; an `@` after one is a userinfo's that holds it unencoded, or the
///   path's own: the host cannot be told from either, and the URL is told by
///   its scheme alone;
/// - no byte of the path, the query or the fragment is told, nor a port that
///   is no number.
///
/// The cut reads each URL a fixed number of times, whatever it holds.
///
/// A status reqwest's own sentence says, where an error hands that sentence
/// on and no `SafeLink` reads it, is told by its class and its code, and the
/// chain no further: reqwest writes after the code the reason phrase the
/// dependency answered with, which can end anywhere.
///
/// Not caught: a URL holding a whitespace, a control character, a double
/// quote or an angle bracket unencoded — it is read as ending there, so a
/// userinfo holding one is told as a host before it and as the sentence's
/// after it; an address with no `://` — a URI with no authority (`mailto:`),
/// a reference with no scheme (`//host/path`), a bare `user@host`, a path or a
/// query quoted alone; and a value an error quotes that is no URL and no
/// status's phrase — what a cause beneath a sentence handed on says of its
/// own, which only an error naming its client's as its source — bare, behind
/// an `Arc` or a `Box`, or inside the I/O error a reader over a body makes of
/// it — leaves to that client's `SafeLink`. This covers the lines this crate
/// files; the line an edge files for an error an app returns is
/// `nest-rs-core`'s to say.
pub(crate) fn told(error: &(dyn Error + 'static)) -> String {
    let chain = error_message(error);
    let chain = ended_at_a_status(&chain).unwrap_or(chain);
    match origins(&chain) {
        Cow::Borrowed(_) => chain,
        Cow::Owned(cut) => cut,
    }
}

/// What reqwest's own sentence for a status opens with, up to its class.
const STATUS_SAID: &str = "HTTP status ";

/// What follows there for each class reqwest tells, up to the code.
const STATUS_CLASSES: [&str; 2] = ["client error (", "server error ("];

/// `text` ended at the code of the first status reqwest's own sentence says in
/// it, where a phrase follows that code — `None` where none does. reqwest
/// writes there the reason phrase the dependency answered with, which may hold
/// anything, a closing parenthesis included: nothing after the code can be
/// told from it.
fn ended_at_a_status(text: &str) -> Option<String> {
    let mut from = 0;
    while let Some(found) = text[from..].find(STATUS_SAID) {
        from += found + STATUS_SAID.len();
        let Some(code) = STATUS_CLASSES
            .iter()
            .find_map(|class| text[from..].strip_prefix(class))
        else {
            continue;
        };
        let digits = code.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && code[digits..].starts_with(' ') {
            let ends = text.len() - code.len() + digits;
            return Some(format!("{})", &text[..ends]));
        }
    }
    None
}

/// Where RFC 3986 writes a URL's authority: after its scheme.
const AUTHORITY_FOLLOWS: &str = "://";

/// What may end a sentence, or a clause of it, after a URL.
const PUNCTUATION: [char; 6] = ['.', ',', ';', ':', '!', '?'];

/// `text`, each URL in it cut to its origin.
fn origins(text: &str) -> Cow<'_, str> {
    let mut cut = String::new();
    // Up to `kept`, `text` is in `cut` as it is told.
    let mut kept = 0;
    let mut from = 0;
    while let Some(found) = text[from..].find(AUTHORITY_FOLLOWS) {
        let after = from + found + AUTHORITY_FOLLOWS.len();
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

/// Where the authority `url` starts with ends: at RFC 3986's `/`, `?` or `#`,
/// or at the `\` a browser reads as a `/`.
fn authority_end(url: &str) -> Option<usize> {
    url.bytes()
        .position(|byte| matches!(byte, b'/' | b'?' | b'#' | b'\\'))
}

/// Whether no URL holds `character` unencoded, in any of its parts.
fn ends_a_url(character: char) -> bool {
    character.is_whitespace() || character.is_control() || matches!(character, '"' | '<' | '>')
}

/// `run` without what the sentence around it closes it with: a bracket or a
/// quote the run does not open, and the punctuation after it; and, where the
/// run is an authority alone, the punctuation ending it, which no host and no
/// port ends on. Punctuation after anything else may be the URL's own, and
/// stays in the run.
fn unclosed(run: &str) -> &str {
    let mut unopened = Unopened::of(run);
    let mut url = run;
    loop {
        let closed = url.trim_end_matches(PUNCTUATION);
        let closes = closed
            .chars()
            .next_back()
            .is_some_and(|last| unopened.takes(last));
        if !closes {
            let authority_alone = authority_end(closed).is_none();
            return if authority_alone { closed } else { url };
        }
        url = &closed[..closed.len() - 1];
    }
}

/// The closers a run holds and does not open, counted in one reading of it:
/// each bracket past the ones opened, and each quote left without its pair.
#[derive(Default)]
struct Unopened {
    round: isize,
    square: isize,
    curly: isize,
    single: bool,
    backtick: bool,
}

impl Unopened {
    fn of(run: &str) -> Self {
        let mut unopened = Self::default();
        for byte in run.bytes() {
            match byte {
                b'(' => unopened.round -= 1,
                b')' => unopened.round += 1,
                b'[' => unopened.square -= 1,
                b']' => unopened.square += 1,
                b'{' => unopened.curly -= 1,
                b'}' => unopened.curly += 1,
                b'\'' => unopened.single ^= true,
                b'`' => unopened.backtick ^= true,
                _ => {}
            }
        }
        unopened
    }

    /// Whether `last`, which the run ends with, is a closer it does not open,
    /// taken off the count: the run goes on without it.
    fn takes(&mut self, last: char) -> bool {
        let bracket = match last {
            ')' => &mut self.round,
            ']' => &mut self.square,
            '}' => &mut self.curly,
            '\'' => return std::mem::take(&mut self.single),
            '`' => return std::mem::take(&mut self.backtick),
            _ => return false,
        };
        let unopened = *bracket > 0;
        if unopened {
            *bracket -= 1;
        }
        unopened
    }
}

/// The host of `url`, what follows its `://`, and its port: the authority
/// after its last `@`, without a port that is no number, which no authority
/// holds — or nothing, where an `@` follows the authority's end and the host
/// cannot be told from a userinfo.
fn host_and_port(url: &str) -> &str {
    let (authority, past) = url.split_at(authority_end(url).unwrap_or(url.len()));
    if past.contains('@') {
        return "";
    }
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
    fn a_url_is_cut_to_its_scheme_host_and_port_wherever_the_sentence_holds_it() {
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
                "invalid URL `https://host/p?token=s3cr3t`: no base",
                "invalid URL `https://host`: no base",
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
            (
                "refused [https://host/s3cr3t], (https://host/s3cr3t). Then gave up",
                "refused [https://host], (https://host). Then gave up",
            ),
            ("https://host\\s3cr3t", "https://host"),
        ] {
            assert_eq!(origins(text), told);
            assert!(!origins(text).contains(SENT), "{text}");
        }
    }

    /// Punctuation ending a URL past its authority may be its own, so it goes
    /// with what is cut; after an authority alone it is the sentence's, no
    /// host and no port ending on one.
    #[test]
    fn punctuation_ending_a_url_is_the_sentences_after_an_authority_alone() {
        for (text, told) in [
            ("ws://host/socket?ticket=s3cr3t.", "ws://host"),
            (
                "could not fetch https://host/s3cr3t, then gave up",
                "could not fetch https://host then gave up",
            ),
            ("see https://host.", "see https://host."),
            ("see https://host:8443, then", "see https://host:8443, then"),
            ("is it https://host?", "is it https://host?"),
            (
                "refused by https://user:s3cr3t@host:8443.",
                "refused by https://host:8443.",
            ),
        ] {
            assert_eq!(origins(text), told);
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
            (
                "jdbc:postgresql://db:5432/ledger?password=s3cr3t",
                "jdbc:postgresql://db:5432",
            ),
            ("x-app.v2://host/s3cr3t", "x-app.v2://host"),
            ("file:///var/run/s3cr3t", "file://"),
            ("(HTTPS://HOST/S3CR3T)", "(HTTPS://HOST)"),
        ] {
            assert_eq!(origins(text), told);
        }
    }

    /// `://` is read as a URL's whatever precedes it: a scheme RFC 3986 would
    /// refuse, or none, hides a userinfo and a path no better.
    #[test]
    fn what_follows_an_authoritys_mark_is_cut_whatever_precedes_it() {
        for (text, told) in [
            ("://user:s3cr3t@host/s3cr3t", "://host"),
            ("1.2://user:s3cr3t@host/s3cr3t", "1.2://host"),
            ("see a://b/c for more", "see a://b for more"),
            ("https://://s3cr3t", "https://:"),
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
            ("https://user:s3cr3t@[::1]:8443/x", "https://[::1]:8443"),
            (
                "for url (https://[::1]): refused",
                "for url (https://[::1]): refused",
            ),
            ("https://[::1]:s3cr3t/x", "https://[::1]"),
        ] {
            assert_eq!(origins(text), told);
        }
    }

    #[test]
    fn a_port_that_is_no_number_is_not_told() {
        for (text, told) in [
            ("https://host:s3cr3t/x", "https://host"),
            ("https://host:80s3cr3t/x", "https://host"),
            ("https://host:8443/x", "https://host:8443"),
            ("https://host:/x", "https://host:"),
        ] {
            assert_eq!(origins(text), told);
        }
    }

    /// The host is what follows the last `@` of the authority, so nothing of a
    /// userinfo is told, an `@` in its password included, nor digits of it as
    /// a port.
    #[test]
    fn a_userinfo_is_never_told_as_a_host_or_a_port() {
        for (text, told) in [
            ("https://ledger@host/x failed", "https://host failed"),
            ("https://ledger:8443@host", "https://host"),
            ("https://ledger:s3@cr3t@host:8443/x", "https://host:8443"),
            ("https://led%2Fger:s3%40cr%3Ft@host/x", "https://host"),
            ("https://ledger:s3cr3t@", "https://"),
            ("https://ledger:s3cr3t@/x", "https://"),
            ("(https://ledger:s3cr3t)@host/x)", "(https://host)"),
        ] {
            assert_eq!(origins(text), told);
        }
    }

    /// A userinfo holding a `/`, a `?`, a `#` or a `\` it did not encode ends
    /// the authority early for a parser, and an `@` in a path follows one the
    /// same: the host cannot be told from either, so the scheme alone is.
    #[test]
    fn a_url_whose_host_cannot_be_told_from_a_userinfo_is_told_by_its_scheme_alone() {
        for (text, told) in [
            (
                "postgres://ledger:2024/Spring@db.internal:5432/ledger refused",
                "postgres:// refused",
            ),
            ("https://ledger:1234?x9@host/p", "https://"),
            ("https://ledger:s3cr/3t@host/x failed", "https:// failed"),
            ("https://ledger:s3cr#3t@host/x", "https://"),
            ("https://ledger:s3cr\\3t@host/x", "https://"),
            ("https://ledger:s3@cr/3t@host/x", "https://"),
            (
                "for url (https://api.example/users?email=ledger@corp.example): refused",
                "for url (https://): refused",
            ),
            ("https://api.example/users/ledger@corp.example", "https://"),
            (
                "proxied https://proxy.example/?to=https://ledger:s3cr3t@inner.example/x",
                "proxied https://",
            ),
        ] {
            assert_eq!(origins(text), told);
            for kept in [
                "ledger",
                "2024",
                "1234",
                "s3",
                "corp",
                "db.internal",
                "host",
            ] {
                assert!(!origins(text).contains(kept), "`{kept}` of {text}");
            }
        }
    }

    /// Every string of up to `length` characters of `alphabet`.
    fn strings(alphabet: &[char], length: usize) -> Vec<String> {
        let mut all = vec![String::new()];
        let mut last = 0;
        for _ in 0..length {
            let from = std::mem::replace(&mut last, all.len());
            for at in from..all.len() {
                for character in alphabet {
                    let longer = format!("{}{character}", all[at]);
                    all.push(longer);
                }
            }
        }
        all
    }

    /// Every userinfo of a few characters — a name, digits, the delimiters a
    /// URL owes an encoding and the `:` and `@` it may hold — before a host
    /// and a port: none of its letters and none of its digits is told, in any
    /// place.
    #[test]
    fn no_userinfo_tells_a_byte_of_itself_whatever_delimiters_it_holds() {
        for userinfo in strings(&['U', '7', ':', '@', '/', '?', '#', '\\'], 4) {
            for rest in ["", "/p", ":5432/p?q#f"] {
                let text = format!("refused x://{userinfo}@h{rest} twice");
                let told = origins(&text);
                assert!(!told.contains(['U', '7']), "{text} is told {told}");
                assert!(
                    told.starts_with("refused x://") && told.ends_with(" twice"),
                    "{text} is told {told}"
                );
            }
        }
    }

    /// Every path, query and fragment of a few characters after a host and a
    /// port, an `@` in them included: none of its letters and none of its
    /// digits is told.
    #[test]
    fn nothing_past_an_authority_tells_a_byte_of_itself_whatever_it_holds() {
        for start in ['/', '?', '#', '\\'] {
            for past in strings(&['P', '7', ':', '@', '/', '?', '#', '.'], 3) {
                for authority in ["h", "h:5432", "u:w@h:5432"] {
                    let text = format!("refused x://{authority}{start}{past}");
                    let told = origins(&text);
                    assert!(!told.contains(['P', '7']), "{text} is told {told}");
                }
            }
        }
    }

    /// The limit `told` names: a URL is read as ending at a whitespace, a
    /// double quote or an angle bracket, so a userinfo holding one unencoded
    /// is told as a host before it and left as the sentence's after it.
    #[test]
    fn a_userinfo_holding_what_ends_a_url_is_read_as_ending_there() {
        for (text, told) in [
            (
                "postgres://ledger:s3 cr3t@db.internal/ledger refused",
                "postgres://ledger cr3t@db.internal/ledger refused",
            ),
            (
                "invalid URL \"https://ledger:s3\"cr3t@host/x\"",
                "invalid URL \"https://ledger\"cr3t@host/x\"",
            ),
        ] {
            assert_eq!(origins(text), told);
        }
    }

    /// What `told` promises nothing of: an address with no `://`.
    #[test]
    fn what_holds_no_authoritys_mark_is_left_as_it_is_said() {
        for text in [
            "tcp connect error: 127.0.0.1:8443: Connection refused (os error 111)",
            "the upstream answered 503 at host:443",
            "expected `std::fmt::Display`, found a path: //comment",
            "mailto:ledger@corp.example bounced",
            "ledger@db.internal could not be reached",
            "ledger:pw@tcp(db.internal:3306)/ledger",
            "//db.internal/ledger",
            "https://",
            "",
        ] {
            assert!(matches!(origins(text), Cow::Borrowed(same) if same == text));
        }
    }

    /// How long a megabyte may take to cut before the suite calls it hung.
    const HANGS: std::time::Duration = std::time::Duration::from_secs(60);

    /// A megabyte of each shape a dependency may write is cut in a pass linear
    /// in it. No cost is measured: one pass over a megabyte takes
    /// milliseconds, and a pass reading the run again per character of it a
    /// quarter of an hour at the least, which [`HANGS`] tells apart on any
    /// machine.
    #[test]
    fn a_megabyte_of_any_shape_is_cut_in_one_pass() {
        const MEGABYTE: usize = 1 << 20;
        let of = |unit: &str| unit.repeat(MEGABYTE / unit.len());
        let shapes = [
            ("closers", format!("x://a{}", of(")")), None),
            (
                "closers and punctuation",
                format!("x://a/b{}", of(").]!}?")),
                Some(format!("x://a{}", of(").]!}?"))),
            ),
            (
                "openers",
                format!("x://a/{}", of("(")),
                Some("x://a".to_owned()),
            ),
            ("authority marks", of("://"), Some("://:".to_owned())),
            (
                "urls",
                of("a://b/c "),
                Some("a://b ".repeat(MEGABYTE / "a://b/c ".len())),
            ),
            ("port marks", format!("x://a{}", of(":")), None),
            (
                "userinfo marks",
                format!("x://{}", of("@")),
                Some("x://".to_owned()),
            ),
            (
                "userinfo marks in a path",
                format!("x://a/{}", of("@")),
                Some("x://".to_owned()),
            ),
            ("one run", format!("x://{}", of("a")), None),
            (
                "one scheme",
                format!("{}://b/c", of("a")),
                Some(format!("{}://b", of("a"))),
            ),
        ];
        let count = shapes.len();
        let (cut, done) = std::sync::mpsc::channel();
        for (shape, text, told) in shapes {
            let cut = cut.clone();
            std::thread::spawn(move || {
                let as_told = origins(&text) == told.as_deref().unwrap_or(&text);
                cut.send((shape, as_told)).expect("the test waits");
            });
        }
        for _ in 0..count {
            let (shape, as_told) = done
                .recv_timeout(HANGS)
                .expect("a megabyte is cut in one pass");
            assert!(as_told, "{shape}");
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

    #[test]
    fn a_status_reqwests_own_sentence_says_is_told_by_its_code_and_nothing_after_it() {
        for (text, told) in [
            (
                "HTTP status server error (503 shard db-7 refused token=s3cr3t) for url \
                 (https://host/parcels/s3cr3t)",
                "HTTP status server error (503)",
            ),
            (
                "the tracker failed: HTTP status client error (404 no parcel (s3cr3t) here)",
                "the tracker failed: HTTP status client error (404)",
            ),
            (
                "HTTP status server error (502): then HTTP status server error (503 s3cr3t)",
                "HTTP status server error (502): then HTTP status server error (503)",
            ),
        ] {
            assert_eq!(ended_at_a_status(text).as_deref(), Some(told), "{text}");
        }
        for as_said in [
            "the tracker failed: HTTP status server error (503)",
            "HTTP status server error (503): see https://host/parcels",
            "HTTP status server error (unknown s3cr3t)",
            "HTTP status server error (",
            "HTTP status of the tracker: client error",
            "HTTP status ",
        ] {
            assert_eq!(ended_at_a_status(as_said), None, "{as_said}");
        }
    }

    /// An error naming reqwest's as its source, under a sentence of its own.
    #[derive(Debug, thiserror::Error)]
    #[error("the tracker failed")]
    struct Named(#[source] reqwest::Error);

    /// What a peer answering `503` makes of `error_for_status`: reqwest's
    /// error for a status, whose sentence quotes the phrase the peer wrote.
    async fn answered_503() -> reqwest::Error {
        use nest_rs_testing::{HttpsPeer, PeerAnswer, TestAuthority};

        let authority = TestAuthority::new();
        let peer =
            HttpsPeer::answering(&authority.server(&["127.0.0.1"]), [PeerAnswer::status(503)])
                .await;
        let trusted =
            reqwest::Certificate::from_pem(authority.pem().as_bytes()).expect("the authority");
        let error = reqwest::Client::builder()
            .tls_certs_only([trusted])
            .build()
            .expect("a client")
            .get(peer.url(&format!("/parcels/{SENT}")))
            .send()
            .await
            .expect("the peer answers")
            .error_for_status()
            .expect_err("a 503");
        assert!(
            error.to_string().contains("(503 Answer)"),
            "reqwest's own sentence quotes the peer's phrase: {error}"
        );
        error
    }

    /// The phrase after a status's code is the dependency's own words: no
    /// line tells it, whether the error names reqwest's as its source, which
    /// its `SafeLink` then says, or hands its sentence on, which hides it.
    #[tokio::test]
    async fn a_status_error_is_told_by_its_code_never_the_phrase_its_dependency_wrote() {
        assert_eq!(
            told(&Named(answered_503().await)),
            "the tracker failed: HTTP status server error (503)"
        );
        assert_eq!(
            told(&HandedOn(answered_503().await)),
            "HTTP status server error (503)"
        );
        assert_eq!(
            told(&HandedOn(answered_503().await.without_url())),
            "HTTP status server error (503)"
        );
    }

    /// An error naming reqwest's as its source behind the pointer sharing it,
    /// which is the link it hands out.
    #[derive(Debug, thiserror::Error)]
    #[error("the tracker failed")]
    struct Shared(#[source] std::sync::Arc<reqwest::Error>);

    /// An error holding the I/O error a reader over a body's stream makes of
    /// reqwest's.
    #[derive(Debug, thiserror::Error)]
    #[error("the download failed")]
    struct Unread(#[from] std::io::Error);

    /// reqwest's error for a peer presenting another name's certificate,
    /// whose cause is rustls's, naming the certificate's own names.
    async fn another_names() -> reqwest::Error {
        use nest_rs_testing::{HttpsPeer, PeerAnswer, TestAuthority};

        let authority = TestAuthority::new();
        let peer = HttpsPeer::answering(
            &authority.server(&["ledger.internal"]),
            [PeerAnswer::status(200)],
        )
        .await;
        let trusted =
            reqwest::Certificate::from_pem(authority.pem().as_bytes()).expect("the authority");
        let error = reqwest::Client::builder()
            .tls_certs_only([trusted])
            .build()
            .expect("a client")
            .get(peer.url(&format!("/parcels/{SENT}?token={SENT}")))
            .send()
            .await
            .expect_err("not this host's certificate");
        assert!(
            format!("{error:?}").contains("ledger.internal"),
            "its cause names the certificate's names: {error:?}"
        );
        error
    }

    /// reqwest's error held behind a pointer, or inside an I/O error, is the
    /// link an error naming it hands out: told as one named bare, its causes
    /// by what their types tell, never rustls's quote of a certificate.
    #[tokio::test]
    async fn a_reqwest_error_held_behind_a_pointer_is_told_as_one_named_bare() {
        const CAUSE: &str =
            "error sending request: invalid peer certificate: certificate not valid for name";
        assert_eq!(
            told(&Named(another_names().await)),
            format!("the tracker failed: {CAUSE}")
        );
        assert_eq!(
            told(&Shared(std::sync::Arc::new(another_names().await))),
            format!("the tracker failed: {CAUSE}")
        );
        assert_eq!(
            told(&Unread(std::io::Error::other(another_names().await))),
            format!("the download failed: {CAUSE}")
        );
        assert_eq!(
            told(&Shared(std::sync::Arc::new(answered_503().await))),
            "the tracker failed: HTTP status server error (503)"
        );
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
