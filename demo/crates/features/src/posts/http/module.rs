use nest_rs::core::module;

use super::controller::PostsController;
use super::exception_filter::PostProblemFilter;
use super::interceptor::PostAuditInterceptor;
use crate::app_authz::AppAuthzHttpModule;
use crate::posts::PostsModule;

#[module(
    imports = [PostsModule, AppAuthzHttpModule],
    providers = [
        PostsController,
        PostAuditInterceptor,
        PostProblemFilter,
    ],
)]
pub struct PostsHttpModule;
