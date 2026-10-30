use nest_rs::core::output;
use uuid::Uuid;

#[output]
pub struct PersonDto {
    pub id: Uuid,
    pub name: String,
}
