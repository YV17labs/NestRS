use nest_rs::core::module;

use super::controller::PostsController;
use super::interceptor::PostAuditInterceptor;
use crate::authz::AuthzModule;
use crate::posts::PostsModule;

#[module(
    imports = [PostsModule, AuthzModule],
    providers = [PostsController, PostAuditInterceptor],
)]
pub struct PostsHttpModule;
