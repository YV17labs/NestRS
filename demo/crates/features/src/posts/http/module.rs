use nest_rs_core::module;

use super::controller::PostsController;
use super::exception_filter::PostProblemFilter;
use super::guard::PostAuthorGuard;
use super::interceptor::PostAuditInterceptor;
use crate::authz::AuthzHttpModule;
use crate::posts::PostsModule;

#[module(
    imports = [PostsModule, AuthzHttpModule],
    providers = [
        PostsController,
        PostAuthorGuard,
        PostAuditInterceptor,
        PostProblemFilter,
    ],
)]
pub struct PostsHttpModule;
