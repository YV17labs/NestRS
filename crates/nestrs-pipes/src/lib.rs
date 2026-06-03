//! Transport-agnostic validation and transformation pipes — the NestJS *pipes*
//! analog. A [`Pipe`] is a pure transform run at a surface's request boundary,
//! between extraction and the handler; the surface binds it to a parameter
//! (HTTP does so via the `Valid<E>` / `Piped<P, E>` extractors in
//! `nestrs-http`).

mod pipe;
mod pipes;

pub use pipe::{Pipe, PipeError};
pub use pipes::{
    Lowercase, Parse, ParseArray, ParseBool, ParseFloat, ParseInt, ParseUuid, ParseUuidV3,
    ParseUuidV4, ParseUuidV5, ParseUuidV7, ParseUuidVersion, Trim, Uppercase, ValidationPipe,
};
