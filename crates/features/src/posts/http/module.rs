use nest_rs_core::module;

use super::controller::PostsController;
use crate::authz::AuthzHttpModule;
use crate::posts::PostsModule;

#[module(
    imports = [PostsModule, AuthzHttpModule],
    providers = [PostsController],
)]
pub struct PostsHttpModule;
