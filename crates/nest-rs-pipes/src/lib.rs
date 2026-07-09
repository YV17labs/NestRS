//! Transport-agnostic validation and transformation pipes.
//!
//! Two flavors:
//!
//! - **Per-extractor [`Pipe`]** — pure transform run at a surface's
//!   request boundary, between extraction and the handler. HTTP binds it
//!   via the `Valid<E>` / `Piped<P, E>` extractors in `nest-rs-http`.
//! - **[`GlobalPipe`]** — applies to every JSON request body across the
//!   app. Declared with `App::builder().use_pipes_global(...)`. Runs after
//!   Guards, before the handler — the [`LayerKind::Pipe`](nest_rs_core::LayerKind)
//!   slot.
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
