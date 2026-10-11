//! Transport-agnostic validation and transformation pipes.
//!
//! Two flavors:
//!
//! - **Per-extractor [`Pipe`]** — pure transform run at a surface's
//!   request boundary, between extraction and the handler. HTTP binds it
//!   via the `Valid<E>` / `Piped<P, E>` extractors in `nest-rs-http`.
//! - **[`GlobalPipe`]** — a provider registered once for the whole app, run
//!   after Guards and before the handler — the
//!   [`LayerKind::Pipe`](nest_rs_core::LayerKind) slot.
//!
//! # Registering global pipes
//!
//! `App::builder().use_pipes_global([pipe::<StripUnknownFields>()])`, through
//! [`AppBuilderPipesExt`]; the example on [`pipe`](fn@pipe) runs it. The pool
//! sits here, beside its contract, and each edge resolves it once, when it
//! mounts: the HTTP route, `/graphql` and every WebSocket gateway. A global pipe
//! no imported module provides fails the boot of every app, whatever it serves.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

mod error;
mod global;
mod pipe;
mod piped;
mod pipes;
mod registry;
mod validate;

pub use error::PipeError;
pub use global::GlobalPipe;
pub use pipe::Pipe;
pub use piped::{Piped, Valid};
pub use pipes::{
    Lowercase, Parse, ParseArray, ParseBool, ParseFloat, ParseInt, ParseUuid, ParseUuidV3,
    ParseUuidV4, ParseUuidV5, ParseUuidV7, ParseUuidVersion, Trim, Uppercase, ValidationPipe,
};
pub use registry::{AppBuilderPipesExt, PipeSpec, PipeSpecs, pipe};
pub use validate::validation_details;

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's macro expansions and sibling crates. Not API:
    //! may change in any release.

    pub use crate::registry::resolve_global_pipes;
    pub use crate::validate::{MaybeValidateFallback, ValidateProbe};
}
