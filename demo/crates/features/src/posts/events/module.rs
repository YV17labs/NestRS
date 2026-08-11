use nest_rs::core::module;

use super::listener::PostsListener;
use crate::posts::PostsModule;

#[module(
    imports = [PostsModule],
    providers = [PostsListener],
)]
pub struct PostsEventsModule;
