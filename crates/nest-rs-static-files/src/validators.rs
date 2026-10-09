//! [`Validators`] — the strong entity tag and modification time a served file
//! carries (RFC 9110 §8.8), and the preconditions a request states against
//! them (§13).

use std::time::{SystemTime, UNIX_EPOCH};

use poem::http::header::IF_RANGE;
use poem::http::{HeaderMap, HeaderValue};
use poem::web::headers::{
    ETag, Header, HeaderMapExt, IfMatch, IfModifiedSince, IfNoneMatch, IfUnmodifiedSince,
    LastModified,
};

/// A representation's validators, as the fields that carry them: a strong
/// entity tag, and the time it last changed when that is known.
#[derive(Clone, Debug)]
pub(crate) struct Validators {
    etag: HeaderValue,
    last_modified: Option<(SystemTime, HeaderValue)>,
}

/// What a request's preconditions make of a representation (§13.2.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Precondition {
    /// Answer as if none were stated.
    Proceed,
    /// The client's copy is current: `304`.
    NotModified,
    /// A precondition the representation fails: `412`.
    Failed,
}

impl Validators {
    /// A file on disk, tagged by its length and modification time — which
    /// change together with its bytes. `None` without a modification time:
    /// a length alone is no strong validator.
    pub(crate) fn of_file(len: u64, modified: Option<SystemTime>) -> Option<Self> {
        let modified = modified?;
        let since = modified.duration_since(UNIX_EPOCH).ok()?;
        let etag = format!(
            "\"{len:x}-{:x}-{:x}\"",
            since.as_secs(),
            since.subsec_nanos()
        );
        Some(Self {
            etag: HeaderValue::try_from(etag).ok()?,
            last_modified: with_header(modified),
        })
    }

    /// An embedded file, tagged by the digest of its bytes.
    pub(crate) fn of_digest(sha256: [u8; 32], last_modified: Option<SystemTime>) -> Option<Self> {
        let mut digest = [0; 64];
        let etag = format!(
            "\"{}\"",
            nest_rs_core::trace_context::hex(&sha256, &mut digest)
        );
        Some(Self {
            etag: HeaderValue::try_from(etag).ok()?,
            last_modified: last_modified.and_then(with_header),
        })
    }

    pub(crate) fn etag(&self) -> &HeaderValue {
        &self.etag
    }

    pub(crate) fn last_modified(&self) -> Option<&HeaderValue> {
        self.last_modified.as_ref().map(|(_, header)| header)
    }

    fn modified(&self) -> Option<SystemTime> {
        self.last_modified.as_ref().map(|(at, _)| *at)
    }

    fn parsed_etag(&self) -> Option<ETag> {
        self.etag.to_str().ok()?.parse().ok()
    }

    /// The preconditions of a `GET` or `HEAD`, in §13.2.2's order: `If-Match`,
    /// else `If-Unmodified-Since`; then `If-None-Match`, else
    /// `If-Modified-Since`. A field that does not parse is ignored.
    pub(crate) fn evaluate(&self, headers: &HeaderMap) -> Precondition {
        if let Some(if_match) = headers.typed_get::<IfMatch>() {
            if !self
                .parsed_etag()
                .is_some_and(|etag| if_match.precondition_passes(&etag))
            {
                return Precondition::Failed;
            }
        } else if let (Some(since), Some(modified)) =
            (headers.typed_get::<IfUnmodifiedSince>(), self.modified())
            && !since.precondition_passes(modified)
        {
            return Precondition::Failed;
        }
        if let Some(if_none_match) = headers.typed_get::<IfNoneMatch>() {
            if self
                .parsed_etag()
                .is_some_and(|etag| !if_none_match.precondition_passes(&etag))
            {
                return Precondition::NotModified;
            }
        } else if let (Some(since), Some(modified)) =
            (headers.typed_get::<IfModifiedSince>(), self.modified())
            && !since.is_modified(modified)
        {
            return Precondition::NotModified;
        }
        Precondition::Proceed
    }

    /// Whether a `Range` applies (§13.1.5): with no `If-Range`, or one naming
    /// this representation exactly — the strong tag, or the very date. A
    /// changed file is sent whole, never spliced onto a stale copy.
    pub(crate) fn range_applies(&self, headers: &HeaderMap) -> bool {
        let Some(if_range) = headers.get(IF_RANGE) else {
            return true;
        };
        if if_range.as_bytes().starts_with(b"\"") {
            return if_range == self.etag;
        }
        let stated = IfModifiedSince::decode(&mut std::iter::once(if_range)).ok();
        match (stated, self.modified()) {
            (Some(stated), Some(modified)) => stated == IfModifiedSince::from(modified),
            _ => false,
        }
    }
}

