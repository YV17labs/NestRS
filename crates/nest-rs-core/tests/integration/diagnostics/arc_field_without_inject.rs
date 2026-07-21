//! CORE-I5: a non-`#[inject]` `Arc<…>` field would be silently
//! `Default::default()`-d (an empty config, a no-op guard/strategy) — a security
//! footgun. The macro must reject it, pointing at `#[inject]`.

use std::sync::Arc;

use nest_rs_core::injectable;

struct StripeConfig;

#[injectable]
struct PaymentsService {
    // Forgotten `#[inject]` — an empty `StripeConfig` would be injected silently.
    config: Arc<StripeConfig>,
}

fn main() {}
