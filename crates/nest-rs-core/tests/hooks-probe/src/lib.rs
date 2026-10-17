//! A package's `#[hooks]` host, for `nest-rs-core`'s suite: an app that does
//! not import [`HooksProbeModule`] never runs [`HooksProbe`]'s hook, and says
//! so at `debug`, as it does for any dependency's host.

mod module;
mod probe;

pub use module::HooksProbeModule;
pub use probe::HooksProbe;
