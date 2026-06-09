use nest_rs_core::module;

use super::service::PostsService;

#[module(providers = [PostsService])]
pub struct PostsModule;
