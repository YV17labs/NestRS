//! `StaticFilesModule::for_root` given a value it takes no such declaration in
//! says what it takes, naming no conversion: the setup has none.

use nest_rs::core::module;
use nest_rs::static_files::StaticFilesModule;

#[module(imports = [StaticFilesModule::for_root("web/dist")])]
struct AppModule;

fn main() {}
