//! Authorization subjects.

/// Compile-time guardrail that `S` is a real subject rather than an arbitrary
/// type. Implemented for every SeaORM entity by the blanket bridge below.
pub trait Subject: 'static {}

impl<E: sea_orm::EntityTrait> Subject for E {}
