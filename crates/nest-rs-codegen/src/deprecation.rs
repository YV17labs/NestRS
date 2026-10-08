//! A route's deprecation date, `#[api(deprecated = "YYYY-MM-DD")]`, and the
//! refusal of Rust's `#[deprecated]` where a route's would be meant.
//!
//! Rust's attribute deprecates an item for its Rust callers, and clippy holds
//! its `since` to a crate version (`deprecated_semver`); the `Deprecation`
//! header a client reads (RFC 9745) is a date. A route has no Rust caller but
//! the framework, so it says its date through `#[api]`.
//!
//! ```
//! # use nest_rs_codegen::deprecation_header;
//! let since: syn::LitStr = syn::parse_quote!("2026-10-08");
//! assert_eq!(deprecation_header(&since)?, "@1791417600");
//! # Ok::<(), syn::Error>(())
//! ```

use syn::{Attribute, LitStr};

use crate::args::takes_value;

/// The `Deprecation` header value for `since`: an sf-date, `@` and the date's
/// Unix time at midnight UTC.
pub fn deprecation_header(since: &LitStr) -> syn::Result<String> {
    full_date_epoch(&since.value())
        .map(|epoch| format!("@{epoch}"))
        .ok_or_else(|| {
            syn::Error::new_spanned(
                since,
                takes_value(
                    "api",
                    Some("deprecated"),
                    "an RFC 3339 full-date, e.g. `deprecated = \"2026-10-08\"`: the \
                     `Deprecation` header (RFC 9745) a client reads is a date",
                ),
            )
        })
}

/// Refuse Rust's `#[deprecated]` among `attrs`, on `item` — `a #[routes]
/// handler` or `a #[controller]` — naming the form a route's deprecation takes.
pub fn refuse_rust_deprecated(attrs: &[Attribute], item: &str) -> syn::Result<()> {
    match attrs.iter().find(|a| a.path().is_ident("deprecated")) {
        Some(attr) => Err(syn::Error::new_spanned(
            attr,
            format!(
                "`#[deprecated]` on {item} deprecates Rust code, whose only caller is the \
                 framework: a route is deprecated for its clients with \
                 `#[api(deprecated = \"2026-10-08\")]` on its handler, which the document and \
                 the `Deprecation` header (RFC 9745) follow"
            ),
        )),
        None => Ok(()),
    }
}

/// `YYYY-MM-DD` as Unix time at midnight UTC, or `None` when it is no date.
fn full_date_epoch(raw: &str) -> Option<i64> {
    let bytes = raw.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let digits = |range: std::ops::Range<usize>| -> Option<i64> {
        raw.get(range)?.bytes().try_fold(0_i64, |acc, b| {
            b.is_ascii_digit().then(|| acc * 10 + i64::from(b - b'0'))
        })
    };
    let (year, month, day) = (digits(0..4)?, digits(5..7)?, digits(8..10)?);
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let month_days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if !(1..=month_days).contains(&day) {
        return None;
    }
    // Days from the civil date (Howard Hinnant, `days_from_civil`).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86_400)
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    #[test]
    fn a_full_date_is_its_unix_time_at_midnight_utc() {
        assert_eq!(full_date_epoch("1970-01-01"), Some(0));
        assert_eq!(full_date_epoch("2000-02-29"), Some(951_782_400));
        assert_eq!(full_date_epoch("2026-10-08"), Some(1_791_417_600));
        for bad in [
            "2026-02-29",
            "2026-13-01",
            "2026-00-10",
            "2026-1-01",
            "26-10-08",
            "v7.1",
        ] {
            assert_eq!(full_date_epoch(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_since_that_is_no_date_is_refused_naming_the_header() {
        let err = deprecation_header(&parse_quote!("7.1"))
            .expect_err("a version is no date")
            .to_string();
        assert!(
            err.contains("#[api] `deprecated` takes an RFC 3339 full-date"),
            "{err}"
        );
        assert!(err.contains("RFC 9745"), "{err}");
    }

    #[test]
    fn rusts_deprecated_is_refused_naming_the_route_form() {
        let method: syn::ImplItemFn = parse_quote! {
            #[deprecated(since = "7.1.0")]
            async fn list(&self) {}
        };
        let err = refuse_rust_deprecated(&method.attrs, "a `#[routes]` handler")
            .expect_err("refused")
            .to_string();
        assert!(err.contains("#[api(deprecated = "), "{err}");
        let plain: syn::ImplItemFn = parse_quote! { async fn list(&self) {} };
        assert!(refuse_rust_deprecated(&plain.attrs, "a `#[routes]` handler").is_ok());
    }
}
