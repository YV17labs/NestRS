use nest_rs_core::module;
use nest_rs_events::EventsModule;

use super::service::PostsService;

#[module(
    imports = [EventsModule],
    providers = [PostsService],
)]
pub struct PostsModule;
