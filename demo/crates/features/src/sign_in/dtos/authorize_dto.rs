use nest_rs::core::input;

#[input]
pub struct AuthorizeDto {
    pub resource: Option<String>,
}
