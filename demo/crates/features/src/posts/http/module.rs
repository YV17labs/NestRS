use nest_rs::core::module;
use nest_rs::http::HttpModule;

use super::controller::PostsController;
use super::interceptor::PostAuditInterceptor;
use crate::authz::AuthzModule;
use crate::posts::PostsModule;

#[module(
    imports = [PostsModule, AuthzModule, HttpModule],
    providers = [PostsController, PostAuditInterceptor],
)]
pub struct PostsHttpModule;
