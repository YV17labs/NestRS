//! Static files and single-page apps beside the API, from one binary.
//!
//! Import [`StaticFilesModule`] and the HTTP transport serves a folder as the
//! router's fallback: every route and every self-mount answers first, and a
//! path under a prefix one of them owns — or under the global prefix — is
//! never a file's. The folder is a directory read at runtime, which
//! [`StaticFilesConfig::root`] names, or one compiled into the binary by
//! `#[derive(Embed)]` and named in code with [`Embedded::of`].
//!
//! What is served is held to the defaults a public folder needs: `GET` and
//! `HEAD` alone, no name beginning with `.` but `/.well-known/` (RFC 8615), a
//! strictly decoded path, every file resolved under the root, no listing,
//! conditional and range requests (RFC 9110), and a navigation fallback to the
//! single-page app's `index.html` only for a browser that asked for a page.
//!
//! The two sources are the whole set: neither is an extension point.

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — the boot line, and a request refused for what
/// it tried to reach.
pub const TARGET: &str = "nest_rs::static_files";

mod asset;
mod byte_range;
mod config;
mod disk;
mod embedded;
mod endpoint;
mod error;
mod files;
mod media_type;
mod module;
mod request_path;
mod validators;

pub use config::StaticFilesConfig;
pub use embedded::Embedded;
pub use error::StaticFilesError;
pub use module::{StaticFilesModule, StaticFilesOptions, StaticFilesSetup};

/// `#[derive(Embed)]` compiles a folder into the binary, for
/// [`Embedded::of`] to name as the source; give it this crate's re-export as
/// its `crate_path`, so the manifest names nothing but `nest-rs`.
///
/// ```
/// use nest_rs_static_files::{Embed, Embedded, StaticFilesModule};
///
/// #[derive(Embed)]
/// #[folder = "tests/harness/site"]
/// #[crate_path = "nest_rs_static_files::rust_embed"]
/// struct WebAssets;
///
/// let _import = StaticFilesModule::for_root(Embedded::of::<WebAssets>());
/// ```
pub use rust_embed::Embed;

/// [rust-embed](https://docs.rs/rust-embed), which `#[derive(Embed)]` expands
/// against: the path its `crate_path` attribute names.
pub use rust_embed;
