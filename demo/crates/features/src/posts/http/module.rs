use nest_rs::core::module;

use super::controller::PostsController;
use super::exception_filter::PostProblemFilter;
use super::interceptor::PostAuditInterceptor;
use crate::authz::AuthzHttpModule;
use crate::posts::PostsModule;

#[module(
    imports = [PostsModule, AuthzHttpModule],
    providers = [
        PostsController,
        PostAuditInterceptor,
        PostProblemFilter,
    ],
)]
pub struct PostsHttpModule;
