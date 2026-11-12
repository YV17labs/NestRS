//! [`RequestPath`] — a request's path below the mount, read strictly before
//! any lookup: each segment percent-decoded once, and refused if it could name
//! something the root must not serve.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

/// The longest name a segment may decode to: what the common filesystems
/// store (`NAME_MAX`), so a longer one names no file and touches no disk.
const MAX_SEGMENT_BYTES: usize = 255;

/// The one hidden name a root may serve below it, as its first segment
/// (RFC 8615): `security.txt` (RFC 9116), app-site associations, and the like.
const WELL_KNOWN: &str = ".well-known";

/// A request path below the mount, every segment decoded and allowed.
#[derive(Debug, Default)]
pub(crate) struct RequestPath<'a> {
    segments: Vec<Cow<'a, str>>,
}

/// Why a request path names nothing the root serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PathRefusal {
    /// A `%` not followed by two hex digits, or bytes that are not UTF-8: no
    /// path can be read off it, so the request is malformed (`400`).
    Malformed,
    /// A `.` or `..` segment — a browser resolves them before sending, so one
    /// on the wire was written to climb out of the root.
    DotSegment,
    /// A `/` or `\` inside a segment, or one decoded from `%2F` or `%5C`.
    EncodedSeparator,
    /// A control character, `NUL` included.
    ControlCharacter,
    /// A name beginning with `.` — `.env`, `.git/` — other than a leading
    /// `.well-known`.
    Hidden,
    /// An empty segment (`a//b`) or one longer than any file name.
    Unnameable,
}

impl PathRefusal {
    /// The `reason` field a refusal is logged with.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Malformed => "malformed",
            Self::DotSegment => "dot_segment",
            Self::EncodedSeparator => "encoded_separator",
            Self::ControlCharacter => "control_character",
            Self::Hidden => "hidden",
            Self::Unnameable => "unnameable",
        }
    }

    /// Whether the refusal reads as an attempt to reach outside the root, as
    /// against a name that is simply never served.
    pub(crate) fn is_escape_attempt(self) -> bool {
        matches!(
            self,
            Self::DotSegment | Self::EncodedSeparator | Self::ControlCharacter
        )
    }
}

impl<'a> RequestPath<'a> {
    /// Read `raw` — the path below the mount, without its leading `/` — or
    /// refuse it.
    pub(crate) fn parse(raw: &'a str) -> Result<Self, PathRefusal> {
        if raw.is_empty() {
            return Ok(Self::default());
        }
        let mut segments = Vec::new();
        for (position, raw) in raw.split('/').enumerate() {
            let segment = decode(raw)?;
            allowed(&segment, position == 0)?;
            segments.push(segment);
        }
        Ok(Self { segments })
    }

    /// The path as an embedded folder keys its files: segments joined by `/`.
    pub(crate) fn key(&self) -> String {
        self.segments.join("/")
    }

    pub(crate) fn relative(&self) -> PathBuf {
        self.segments.iter().map(|segment| &**segment).collect()
    }

    /// Whether the last segment names a file (`app.js`, `logo.svg`) rather
    /// than a client-side route: a miss there is a missing file, never a page.
    pub(crate) fn names_a_file(&self) -> bool {
        self.segments
            .last()
            .is_some_and(|segment| segment.contains('.'))
    }
}

/// `raw` percent-decoded once — borrowed when it holds no escape — or
/// [`PathRefusal::Malformed`].
#[expect(
    clippy::map_err_ignore,
    reason = "the refusal is the whole answer: the bytes that were not UTF-8 are the client's"
)]
fn decode(raw: &str) -> Result<Cow<'_, str>, PathRefusal> {
    if !raw.contains('%') {
        return Ok(Cow::Borrowed(raw));
    }
    let bytes = raw.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while let Some(&byte) = bytes.get(at) {
        if byte == b'%' {
            let digit = |at: usize| bytes.get(at).and_then(|b| char::from(*b).to_digit(16));
            let escaped = digit(at + 1)
                .zip(digit(at + 2))
                .and_then(|(high, low)| u8::try_from(high * 16 + low).ok());
            let Some(escaped) = escaped else {
                return Err(PathRefusal::Malformed);
            };
            decoded.push(escaped);
            at += 3;
        } else {
            decoded.push(byte);
            at += 1;
        }
    }
    String::from_utf8(decoded)
        .map(Cow::Owned)
        .map_err(|_| PathRefusal::Malformed)
}

/// Whether a decoded segment may name a served file.
fn allowed(segment: &str, first: bool) -> Result<(), PathRefusal> {
    if segment == "." || segment == ".." {
        return Err(PathRefusal::DotSegment);
    }
    if segment.contains(['/', '\\']) {
        return Err(PathRefusal::EncodedSeparator);
    }
    if segment.chars().any(char::is_control) {
        return Err(PathRefusal::ControlCharacter);
    }
    if segment.is_empty() || segment.len() > MAX_SEGMENT_BYTES {
        return Err(PathRefusal::Unnameable);
    }
    if is_hidden(segment, first) {
        return Err(PathRefusal::Hidden);
    }
    Ok(())
}

