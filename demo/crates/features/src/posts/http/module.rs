use nest_rs_core::module;

use super::controller::PostsController;
use super::guard::PostAuthorGuard;
use crate::authz::AuthzHttpModule;
use crate::posts::PostsModule;

#[module(
    imports = [PostsModule, AuthzHttpModule],
    providers = [PostsController, PostAuthorGuard],
)]
pub struct PostsHttpModule;
