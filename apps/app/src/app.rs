use nestrs_core::module;

use crate::hello::HelloModule;

#[module(imports = [HelloModule])]
pub struct AppModule;
