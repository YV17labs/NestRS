//! Output surfaces of `#[expose]`: relation loader traits ([`relations`]) and
//! the wire defaults trait ([`wire`]) the macro fills in. Grouped together
//! because they all serve the same generated code.

#[cfg(feature = "graphql")]
pub mod relations;
pub mod wire;
