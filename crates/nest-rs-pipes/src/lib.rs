//! Transport-agnostic transformation pipes, and validation at the edge.
//!
//! - **Per-argument [`Pipe`]** — a pure transform an edge runs between decoding
//!   an argument and the handler, bound as [`Piped`] on every edge: `Piped<P>`
//!   where the edge decodes the argument itself, `Piped<P, Json<T>>` over an
//!   HTTP extractor.
//! - **[`GlobalPipe`]** — a provider registered once for the whole app, run
//!   after Guards and before the handler — the
//!   [`LayerKind::Pipe`](nest_rs_core::LayerKind) slot.
//! - **Validation** needs neither: every edge validates each argument whose
//!   type declares `validator` rules, once its guards admitted the caller, and
//!   answers a refusal as the [`PipeError`] a pipe's would be.
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
pub use global::{ArgumentKind, GlobalPipe};
pub use pipe::Pipe;
pub use piped::{Decoded, PipeOver, Piped, Source};
pub use pipes::{
    Lowercase, Parse, ParseArray, ParseBool, ParseFloat, ParseInt, ParseUuid, ParseUuidV3,
    ParseUuidV4, ParseUuidV5, ParseUuidV7, ParseUuidVersion, Trim, Uppercase,
};
pub use registry::{AppBuilderPipesExt, PipeSpec, pipe};
pub use validate::validation_details;

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's macro expansions and sibling crates. Not API:
    //! may change in any release.

    pub use crate::registry::{PipeSpecs, resolve_global_pipes};
    pub use crate::validate::{DeclaredRules, NoRules, OptionalRules, ValidateProbe};
}
