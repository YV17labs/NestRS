//! One of the four refusals a `key = value` grammar owes, pinned where the
//! compiler says it. Refusals are shared, not per key. One
//! helper, one sentence, every key it covers, **one trybuild snapshot per
//! site**.
//!
//! The second is a misspelling followed by another key: it was answered "takes
//! at most one `priority`" — a repeat of a key never written — and the
//! misspelling itself was never named.

use nest_rs_http::interceptor;

#[interceptor(order = 100)]
#[derive(Default)]
struct Tracing;

#[interceptor(prority = 1, order = 2)]
#[derive(Default)]
struct Timing;

fn main() {}
