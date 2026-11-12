/// `std::any::type_name::<T>()` without its module paths, keeping the leaf of
/// each segment so generics survive: `a::b::AuthnGuard<c::d::JwtStrategy<e::Claims>>`
/// reads `AuthnGuard<JwtStrategy<Claims>>`. Cutting at the last `::` alone would
/// read `Claims>>`.
pub fn short_type_name<T: ?Sized>() -> String {
    shorten(std::any::type_name::<T>())
}

/// [`short_type_name`] for a name already in hand.
pub fn shorten(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut segment_start = 0;
    for (i, c) in name.char_indices() {
        if !is_path_char(c) {
            out.push_str(leaf(&name[segment_start..i]));
            out.push(c);
            segment_start = i + c.len_utf8();
        }
    }
    out.push_str(leaf(&name[segment_start..]));
    out
}

/// [`shorten`] for a name the binary holds, held as long: a name of one path
/// is its own last segment, and any other is shortened once per process.
pub(crate) fn short_static(name: &'static str) -> &'static str {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock, PoisonError};

    if name.chars().all(is_path_char) {
        return leaf(name);
    }
    static SHORTENED: OnceLock<Mutex<HashMap<&'static str, &'static str>>> = OnceLock::new();
    let mut shortened = SHORTENED
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    shortened
        .entry(name)
        .or_insert_with(|| shorten(name).leak())
}

/// Whether `c` belongs to a path segment, `::` included.
fn is_path_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == ':'
}

fn leaf(segment: &str) -> &str {
    segment.rsplit("::").next().unwrap_or(segment)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_segment_keeps_its_leaf() {
        assert_eq!(
            shorten("a::b::AuthnGuard<c::d::JwtStrategy<e::Claims>>"),
            "AuthnGuard<JwtStrategy<Claims>>"
        );
    }

    #[test]
    fn a_held_name_shortens_as_any_other() {
        for name in [
            std::any::type_name::<std::num::NonZeroU64>(),
            "a::b::AuthnGuard<c::d::JwtStrategy<e::Claims>>",
            "a::b::LedgerModule",
            "LedgerModule",
        ] {
            assert_eq!(short_static(name), shorten(name));
        }
    }

    #[test]
    fn a_generic_from_std_reads_whole() {
        assert_eq!(short_type_name::<std::num::NonZeroU64>(), "NonZero<u64>");
        assert_eq!(short_type_name::<u64>(), "u64");
    }
}
