//! `priority` takes an integer literal in `i32`'s range — negative ones
//! included — and is refused in the sentence every value refusal opens with its
//! site. "priority must be an integer" named neither the decorator nor the key,
//! and a literal too large for an `i32` got syn's own "number too large".

use nest_rs_http::interceptor;

#[interceptor(priority = "high")]
#[derive(Default)]
struct Named;

#[interceptor(priority = 3000000000)]
#[derive(Default)]
struct TooLarge;

fn main() {}
