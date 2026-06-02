//! Default values for `#[expose(skip)]` columns when reconciling a wire DTO
//! back into a SeaORM `Model` for response masking.

use sea_orm::EntityTrait;
use serde_json::{Map, Value};

/// Fills absent JSON keys for server-only columns before deserializing a handler
/// DTO into `Self::Model`. Emitted by `#[expose]` for each skipped scalar column;
/// entities without an `#[expose]` impl use the trait's default no-op when they
/// do not implement it — handlers masking those types must deserialize without
/// skipped columns, or add a manual impl.
pub trait WireModelDefaults: EntityTrait {
    fn fill_wire_defaults(_map: &mut Map<String, Value>) {}
}
