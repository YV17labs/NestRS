//! The queue name rule, read at compile time — the copy `#[queue]` checks its
//! literals against.
//!
//! The rule's authority is `nest_rs_queue::QueueName`, which a macro crate cannot
//! depend on. This copy is pinned against it by a test in `nest-rs-queue`, and
//! the two refusals state one fact: the charset and length, and why the
//! characters left out are left out.

/// The longest a queue name, a dynamic queue's prefix or its key may be.
const MAX_LEN: usize = 128;

/// Whether `value` is 1 to 128 of `[A-Za-z0-9_.-]` — the rule a queue name, a
/// dynamic queue's prefix and its key share.
pub fn is_valid_queue_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_LEN
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

/// The refusal of a `#[queue]` literal outside the rule. `what` is the value's
/// role in the runtime's words — `"queue name"`, `"dynamic queue prefix"` — so
/// the compile error and the runtime error read as one sentence with a site
/// in front.
pub fn invalid_queue_name(attr: &str, key: &str, what: &str, value: &str) -> String {
    format!(
        "#[{attr}] `{key}`: {value:?} is not a valid {what}: it takes 1 to 128 ASCII letters, \
         digits, `_`, `.` or `-` — `:` and `#` are the separators a backend's keys and a dynamic \
         queue's instances are built with, and whitespace would reach a log field or a metric \
         label"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rule_takes_the_charset_and_the_length_and_nothing_else() {
        assert!(is_valid_queue_name("audio.preview-2_x"));
        assert!(is_valid_queue_name(&"a".repeat(MAX_LEN)));
        for refused in ["", "audio queue", "nestrs:queue", "tenant#acme", "é"] {
            assert!(!is_valid_queue_name(refused), "{refused:?}");
        }
        assert!(!is_valid_queue_name(&"a".repeat(MAX_LEN + 1)));
    }
}
