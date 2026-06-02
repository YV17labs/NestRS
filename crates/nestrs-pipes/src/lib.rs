//! Validation and transformation pipes for nestrs — the NestJS *pipes* concept,
//! transport-agnostic.
//!
//! A [`Pipe`] is a pure transform run at a surface's request boundary, between
//! extraction and the handler: it either reshapes a value (`String` → `i64`) or
//! validates it, rejecting bad input with a [`PipeError`]. Pipes know nothing
//! about HTTP — a *surface* binds them to a parameter (the HTTP transport does
//! it with the `Valid<E>` / `Piped<P, E>` poem extractors in `nestrs-http`).
//!
//! Each pipe lives in its own module. The base set mirrors NestJS:
//!
//! | NestJS                         | here |
//! |--------------------------------|------|
//! | `ParseIntPipe`/`Float`/`Bool`  | [`Parse<T>`] + aliases [`ParseInt`]/[`ParseFloat`]/[`ParseBool`] |
//! | `ParseEnumPipe`                | [`Parse<T>`] for any `T: FromStr` enum |
//! | `ParseUUIDPipe` (`{ version }`)| [`ParseUuid`] / [`ParseUuidVersion`] + aliases |
//! | `ParseArrayPipe`               | [`ParseArray<T>`] |
//! | `ValidationPipe`               | [`ValidationPipe<T>`] |
//! | (transformation)               | [`Trim`] / [`Lowercase`] / [`Uppercase`] |
//!
//! Deliberately omitted, with the Rust-idiomatic replacement: `DefaultValuePipe`
//! — use `Option<T>` plus `unwrap_or`/`#[serde(default)]`, since a stateless
//! pipe can't carry a runtime default; `ParseFilePipe` — a multipart concern
//! that belongs to HTTP file handling, not a transport-agnostic pipe;
//! `ParseDatePipe` — would pull in a date crate, added behind a feature once a
//! date type is chosen.

mod pipe;
mod pipes;

pub use pipe::{Pipe, PipeError};
pub use pipes::{
    Lowercase, Parse, ParseArray, ParseBool, ParseFloat, ParseInt, ParseUuid, ParseUuidV3,
    ParseUuidV4, ParseUuidV5, ParseUuidV7, ParseUuidVersion, Trim, Uppercase, ValidationPipe,
};
