use nest_rs::core::input;
use uuid::Uuid;

#[input]
pub struct PersonDto {
    pub id: Uuid,
    pub name: String,
}
