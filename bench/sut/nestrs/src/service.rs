use nest_rs::core::injectable;

use crate::dto::ItemDto;

#[injectable]
#[derive(Default)]
pub struct HelloService;

impl HelloService {
    pub fn greeting(&self) -> String {
        "Hello World".to_string()
    }

    pub fn items(&self) -> Vec<ItemDto> {
        (0..1000)
            .map(|id| ItemDto {
                id,
                message: self.greeting(),
            })
            .collect()
    }
}
