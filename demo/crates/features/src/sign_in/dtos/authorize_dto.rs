use nest_rs::http::input;

#[input]
pub struct AuthorizeDto {
    pub resource: Option<String>,
}