fn with_header(modified: SystemTime) -> Option<(SystemTime, HeaderValue)> {
    let mut headers = HeaderMap::new();
    headers.typed_insert(LastModified::from(modified));
    let header = headers.remove(poem::http::header::LAST_MODIFIED)?;
    Some((modified, header))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use poem::http::header::{IF_MATCH, IF_MODIFIED_SINCE, IF_NONE_MATCH, IF_UNMODIFIED_SINCE};

    use super::*;

    fn modified() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_700_000_000)
    }

    fn file() -> Validators {
        Validators::of_file(42, Some(modified())).unwrap()
    }

    fn etag(validators: &Validators) -> &str {
        validators.etag().to_str().unwrap()
    }

    fn headers(fields: &[(poem::http::HeaderName, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in fields {
            map.insert(name.clone(), HeaderValue::from_str(value).unwrap());
        }
        map
    }

    fn http_date(time: SystemTime) -> String {
        with_header(time).unwrap().1.to_str().unwrap().to_owned()
    }

    #[test]
    fn a_file_without_a_modification_time_carries_no_tag() {
        assert!(Validators::of_file(42, None).is_none());
        assert!(Validators::of_file(42, Some(UNIX_EPOCH - Duration::from_secs(1))).is_none());
    }

    #[test]
    fn a_digest_is_a_strong_quoted_tag() {
        let digest = Validators::of_digest([0xab; 32], None).unwrap();
        assert_eq!(etag(&digest).len(), 66);
        assert!(etag(&digest).starts_with("\"abab"));
        assert!(digest.parsed_etag().is_some());
        assert!(digest.last_modified().is_none());
    }

    #[test]
    fn a_matching_tag_or_date_is_not_modified() {
        let file = file();
        assert_eq!(file.evaluate(&HeaderMap::new()), Precondition::Proceed);
        assert_eq!(
            file.evaluate(&headers(&[(IF_NONE_MATCH, etag(&file))])),
            Precondition::NotModified,
        );
        let weak = format!("W/{}", etag(&file));
        assert_eq!(
            file.evaluate(&headers(&[(IF_NONE_MATCH, &weak)])),
            Precondition::NotModified,
            "If-None-Match compares weakly",
        );
        assert_eq!(
            file.evaluate(&headers(&[(IF_MODIFIED_SINCE, &http_date(modified()))])),
            Precondition::NotModified,
        );
        assert_eq!(
            file.evaluate(&headers(&[
                (IF_NONE_MATCH, "\"other\""),
                (IF_MODIFIED_SINCE, &http_date(modified())),
            ])),
            Precondition::Proceed,
            "If-Modified-Since is ignored beside If-None-Match",
        );
    }

    #[test]
    fn a_failed_precondition_is_412() {
        let file = file();
        assert_eq!(
            file.evaluate(&headers(&[(IF_MATCH, "\"other\"")])),
            Precondition::Failed,
        );
        assert_eq!(
            file.evaluate(&headers(&[(IF_MATCH, etag(&file))])),
            Precondition::Proceed,
        );
        let earlier = http_date(modified() - Duration::from_secs(60));
        assert_eq!(
            file.evaluate(&headers(&[(IF_UNMODIFIED_SINCE, &earlier)])),
            Precondition::Failed,
        );
    }

    #[test]
    fn a_range_applies_only_to_the_representation_if_range_names() {
        let file = file();
        assert!(file.range_applies(&HeaderMap::new()));
        assert!(file.range_applies(&headers(&[(IF_RANGE, etag(&file))])));
        assert!(!file.range_applies(&headers(&[(IF_RANGE, "\"stale\"")])));
        let weak = format!("W/{}", etag(&file));
        assert!(
            !file.range_applies(&headers(&[(IF_RANGE, &weak)])),
            "If-Range compares strongly"
        );
        assert!(file.range_applies(&headers(&[(IF_RANGE, &http_date(modified()))])));
        let later = http_date(modified() + Duration::from_secs(60));
        assert!(
            !file.range_applies(&headers(&[(IF_RANGE, &later)])),
            "a date matches exactly or not at all",
        );
    }
}
