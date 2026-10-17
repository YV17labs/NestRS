/// `std::any::type_name::<T>()` without its module paths, keeping the leaf of
/// each segment so generics survive: `a::b::AuthnGuard<c::d::JwtStrategy<e::Claims>>`
/// reads `AuthnGuard<JwtStrategy<Claims>>`. Cutting at the last `::` alone would
/// read `Claims>>`.
pub fn short_type_name<T: ?Sized>() -> String {
    shorten(std::any::type_name::<T>())
}

/// [`short_type_name`] for a name already in hand.
pub(crate) fn shorten(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut segment_start = 0;
    for (i, c) in name.char_indices() {
        if !(c.is_alphanumeric() || c == '_' || c == ':') {
            out.push_str(leaf(&name[segment_start..i]));
            out.push(c);
            segment_start = i + c.len_utf8();
        }
    }
    out.push_str(leaf(&name[segment_start..]));
    out
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
    fn a_generic_from_std_reads_whole() {
        assert_eq!(short_type_name::<std::num::NonZeroU64>(), "NonZero<u64>");
        assert_eq!(short_type_name::<u64>(), "u64");
    }
}
