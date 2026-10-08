//! Output surfaces of `#[expose]`: relation loader traits ([`relations`]) and
//! the wire defaults trait ([`wire`]) the macro fills in.

#[cfg(feature = "graphql")]
pub(crate) mod relations;
pub(crate) mod wire;
