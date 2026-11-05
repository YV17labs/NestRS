//! `OAuthResourceModule::for_root` given a value it takes no such declaration in
//! says what it takes, naming no conversion: the setup has none.

use nest_rs::core::module;
use nest_rs::oauth::resource::OAuthResourceModule;

#[module(imports = [OAuthResourceModule::for_root("posts:read")])]
struct AppModule;

fn main() {}
