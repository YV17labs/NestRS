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
        if !(c.is_alphanumeric() || c == '_' || c == ':') {
            out.push_str(leaf(&name[segment_start..i]));
            out.push(c);
            segment_start = i + c.len_utf8();
        }
    }
    out.push_str(leaf(&name[segment_start..]));
    out
}

/// Whether a block written `impl {written}` in the module `here` reaches the
/// private items beside the struct `host` of the module `host_module`: by the
/// struct's own name, from its module or one below it.
pub const fn reaches_host(host: &str, written: &str, host_module: &str, here: &str) -> bool {
    if !same(host.as_bytes(), written.as_bytes()) {
        return false;
    }
    let (inner, outer) = (here.as_bytes(), host_module.as_bytes());
    if inner.len() < outer.len() {
        return false;
    }
    let mut i = 0;
    while i < outer.len() {
        if inner[i] != outer[i] {
            return false;
        }
        i += 1;
    }
    inner.len() == outer.len() || (inner.len() > outer.len() + 1 && inner[outer.len()] == b':')
}

const fn same(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
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
    fn a_block_reaches_its_host_by_its_name_from_its_module_or_below() {
        assert!(reaches_host("Notes", "Notes", "app::notes", "app::notes"));
        assert!(reaches_host(
            "Notes",
            "Notes",
            "app::notes",
            "app::notes::http"
        ));
        assert!(!reaches_host("Notes", "Alias", "app::notes", "app::notes"));
        assert!(!reaches_host("Notes", "Notes", "app::notes", "app::other"));
        assert!(!reaches_host(
            "Notes",
            "Notes",
            "app::notes",
            "app::notes_admin"
        ));
    }

    #[test]
    fn a_generic_from_std_reads_whole() {
        assert_eq!(short_type_name::<std::num::NonZeroU64>(), "NonZero<u64>");
        assert_eq!(short_type_name::<u64>(), "u64");
    }
}
