use nestrs_core::injectable;

#[injectable]
#[derive(Default)]
pub struct HelloService;

impl HelloService {
    pub fn greeting(&self) -> &'static str {
        "Hello World"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greeting_returns_hello_world() {
        assert_eq!(HelloService.greeting(), "Hello World");
    }
}
