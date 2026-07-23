use nest_rs_core::module;
use nest_rs_events::EventsModule;

use super::guard::PostAuthorGuard;
use super::service::PostsService;

#[module(
    imports = [EventsModule],
    providers = [PostsService, PostAuthorGuard],
)]
pub struct PostsModule;
