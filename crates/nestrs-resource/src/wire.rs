//! Defaults for `#[expose(skip)]` columns when reconciling a wire DTO back
//! into a SeaORM `Model` for response masking.

use sea_orm::EntityTrait;
use serde_json::{Map, Value};

/// Fills absent JSON keys for server-only columns before deserializing a
/// handler DTO into `Self::Model`. Emitted by `#[expose]` per skipped scalar
/// column; entities without an `#[expose]` impl get the default no-op (their
/// masking handlers must deserialize without skipped columns).
pub trait WireModelDefaults: EntityTrait {
    fn fill_wire_defaults(_map: &mut Map<String, Value>) {}
}