/// Whether `name` is one the root never serves: it begins with `.`, and is not
/// a leading `.well-known`.
fn is_hidden(name: &str, first: bool) -> bool {
    name.starts_with('.') && !(first && name == WELL_KNOWN)
}

/// Whether a path below the root, as the filesystem resolved it, holds a
/// hidden name — what a link inside the root may point at.
pub(crate) fn hides(relative: &Path) -> bool {
    relative
        .components()
        .enumerate()
        .any(|(position, part)| is_hidden(&part.as_os_str().to_string_lossy(), position == 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refusal(raw: &str) -> PathRefusal {
        RequestPath::parse(raw).unwrap_err()
    }

    #[test]
    fn a_plain_path_reads_segment_by_segment() {
        let path = RequestPath::parse("assets/app.js").unwrap();
        assert_eq!(path.key(), "assets/app.js");
        assert_eq!(path.relative(), PathBuf::from("assets").join("app.js"));
        assert!(path.names_a_file());
        assert!(
            !RequestPath::parse("settings/profile")
                .unwrap()
                .names_a_file()
        );
        assert_eq!(RequestPath::parse("").unwrap().key(), "");
        assert!(matches!(
            RequestPath::parse("assets").unwrap().segments[0],
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn a_segment_is_percent_decoded_once() {
        assert_eq!(
            RequestPath::parse("caf%C3%A9.txt").unwrap().key(),
            "café.txt"
        );
        assert_eq!(
            RequestPath::parse("%2541").unwrap().key(),
            "%41",
            "decoded once, never twice"
        );
    }

    #[test]
    fn a_climb_out_of_the_root_is_refused_however_it_is_spelled() {
        assert_eq!(refusal(".."), PathRefusal::DotSegment);
        assert_eq!(refusal("a/../b"), PathRefusal::DotSegment);
        assert_eq!(refusal("%2e%2e"), PathRefusal::DotSegment);
        assert_eq!(refusal("."), PathRefusal::DotSegment);
        assert_eq!(refusal("%2e%2e%2fetc"), PathRefusal::EncodedSeparator);
        assert_eq!(refusal("a%2Fb"), PathRefusal::EncodedSeparator);
        assert_eq!(refusal("a%5cb"), PathRefusal::EncodedSeparator);
        assert_eq!(refusal("a\\b"), PathRefusal::EncodedSeparator);
        assert_eq!(refusal("a%00b"), PathRefusal::ControlCharacter);
        assert_eq!(refusal("a%0Ab"), PathRefusal::ControlCharacter);
        for attempt in ["..", "a%2Fb", "a%00b"] {
            assert!(refusal(attempt).is_escape_attempt(), "{attempt}");
        }
    }

    #[test]
    fn a_hidden_name_is_refused_but_a_leading_well_known() {
        assert_eq!(refusal(".env"), PathRefusal::Hidden);
        assert_eq!(refusal(".git/config"), PathRefusal::Hidden);
        assert_eq!(refusal("assets/.DS_Store"), PathRefusal::Hidden);
        assert_eq!(refusal("%2eenv"), PathRefusal::Hidden);
        assert_eq!(refusal("assets/.well-known/x"), PathRefusal::Hidden);
        assert_eq!(refusal(".well-known/.secret"), PathRefusal::Hidden);
        assert!(!refusal(".env").is_escape_attempt());
        assert_eq!(
            RequestPath::parse(".well-known/security.txt")
                .unwrap()
                .key(),
            ".well-known/security.txt",
        );
    }

    #[test]
    fn a_path_no_file_can_be_named_by_is_refused() {
        assert_eq!(refusal("a//b"), PathRefusal::Unnameable);
        assert_eq!(refusal(&"a".repeat(256)), PathRefusal::Unnameable);
        assert!(RequestPath::parse(&"a".repeat(255)).is_ok());
    }

    #[test]
    fn a_path_that_cannot_be_read_is_malformed() {
        assert_eq!(refusal("a%2"), PathRefusal::Malformed);
        assert_eq!(refusal("a%zz"), PathRefusal::Malformed);
        assert_eq!(refusal("%FF"), PathRefusal::Malformed, "not UTF-8");
    }

    #[test]
    fn a_resolved_path_hides_what_a_request_could_not_name() {
        assert!(hides(Path::new(".env")));
        assert!(hides(Path::new("assets/.git/config")));
        assert!(!hides(Path::new(".well-known/security.txt")));
        assert!(hides(Path::new("assets/.well-known/x")));
        assert!(!hides(Path::new("assets/app.js")));
    }
}
