//! `#[instance]` — its `Instance` impl and link-time declaration resolve
//! through the umbrella alone.

use nest_rs::core::instance;

/// A second connection to a resource the app opens twice.
#[instance]
pub struct HygieneCache;
