use std::fmt::Write;
use std::time::Duration;

use poem::http::HeaderValue;

use crate::entry::Entry;

pub(crate) fn format_header(entries: &[Entry], total: Duration) -> Option<HeaderValue> {
    let mut buf = String::with_capacity(64);
    for e in entries.iter().filter(|e| is_valid_token(&e.name)) {
        if !buf.is_empty() {
            buf.push_str(", ");
        }
        write_entry(&mut buf, e);
    }
    if !buf.is_empty() {
        buf.push_str(", ");
    }
    let _ = write!(buf, "total;dur={:.2}", as_millis_f64(total));
    HeaderValue::from_str(&buf).ok()
}

fn write_entry(buf: &mut String, e: &Entry) {
    let _ = write!(buf, "{};dur={:.2}", e.name, as_millis_f64(e.dur));
    if let Some(desc) = &e.desc {
        // `server-timing-param-value` is `token / quoted-string` (RFC 7230);
        // non-token chars require quoting + `\`/`"` escaping.
        if is_valid_token(desc) {
            buf.push_str(";desc=");
            buf.push_str(desc);
        } else {
            buf.push_str(";desc=\"");
            for ch in desc.chars() {
                match ch {
                    '"' | '\\' => {
                        buf.push('\\');
                        buf.push(ch);
                    }
                    _ => buf.push(ch),
                }
            }
            buf.push('"');
        }
    }
}

fn as_millis_f64(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// RFC 7230 token: visible ASCII minus separators. Non-conforming names are
/// silently dropped rather than failing the response.
fn is_valid_token(s: &str) -> bool {
    !s.is_empty()
        && s.bytes().all(|b| {
            matches!(b,
                b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.'
                | b'^' | b'_' | b'`' | b'|' | b'~'
                | b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z'
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, ms: u64) -> Entry {
        Entry {
            name: name.into(),
            desc: None,
            dur: Duration::from_millis(ms),
        }
    }

    fn entry_with_desc(name: &str, desc: &str, ms: u64) -> Entry {
        Entry {
            name: name.into(),
            desc: Some(desc.into()),
            dur: Duration::from_millis(ms),
        }
    }

    #[test]
    fn header_includes_total() {
        let h = format_header(&[], Duration::from_millis(12)).unwrap();
        assert!(h.to_str().unwrap().contains("total;dur="));
    }

    #[test]
    fn header_includes_named_entries() {
        let h = format_header(&[entry("db", 5)], Duration::from_millis(20)).unwrap();
        let s = h.to_str().unwrap();
        assert!(s.contains("db;dur=5"));
        assert!(s.contains("total;dur=20"));
    }

    #[test]
    fn invalid_token_is_dropped() {
        let h = format_header(
            &[entry("bad name with space", 5)],
            Duration::from_millis(20),
        )
        .unwrap();
        let s = h.to_str().unwrap();
        assert!(!s.contains("bad"));
        assert!(s.contains("total;dur=20"));
    }

    #[test]
    fn token_desc_emits_unquoted() {
        let h = format_header(
            &[entry_with_desc("db", "users-fetch", 5)],
            Duration::from_millis(20),
        )
        .unwrap();
        let s = h.to_str().unwrap();
        assert!(s.contains("db;dur=5.00;desc=users-fetch"));
    }

    #[test]
    fn non_token_desc_is_quoted_and_escaped() {
        let h = format_header(
            &[entry_with_desc("db", r#"users "primary""#, 5)],
            Duration::from_millis(20),
        )
        .unwrap();
        let s = h.to_str().unwrap();
        assert!(s.contains(r#"db;dur=5.00;desc="users \"primary\"""#));
    }

    #[test]
    fn multiple_entries_separated_by_comma() {
        let h = format_header(
            &[entry("db", 5), entry("graphql", 3)],
            Duration::from_millis(20),
        )
        .unwrap();
        let s = h.to_str().unwrap();
        assert_eq!(s, "db;dur=5.00, graphql;dur=3.00, total;dur=20.00");
    }
}
