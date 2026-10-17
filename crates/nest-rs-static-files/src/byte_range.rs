//! [`ByteRange`] — the part of a representation a `Range` header asks for
//! (RFC 9110 §14.1.2), read for one range at a time.

use std::ops::Range;

/// What a `Range` header asks of a representation of a known length.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ByteRange {
    /// The whole representation: no range asked, another unit, a malformed
    /// set, or several ranges — each of which §14.2 lets a server ignore.
    Whole,
    /// One satisfiable range, `start..end` with `end` exclusive: `206`.
    Part(Range<u64>),
    /// One range reaching no byte of the representation: `416`.
    Unsatisfiable,
}

impl ByteRange {
    /// The range `header` asks of a representation `len` bytes long.
    pub(crate) fn of(header: &str, len: u64) -> Self {
        let Some((unit, set)) = header.split_once('=') else {
            return Self::Whole;
        };
        if !unit.trim().eq_ignore_ascii_case("bytes") {
            return Self::Whole;
        }
        let mut specs = set
            .split(',')
            .map(str::trim)
            .filter(|spec| !spec.is_empty());
        let (Some(spec), None) = (specs.next(), specs.next()) else {
            return Self::Whole;
        };
        let Some((first, last)) = spec.split_once('-') else {
            return Self::Whole;
        };
        match (position(first), position(last)) {
            (Some(Some(first)), Some(last)) => {
                if last.is_some_and(|last| last < first) {
                    return Self::Whole;
                }
                if first >= len {
                    return Self::Unsatisfiable;
                }
                let end = last.map_or(len, |last| last.min(len - 1) + 1);
                Self::Part(first..end)
            }
            // A suffix: the last `n` bytes, the whole of a shorter representation.
            (Some(None), Some(Some(suffix))) => {
                if suffix == 0 || len == 0 {
                    return Self::Unsatisfiable;
                }
                Self::Part(len.saturating_sub(suffix)..len)
            }
            _ => Self::Whole,
        }
    }
}

/// A position: `Some(None)` when empty, `Some(Some(n))` for digits, `None`
/// for anything else.
fn position(raw: &str) -> Option<Option<u64>> {
    if raw.is_empty() {
        return Some(None);
    }
    if !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    raw.parse().ok().map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_range_inside_the_representation_is_a_part() {
        assert_eq!(ByteRange::of("bytes=0-9", 100), ByteRange::Part(0..10));
        assert_eq!(ByteRange::of("bytes=90-", 100), ByteRange::Part(90..100));
        assert_eq!(ByteRange::of("bytes=-10", 100), ByteRange::Part(90..100));
        assert_eq!(ByteRange::of("Bytes = 5-5 ", 100), ByteRange::Part(5..6));
    }

    #[test]
    fn a_range_past_the_end_is_clipped_and_a_long_suffix_is_the_whole() {
        assert_eq!(ByteRange::of("bytes=50-500", 100), ByteRange::Part(50..100));
        assert_eq!(ByteRange::of("bytes=-500", 100), ByteRange::Part(0..100));
        assert_eq!(
            ByteRange::of(&format!("bytes=0-{}", u64::MAX), 100),
            ByteRange::Part(0..100),
        );
    }

    #[test]
    fn a_range_reaching_no_byte_is_unsatisfiable() {
        assert_eq!(ByteRange::of("bytes=100-", 100), ByteRange::Unsatisfiable);
        assert_eq!(
            ByteRange::of("bytes=200-300", 100),
            ByteRange::Unsatisfiable
        );
        assert_eq!(ByteRange::of("bytes=-0", 100), ByteRange::Unsatisfiable);
        assert_eq!(ByteRange::of("bytes=0-", 0), ByteRange::Unsatisfiable);
    }

    #[test]
    fn what_a_server_may_ignore_answers_the_whole() {
        for header in [
            "items=0-9",
            "bytes=0-9,20-29",
            "bytes=9-0",
            "bytes=a-b",
            "bytes=-",
            "bytes=",
            "bytes 0-9",
            "bytes=99999999999999999999-",
        ] {
            assert_eq!(ByteRange::of(header, 100), ByteRange::Whole, "{header}");
        }
    }
}
