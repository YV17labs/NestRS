use nest_rs::core::module;
use nest_rs::events::EventsModule;

use super::guard::PostAuthorGuard;
use super::service::PostsService;
use super::store::PublicationStore;

#[module(
    imports = [EventsModule],
    providers = [PostsService, PublicationStore, PostAuthorGuard],
)]
pub struct PostsModule;
