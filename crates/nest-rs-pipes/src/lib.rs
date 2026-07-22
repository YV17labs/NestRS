//! Transport-agnostic validation and transformation pipes.
//!
//! Two flavors:
//!
//! - **Per-extractor [`Pipe`]** — pure transform run at a surface's
//!   request boundary, between extraction and the handler. HTTP binds it
//!   via the `Valid<E>` / `Piped<P, E>` extractors in `nest-rs-http`.
//! - **[`GlobalPipe`]** — applies to every JSON request body across the
//!   app. Runs after Guards, before the handler — the
//!   [`LayerKind::Pipe`](nest_rs_core::LayerKind) slot.
//!
//! # Where `use_pipes_global` lives
//!
//! Registration is imported from **`nest-rs-guards`**, not from here:
//!
//! ```rust,ignore
//! use nest_rs_guards::{AppBuilderPipesExt, pipe};
//!
//! App::builder().use_pipes_global([pipe::<ValidationPipe>()])
//! ```
//!
//! The asymmetry with the other layer families is structural, not an
//! oversight: `nest-rs-guards` owns the route shaper that *executes* the pipe
//! pool, and it already depends on this crate for [`GlobalPipe`] — so hosting
//! `use_pipes_global` here would close a dependency cycle. The pipe trait lives
//! with the pipes; the registration lives with the dispatch that runs it.
#![warn(missing_docs)]

mod global;
mod pipe;
mod piped;
mod pipes;
mod validate;

pub use global::GlobalPipe;
pub use pipe::{Pipe, PipeError};
pub use piped::{Piped, Valid};
pub use pipes::{
    Lowercase, Parse, ParseArray, ParseBool, ParseFloat, ParseInt, ParseUuid, ParseUuidV3,
    ParseUuidV4, ParseUuidV5, ParseUuidV7, ParseUuidVersion, Trim, Uppercase, ValidationPipe,
};
pub use validate::{MaybeValidateFallback, ValidateProbe};
