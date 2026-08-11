use nest_rs::core::module;
use nest_rs::events::EventsModule;

use super::feed::PostFeed;
use super::guard::PostAuthorGuard;
use super::service::PostsService;

#[module(
    imports = [EventsModule],
    providers = [PostsService, PostAuthorGuard, PostFeed],
)]
pub struct PostsModule;
