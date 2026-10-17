//! Re-export everything a custom guard implementation needs in one `use`; the
//! view of the edge a guard checks comes from that edge's crate.
//!
//! ```
//! use nest_rs_guards::prelude::*;
//! use nest_rs_http::HttpView;
//!
//! #[injectable]
//! #[derive(Default)]
//! pub struct MyGuard;
//!
//! impl Layer for MyGuard {}
//!
//! #[async_trait]
//! impl Guard for MyGuard {
//!     async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
//!         if cx.reflector().is_public() {
//!             return Ok(());
//!         }
//!         match HttpView::of(cx)?.request().header("x-api-key") {
//!             Some(_) => Ok(()),
//!             None => Err(Denial::unauthorized("an API key is required")),
//!         }
//!     }
//! }
//!
//! impl GuardFor<HttpView> for MyGuard {}
//! # fn main() {}
//! ```

pub use crate::{AppBuilderGuardsExt, Denial, Guard, GuardFor, GuardSpec, guard};
pub use async_trait::async_trait;
pub use nest_rs_core::{
    AmbientValue, Layer, LayerKind, LayerSite, Posture, UnitContext, injectable,
};
