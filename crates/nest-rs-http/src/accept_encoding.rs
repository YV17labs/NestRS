//! `Accept-Encoding` as RFC 9110 §12.5.3 reads it, for the codings the
//! transport compresses a response with: each weight, `identity` and `*`, a
//! coding refused with `q=0`, whitespace where the grammar allows it.

use http::HeaderMap;
use http::header::ACCEPT_ENCODING;

/// A content coding the transport compresses a response with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Coding {
    Zstd,
    Brotli,
    Gzip,
    Deflate,
}

impl Coding {
    /// Every coding offered, the first winning a tie between equal weights.
    const OFFERED: [Self; 4] = [Self::Zstd, Self::Brotli, Self::Gzip, Self::Deflate];

    /// The coding's token (RFC 9110 §8.4.1, IANA's content coding registry).
    fn token(self) -> &'static str {
        match self {
            Self::Zstd => "zstd",
            Self::Brotli => "br",
            Self::Gzip => "gzip",
            Self::Deflate => "deflate",
        }
    }
}

/// A weight in thousandths: `q=0.5` is `500`, `1` is [`FULL`].
type Weight = u16;

/// The weight of an element that states none.
const FULL: Weight = 1000;

/// What a request's `Accept-Encoding` weighs each coding at.
#[derive(Default)]
pub(crate) struct AcceptEncoding {
    /// The offered codings, in [`Coding::OFFERED`]'s order.
    named: [Option<Weight>; 4],
    identity: Option<Weight>,
    any: Option<Weight>,
}

impl AcceptEncoding {
    /// The weights `headers` state; none when they carry no `Accept-Encoding`.
    pub(crate) fn of(headers: &HeaderMap) -> Option<Self> {
        if !headers.contains_key(ACCEPT_ENCODING) {
            return None;
        }
        let mut weights = Self::default();
        let elements = headers
            .get_all(ACCEPT_ENCODING)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','));
        for element in elements {
            let Some((token, weight)) = element_of(element) else {
                continue;
            };
            let slot = match token {
                _ if token.eq_ignore_ascii_case("identity") => &mut weights.identity,
                "*" => &mut weights.any,
                _ => match Coding::OFFERED
                    .iter()
                    .position(|coding| token.eq_ignore_ascii_case(coding.token()))
                {
                    Some(index) => &mut weights.named[index],
                    None => continue,
                },
            };
            *slot = Some(weight);
        }
        Some(weights)
    }

    /// The coding to compress the response with, or `None` to answer it
    /// unencoded: the coding weighed highest, unless none is acceptable or a
    /// weight stated for `identity` is higher. Unstated, `identity` is
    /// acceptable without outranking any coding (RFC 9110 §12.5.3).
    pub(crate) fn negotiate(&self) -> Option<Coding> {
        let (best, weight) = Coding::OFFERED
            .into_iter()
            .enumerate()
            .map(|(index, coding)| (coding, self.coding(index)))
            .fold(
                None,
                |best: Option<(Coding, Weight)>, (coding, weight)| match best {
                    Some((_, kept)) if kept >= weight => best,
                    _ => Some((coding, weight)),
                },
            )?;
        (weight > 0 && self.identity.is_none_or(|identity| weight >= identity)).then_some(best)
    }

    /// What `coding` weighs: as named, else as `*` says, else not acceptable.
    fn coding(&self, index: usize) -> Weight {
        self.named[index].or(self.any).unwrap_or(0)
    }
}

/// One element, `coding [ OWS ";" OWS "q=" qvalue ]`: its token and weight, or
/// `None` when it is empty or malformed.
fn element_of(element: &str) -> Option<(&str, Weight)> {
    let mut parts = element.split(';');
    let token = parts.next()?.trim();
    if token.is_empty() {
        return None;
    }
    let mut weight = FULL;
    for parameter in parts {
        let (name, value) = parameter.split_once('=')?;
        if name.trim().eq_ignore_ascii_case("q") {
            weight = qvalue(value.trim())?;
        }
    }
    Some((token, weight))
}

/// `qvalue = ( "0" [ "." 0*3DIGIT ] ) / ( "1" [ "." 0*3("0") ] )`.
fn qvalue(value: &str) -> Option<Weight> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if fraction.len() > 3 || !fraction.bytes().all(|digit| digit.is_ascii_digit()) {
        return None;
    }
    let thousandths = fraction
        .bytes()
        .chain(std::iter::repeat(b'0'))
        .take(3)
        .fold(0, |sum, digit| sum * 10 + Weight::from(digit - b'0'));
    match whole {
        "0" => Some(thousandths),
        "1" if thousandths == 0 => Some(FULL),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use http::HeaderValue;

    use super::*;

    fn chosen(accept_encoding: &str) -> Option<Coding> {
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT_ENCODING,
            HeaderValue::from_str(accept_encoding).expect("a header value"),
        );
        AcceptEncoding::of(&headers)?.negotiate()
    }

    #[test]
    fn the_highest_weight_wins_and_a_tie_goes_to_the_first_offered() {
        assert_eq!(chosen("gzip;q=0.2, deflate;q=0.9"), Some(Coding::Deflate));
        assert_eq!(chosen("gzip, deflate, br, zstd"), Some(Coding::Zstd));
        assert_eq!(chosen("deflate, gzip"), Some(Coding::Gzip));
        assert_eq!(
            chosen("GZIP"),
            Some(Coding::Gzip),
            "tokens are case-insensitive"
        );
    }

    #[test]
    fn whitespace_around_the_weight_is_the_grammars() {
        assert_eq!(chosen("gzip ; q=0.2 , br ;Q=0.3"), Some(Coding::Brotli));
    }

    #[test]
    fn identity_and_zero_weights_are_honoured() {
        assert_eq!(chosen("gzip;q=0.5, identity;q=1"), None);
        assert_eq!(chosen("gzip;q=0"), None);
        assert_eq!(chosen("*;q=0, identity"), None);
        assert_eq!(chosen("*"), Some(Coding::Zstd), "`*` names every coding");
        assert_eq!(chosen("*, zstd;q=0"), Some(Coding::Brotli));
        assert_eq!(
            chosen("gzip, identity"),
            Some(Coding::Gzip),
            "a tie compresses"
        );
    }

    #[test]
    fn nothing_offered_or_nothing_valid_answers_unencoded() {
        assert!(AcceptEncoding::of(&HeaderMap::new()).is_none());
        assert_eq!(chosen(""), None);
        assert_eq!(chosen("compress, x-unknown"), None);
        assert_eq!(chosen("gzip;q=2"), None, "a weight past 1 is malformed");
        assert_eq!(chosen("gzip;q=0.1234"), None, "four decimals are malformed");
    }

    #[test]
    fn a_qvalue_is_read_in_thousandths() {
        assert_eq!(qvalue("0"), Some(0));
        assert_eq!(qvalue("0.5"), Some(500));
        assert_eq!(qvalue("0.125"), Some(125));
        assert_eq!(qvalue("1.000"), Some(1000));
        assert_eq!(qvalue("1.5"), None);
        assert_eq!(qvalue(".5"), None);
    }
}
