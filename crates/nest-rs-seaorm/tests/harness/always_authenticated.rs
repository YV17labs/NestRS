//! The suites' authentication stand-in: every caller is the suite's own
//! `Anyone`, since on GraphQL a visitor ability cannot satisfy an
//! `#[authorize]`.

use nest_rs_core::{Layer, UnitContext, injectable};
use nest_rs_guards::{Denial, Guard, GuardFor, async_trait};

/// Authenticates every caller as `crate::Anyone`.
#[injectable]
#[derive(Default)]
pub(crate) struct AlwaysAuthenticated;

impl Layer for AlwaysAuthenticated {}

#[async_trait]
impl Guard for AlwaysAuthenticated {
    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        cx.attach(crate::Anyone);
        Ok(())
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for AlwaysAuthenticated {}
