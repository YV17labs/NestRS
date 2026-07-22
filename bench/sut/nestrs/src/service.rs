use nest_rs_core::injectable;

#[injectable]
#[derive(Default)]
pub struct HelloService;

impl HelloService {
    pub fn greeting(&self) -> String {
        "Hello World".to_string()
    }
}
